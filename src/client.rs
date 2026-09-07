//! Relaying one message, one step at a time.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use transport::error::{Result, classify, protocol_error};
use transport::wire::with_default_port;

use super::session::{expect, read_reply, say, write_stuffed};

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

/// Relay one message through one server.
///
/// # Errors
///
/// Where the target is not a mailbox, the relay could not be reached, or any
/// step of the exchange was refused.
pub fn relay(relay: &str, from: &str, target: &str, bytes: &[u8]) -> Result<()> {
    let recipient = Recipient::parse(target)?;
    let address = with_default_port(relay, 25);

    let mut stream =
        TcpStream::connect(&address).map_err(|e| classify("connecting to the relay", &e))?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| classify("cloning the connection", &e))?,
    );

    envelope(&mut stream, &mut reader, from, &recipient)?;
    body(&mut stream, bytes)?;
    finish(&mut stream, &mut reader)
}

/// Greeting through to `DATA`: who is sending, to whom.
fn envelope(
    stream: &mut impl Write,
    reader: &mut impl BufRead,
    from: &str,
    recipient: &Recipient,
) -> Result<()> {
    expect(reader, 220, "the greeting")?;

    say(stream, "EHLO xmip")?;
    expect(reader, 250, "EHLO")?;

    say(stream, &format!("MAIL FROM:<{from}>"))?;
    expect(reader, 250, "MAIL FROM")?;

    say(stream, &format!("RCPT TO:<{}>", recipient.mailbox()))?;
    expect(reader, 250, "RCPT TO")?;

    say(stream, "DATA")?;
    expect(reader, 354, "DATA")
}

/// The message itself, one line at a time, periods stuffed.
fn body(stream: &mut impl Write, bytes: &[u8]) -> Result<()> {
    for line in bytes.split(|byte| *byte == b'\n') {
        write_stuffed(stream, line.strip_suffix(b"\r").unwrap_or(line))?;
    }

    Ok(())
}

/// End the data, wait for the acknowledgement, say goodbye.
fn finish(stream: &mut impl Write, reader: &mut impl BufRead) -> Result<()> {
    say(stream, ".")?;
    expect(reader, 250, "the end of data")?;
    say(stream, "QUIT")?;

    // Wait for the goodbye before dropping the connection. Without this the
    // server is still writing 221 when the socket closes under it, and a
    // delivery that fully succeeded is reported as a broken pipe.
    let _ = read_reply(reader);

    Ok(())
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
    fn every_line_of_the_body_is_terminated_as_the_protocol_requires() {
        let mut written = Vec::new();
        body(&mut written, b"Subject: one\r\n\r\nhello").expect("written");

        assert_eq!(written, b"Subject: one\r\n\r\nhello\r\n");
    }

    #[test]
    fn a_leading_period_is_stuffed_so_it_does_not_end_the_message() {
        let mut written = Vec::new();
        body(&mut written, b"one\r\n.hidden").expect("written");

        assert_eq!(written, b"one\r\n..hidden\r\n");
    }
}
