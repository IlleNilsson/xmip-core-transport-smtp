//! Answering a client's commands, and the far end that takes one message
//! and answers it at once. Enough of RFC 5321 and no more. A Receive
//! Location answers the end of `DATA` only after its receive cycle
//! ([`crate::receiving`]); this module answers everything else.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use transport::error::{Result, classify};
use transport::socket;
use transport::taken::Taken;

use context::property::SMTP_MAIL_FROM;

use super::session::{read_data, say};

/// What one command came to.
pub enum Next {
    /// Answered: keep reading commands.
    Continue,
    /// `MAIL FROM` answered: the reverse-path it named, without its angle
    /// brackets, the sender the envelope says (RFC 5321 section 4.1.1.2).
    Mail(String),
    /// A message's `DATA` was read whole, and the end of it is not yet
    /// answered.
    Data(Vec<u8>),
    /// The client said goodbye, or hung up.
    Done,
}

/// The reply to the end of `DATA` that takes the message.
pub const ACCEPTED: &str = "250 accepted";

/// Take one message from an already-bound listener, within `timeout` and
/// with `timeout` on its reads, answering its end at once: the loopback's far
/// end, which holds what it took whole to compare.
///
/// `MAIL FROM` is a *passed* identity and belongs at the identification gate:
/// [`Taken`] carries the reverse-path it named, as the arrival does, under
/// `context::property::SMTP_MAIL_FROM`, and the peer.
///
/// # Errors
///
/// Where nothing connected within `timeout`, the connection failed, or a
/// command could not be answered.
pub fn accept_one(listener: &TcpListener, timeout: Option<Duration>) -> Result<Taken> {
    // The wait for the connection is bounded as well as the reads. This did
    // a bare accept until 2026-09-21, so a far end whose near end never
    // connected waited for good, and a hang has no verdict.
    let (mut stream, peer) = socket::accept_tcp(listener, timeout)?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| classify("cloning the connection", &e))?,
    );

    say(&mut stream, "220 xmip ESMTP")?;

    let (bytes, from) = converse(&mut stream, &mut reader)?;

    let taken = Taken::new(format!("smtp://{peer}"), bytes).from_peer(peer);
    Ok(match from {
        Some(from) => taken.observing(SMTP_MAIL_FROM, from),
        None => taken,
    })
}

/// Answer commands until the client leaves, the end of each `DATA` at once,
/// and hand back what it sent last, with the reverse-path it named.
fn converse(
    stream: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<(Vec<u8>, Option<String>)> {
    let mut bytes = Vec::new();
    let mut from = None;

    loop {
        match command(stream, reader)? {
            Next::Continue => (),
            Next::Mail(said) => from = Some(said),
            Next::Data(data) => {
                bytes = data;
                say(stream, ACCEPTED)?;
            }
            Next::Done => return Ok((bytes, from)),
        }
    }
}

/// Read one command and answer it, but for the end of a `DATA`, which is
/// handed back unanswered.
///
/// # Errors
/// Where the connection broke, or a `DATA` did not end.
pub fn command(stream: &mut impl Write, reader: &mut impl BufRead) -> Result<Next> {
    match next_verb(reader)? {
        Some((verb, command)) => answer(stream, reader, &verb, &command),
        None => Ok(Next::Done),
    }
}

/// The next command's verb and the command whole, or `None` where the
/// client hung up.
fn next_verb(reader: &mut impl BufRead) -> Result<Option<(String, String)>> {
    let Some(command) = net::read::line(reader)? else {
        return Ok(None);
    };

    let verb = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    Ok(Some((verb, command)))
}

/// Answer one command.
fn answer(
    stream: &mut impl Write,
    reader: &mut impl BufRead,
    verb: &str,
    command: &str,
) -> Result<Next> {
    match verb {
        "" => Ok(Next::Continue),
        "HELO" => say(stream, "250 xmip").map(|()| Next::Continue),
        "EHLO" => say(stream, "250-xmip\r\n250 8BITMIME").map(|()| Next::Continue),
        "MAIL" => say(stream, "250 ok").map(|()| Next::Mail(reverse_path(command))),
        "RCPT" | "RSET" | "NOOP" => say(stream, "250 ok").map(|()| Next::Continue),
        "DATA" => {
            say(stream, "354 end with a line containing only a period")?;
            Ok(Next::Data(read_data(reader)?))
        }
        "QUIT" => {
            // A client that hangs up without waiting for the goodbye is rude,
            // not broken. The Stream already arrived, so a failed farewell must
            // not become a failed receive.
            let _ = say(stream, "221 closing");

            Ok(Next::Done)
        }
        _ => say(stream, "502 not implemented").map(|()| Next::Continue),
    }
}

/// The reverse-path `MAIL FROM:<path> [parameters]` names, its angle
/// brackets taken off: `a@example.com`, or empty for the null path `<>`.
fn reverse_path(command: &str) -> String {
    let after = command
        .split_once(':')
        .map_or("", |(_, path)| path)
        .trim_start();
    let path = after.split_whitespace().next().unwrap_or_default();
    path.trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converse_with(script: &[u8]) -> (Vec<u8>, String) {
        let mut said = Vec::new();
        let (bytes, _) = converse(&mut said, &mut &script[..]).expect("conversing");

        (bytes, String::from_utf8_lossy(&said).to_string())
    }

    #[test]
    fn a_full_exchange_yields_the_message_body() {
        let (bytes, said) = converse_with(
            b"EHLO client\r\n\
              MAIL FROM:<a@example.com>\r\n\
              RCPT TO:<b@example.com>\r\n\
              DATA\r\n\
              Subject: one\r\n\
              .\r\n\
              QUIT\r\n",
        );

        assert_eq!(bytes, b"Subject: one");
        assert!(said.contains("354 "), "the client was told to send data");
        assert!(said.contains("221 closing"));
    }

    #[test]
    fn a_client_that_hangs_up_without_quit_still_delivered() {
        // The Stream arrived. Losing it because the farewell never came would
        // be losing an accepted Message, which ADR-0013 forbids.
        let (bytes, _) = converse_with(b"DATA\r\nSubject: one\r\n.\r\n");

        assert_eq!(bytes, b"Subject: one");
    }

    #[test]
    fn an_unknown_verb_is_refused_and_the_session_continues() {
        let (bytes, said) = converse_with(b"WIBBLE\r\nDATA\r\nx\r\n.\r\nQUIT\r\n");

        assert!(said.contains("502 not implemented"));
        assert_eq!(bytes, b"x", "the session carried on to deliver");
    }

    #[test]
    fn the_envelope_is_answered_and_its_sender_kept_beside_the_stream() {
        // ADR-0019: MAIL FROM is a passed identity for the identification
        // gate; the Stream is the content, the reverse-path travels beside it.
        let mut said = Vec::new();
        let script = b"MAIL FROM:<a@example.com> SIZE=12\r\nQUIT\r\n";
        let (bytes, from) = converse(&mut said, &mut &script[..]).expect("conversing");

        assert!(String::from_utf8_lossy(&said).contains("250 ok"));
        assert_eq!(bytes, [] as [u8; 0]);
        assert_eq!(from.as_deref(), Some("a@example.com"));
        assert_eq!(reverse_path("MAIL FROM:<>"), "");
    }
}
