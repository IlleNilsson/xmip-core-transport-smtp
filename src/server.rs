//! Accepting one message. Enough of RFC 5321 and no more.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use transport::Arrived;
use transport::error::{Result, classify};
use transport::wire::trim_eol;

use super::session::{read_data, say};

/// What the command loop decided to do next.
enum Next {
    /// Keep reading commands.
    Continue,
    /// The client said goodbye, or hung up.
    Done,
}

/// Take one message from an already-bound listener.
///
/// `MAIL FROM` is a *passed* identity and belongs at the identification gate, so
/// this answers it and keeps nothing. [`Arrived`] carries where the bytes came
/// from, and the envelope is not that.
///
/// # Errors
///
/// Where the connection failed, or a command could not be answered.
pub fn accept_one(listener: &TcpListener) -> Result<Arrived> {
    let (mut stream, peer) = listener
        .accept()
        .map_err(|e| classify("accepting a connection", &e))?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| classify("cloning the connection", &e))?,
    );

    say(&mut stream, "220 xmip ESMTP")?;

    let bytes = converse(&mut stream, &mut reader)?;

    Ok(Arrived::new(format!("smtp://{peer}"), bytes))
}

/// Answer commands until the client leaves, and hand back what it sent.
fn converse(stream: &mut impl Write, reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();

    loop {
        let Some(verb) = next_verb(reader)? else {
            return Ok(bytes);
        };

        match answer(stream, reader, &verb, &mut bytes)? {
            Next::Continue => (),
            Next::Done => return Ok(bytes),
        }
    }
}

/// The next command's verb, or `None` where the client hung up.
fn next_verb(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut raw = String::new();

    let read = reader
        .read_line(&mut raw)
        .map_err(|e| classify("reading a command", &e))?;

    if read == 0 {
        return Ok(None);
    }

    let command = String::from_utf8_lossy(trim_eol(raw.as_bytes())).to_string();

    Ok(Some(
        command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase(),
    ))
}

/// Answer one command.
fn answer(
    stream: &mut impl Write,
    reader: &mut impl BufRead,
    verb: &str,
    bytes: &mut Vec<u8>,
) -> Result<Next> {
    match verb {
        "" => Ok(Next::Continue),
        "HELO" => say(stream, "250 xmip").map(|()| Next::Continue),
        "EHLO" => say(stream, "250-xmip\r\n250 8BITMIME").map(|()| Next::Continue),
        "MAIL" | "RCPT" | "RSET" | "NOOP" => say(stream, "250 ok").map(|()| Next::Continue),
        "DATA" => {
            say(stream, "354 end with a line containing only a period")?;
            *bytes = read_data(reader)?;
            say(stream, "250 accepted")?;

            Ok(Next::Continue)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn converse_with(script: &[u8]) -> (Vec<u8>, String) {
        let mut said = Vec::new();
        let bytes = converse(&mut said, &mut &script[..]).expect("conversing");

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
    fn the_envelope_is_answered_and_not_kept() {
        // ADR-0019: MAIL FROM is a passed identity for the identification gate,
        // and Arrived carries origin rather than envelope.
        let (bytes, said) = converse_with(b"MAIL FROM:<a@example.com>\r\nQUIT\r\n");

        assert!(said.contains("250 ok"));
        assert!(bytes.is_empty());
    }
}
