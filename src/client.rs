//! Relaying messages through one server, one step at a time, on a session
//! kept between them.
//!
//! A session is greeted and said `EHLO` once, when it opens, and each
//! message after is one mail transaction on it: `MAIL FROM`, `RCPT TO`,
//! `DATA`. A transaction that ended — the end of its `DATA` answered,
//! whatever the answer — is followed straight by the next `MAIL FROM`, as
//! RFC 5321 section 3.3 allows; one refused before its end is reset by
//! `RSET` (section 4.1.1.5), so the session carries the next. The session
//! says `QUIT` when it is let go. Until 2026-10-21 every message connected,
//! greeted, said `EHLO` and `QUIT` of its own.

use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use transport::Pooled;
use transport::error::{Result, classify, protocol_error};
use transport::pool::quiet;
use transport::socket;

use super::session::{expect, judged, say};

/// One mailbox to deliver to.
///
/// Its own type so that "is this even a mailbox" is answered once, before a
/// connection is opened, rather than by the relay after a round trip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recipient(String);

impl Recipient {
    /// # Errors
    ///
    /// Where the target is not a mailbox. `mailto:` is optional; an `@` is not.
    pub fn parse(target: &str) -> Result<Self> {
        let mailbox = target.strip_prefix("mailto:").unwrap_or(target);

        if !mailbox.contains('@') {
            return Err(protocol_error(format!(
                "an smtp target must be a mailbox — got {target}"
            )));
        }

        Ok(Self(mailbox.to_string()))
    }

    #[must_use]
    pub fn mailbox(&self) -> &str {
        &self.0
    }
}

/// A session with one relay, greeted once and kept between messages.
pub struct Session {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// Whether the last transaction left the session able to carry the
    /// next: not where its `RSET` failed.
    open: bool,
}

impl Pooled for Session {
    /// A server speaks only when asked, so anything it sent to an idle
    /// session — a `421` before it closes — or its close lets it go.
    fn usable(&mut self) -> bool {
        self.open && self.reader.buffer().is_empty() && quiet(&self.writer)
    }
}

impl Session {
    /// Connect to the relay at `address` within `timeout`, with `timeout`
    /// on every read, and be greeted. `None` waits as long as the operating
    /// system does.
    ///
    /// # Errors
    ///
    /// Where the relay could not be reached within `timeout`, or did not
    /// greet or answer `EHLO`.
    pub fn open(address: &str, timeout: Option<Duration>) -> Result<Self> {
        // The connect is bounded as well as the reads. This was a bare
        // connect until 2026-09-21, which waits on the operating system's
        // schedule, and longer still on a machine out of ephemeral ports.
        let (mut reader, mut writer) = socket::split(socket::connect_tcp(address, timeout)?)?;
        expect(&mut reader, 220, "the greeting")?;
        say(&mut writer, "EHLO xmip")?;
        expect(&mut reader, 250, "EHLO")?;
        Ok(Self {
            reader,
            writer,
            open: true,
        })
    }

    /// Relay `bytes` from `from` to `recipient` as one mail transaction:
    /// `Ok(Ok(()))` where the server took it, `Ok(Err(refusal))` where it
    /// answered a step with a refusal — retryable on a `4xx`, permanent on
    /// a `5xx` — and the session is kept for the next.
    ///
    /// # Errors
    ///
    /// Where the session broke: the relay closed it or stopped answering.
    pub fn deliver(
        &mut self,
        from: &str,
        recipient: &Recipient,
        bytes: &[u8],
    ) -> Result<Result<()>> {
        let steps = [
            (format!("MAIL FROM:<{from}>"), 250, "MAIL FROM"),
            (format!("RCPT TO:<{}>", recipient.mailbox()), 250, "RCPT TO"),
            ("DATA".to_string(), 354, "DATA"),
        ];
        for (command, wanted, step) in &steps {
            say(&mut self.writer, command)?;
            if let Err(refused) = judged(&mut self.reader, *wanted, step)? {
                return Ok(Err(self.reset(refused)));
            }
        }
        body(&mut self.writer, bytes)?;
        // The transaction ends here, whatever the answer: the next goes
        // straight on with its `MAIL FROM`.
        judged(&mut self.reader, 250, "the end of data")
    }

    /// `refused`, the transaction it ended reset so the session carries
    /// the next, or let go where the reset is not answered.
    fn reset(&mut self, refused: transport::TransportError) -> transport::TransportError {
        self.open = say(&mut self.writer, "RSET")
            .and_then(|()| expect(&mut self.reader, 250, "RSET"))
            .is_ok();
        refused
    }
}

impl Drop for Session {
    /// Say goodbye. The reply is not waited for: every message the session
    /// carried was answered already, and nothing is left to report.
    fn drop(&mut self) {
        drop(say(&mut self.writer, "QUIT"));
    }
}

/// The message itself as one dot-stuffed block, bytes as they are, the
/// terminator after (`transport::stuffed`, since 2026-09-10).
fn body(stream: &mut impl Write, bytes: &[u8]) -> Result<()> {
    stream
        .write_all(&transport::stuffed::stuff(bytes))
        .map_err(|e| classify("writing the message data", &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mailto_prefix_is_optional() {
        assert_eq!(
            Recipient::parse("mailto:orders@example.com")
                .expect("parsed")
                .mailbox(),
            "orders@example.com"
        );
        assert_eq!(
            Recipient::parse("orders@example.com")
                .expect("parsed")
                .mailbox(),
            "orders@example.com"
        );
    }

    #[test]
    fn a_target_that_is_not_a_mailbox_is_rejected_before_connecting() {
        // A host and port is a plausible mistake and reaches a relay that
        // rejects it after a round trip. Better to refuse it here.
        let failure = Recipient::parse("127.0.0.1:25").expect_err("not a mailbox");

        assert!(!failure.retryable);
    }

    #[test]
    fn the_body_is_one_block_ended_by_the_terminator() {
        let mut written = Vec::new();
        body(&mut written, b"Subject: one\r\n\r\nhello").expect("written");

        assert_eq!(written, b"Subject: one\r\n\r\nhello\r\n.\r\n");
    }

    #[test]
    fn a_leading_period_is_stuffed_so_it_does_not_end_the_message() {
        let mut written = Vec::new();
        body(&mut written, b"one\r\n.hidden").expect("written");

        assert_eq!(written, b"one\r\n..hidden\r\n.\r\n");
    }
}
