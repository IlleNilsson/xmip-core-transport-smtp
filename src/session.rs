//! Saying and hearing one line at a time.
//!
//! The two mistakes this exists to stop happening twice: reading a multi-line
//! reply as one line, and getting the retryable/permanent judgement backwards.

use std::io::{BufRead, Write};

use transport::error::{Result, TransportError, classify, protocol_error};
use transport::wire::{MAX_BODY, trim_eol};

/// Write one command or reply, terminated as the protocol requires.
///
/// # Errors
///
/// Where the connection could not be written to.
pub fn say(stream: &mut impl Write, line: &str) -> Result<()> {
    stream
        .write_all(line.as_bytes())
        .map_err(|e| classify("writing a line", &e))?;

    stream
        .write_all(b"\r\n")
        .map_err(|e| classify("writing a line ending", &e))?;

    stream.flush().map_err(|e| classify("flushing a line", &e))
}

/// Read one reply, following continuation lines.
///
/// A multi-line reply is `250-first` … `250 last`. **It is the space in the
/// fourth column that ends it, not the line ending** — reading line-at-a-time
/// and stopping at the first newline is the classic way to hang an SMTP client
/// forever.
///
/// # Errors
///
/// Where the connection closed mid-reply, or a line carried no reply code.
pub fn read_reply(reader: &mut impl BufRead) -> Result<(u16, String)> {
    let mut text = String::new();

    loop {
        let line = read_line(reader)?;
        let code = reply_code(&line)?;
        let continued = line.as_bytes().get(3) == Some(&b'-');

        text.push_str(&line);

        if !continued {
            return Ok((code, text));
        }

        text.push('\n');
    }
}

fn read_line(reader: &mut impl BufRead) -> Result<String> {
    let mut raw = String::new();

    let read = reader
        .read_line(&mut raw)
        .map_err(|e| classify("reading a reply", &e))?;

    if read == 0 {
        return Err(protocol_error("a connection that closed mid-reply"));
    }

    Ok(String::from_utf8_lossy(trim_eol(raw.as_bytes())).to_string())
}

fn reply_code(line: &str) -> Result<u16> {
    line.get(..3)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| protocol_error(format!("a reply with no code: {line}")))
}

/// Read one reply and insist on a particular code.
///
/// # Errors
///
/// Where the server answered anything else. **SMTP runs the opposite way round
/// from HTTP**: 4xx is the transient failure and 5xx is the permanent one.
pub fn expect(reader: &mut impl BufRead, wanted: u16, step: &str) -> Result<()> {
    let (code, text) = read_reply(reader)?;

    if code == wanted {
        return Ok(());
    }

    Err(TransportError {
        message: format!("{step}: the server answered {text}"),
        retryable: (400..500).contains(&code),
    })
}

/// Read a DATA payload, undoing the dot-stuffing that protects a leading period.
///
/// # Errors
///
/// Where the connection closed inside DATA, or the message exceeded
/// [`MAX_BODY`].
pub fn read_data(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();

    loop {
        let mut raw = Vec::new();
        let read = reader
            .read_until(b'\n', &mut raw)
            .map_err(|e| classify("reading the message data", &e))?;

        if read == 0 {
            return Err(protocol_error("a connection that closed inside DATA"));
        }

        let line = trim_eol(&raw);

        if line == b"." {
            return Ok(bytes);
        }

        let line = unstuff(line);

        if bytes.len() + line.len() > MAX_BODY {
            return Err(protocol_error("a message over the size Xmip will read"));
        }

        if !bytes.is_empty() {
            bytes.extend_from_slice(b"\r\n");
        }

        bytes.extend_from_slice(line);
    }
}

/// A line the sender stuffed with a leading period, unstuffed.
const fn unstuff(line: &[u8]) -> &[u8] {
    if line.len() >= 2 && line[0] == b'.' && line[1] == b'.' {
        line.split_at(1).1
    } else {
        line
    }
}

/// Write one line of a message body, stuffing a leading period.
///
/// A line that starts with a period would otherwise end the message early.
///
/// # Errors
///
/// Where the connection could not be written to.
pub fn write_stuffed(stream: &mut impl Write, line: &[u8]) -> Result<()> {
    if line.starts_with(b".") {
        stream
            .write_all(b".")
            .map_err(|e| classify("writing the message data", &e))?;
    }

    stream
        .write_all(line)
        .map_err(|e| classify("writing the message data", &e))?;

    stream
        .write_all(b"\r\n")
        .map_err(|e| classify("writing the message data", &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_reply_ends_at_the_space_not_the_newline() {
        let mut reader = &b"250-xmip\r\n250-8BITMIME\r\n250 SIZE\r\n"[..];
        let (code, text) = read_reply(&mut reader).expect("read");

        assert_eq!(code, 250);
        assert_eq!(text, "250-xmip\n250-8BITMIME\n250 SIZE");
        assert!(reader.is_empty(), "the whole reply was consumed");
    }

    #[test]
    fn a_single_line_reply_is_one_line() {
        let (code, text) = read_reply(&mut &b"220 xmip ESMTP\r\n"[..]).expect("read");

        assert_eq!(code, 220);
        assert_eq!(text, "220 xmip ESMTP");
    }

    #[test]
    fn a_connection_that_closes_mid_reply_is_a_protocol_error() {
        assert!(read_reply(&mut &b""[..]).is_err());
    }

    #[test]
    fn four_hundred_is_transient_and_five_hundred_is_not() {
        // Backwards from HTTP, and getting it the HTTP way round means retrying
        // a permanent rejection until retention expires.
        let transient = expect(&mut &b"451 try later\r\n"[..], 250, "MAIL FROM").expect_err("451");
        let permanent =
            expect(&mut &b"550 no such mailbox\r\n"[..], 250, "RCPT TO").expect_err("550");

        assert!(transient.retryable);
        assert!(!permanent.retryable);
    }

    #[test]
    fn data_ends_at_a_lone_period() {
        let data = read_data(&mut &b"one\r\ntwo\r\n.\r\n"[..]).expect("read");

        assert_eq!(data, b"one\r\ntwo");
    }

    #[test]
    fn a_stuffed_period_survives_the_round_trip() {
        let mut written = Vec::new();
        write_stuffed(&mut written, b".hidden").expect("written");

        assert_eq!(written, b"..hidden\r\n");
        assert_eq!(
            read_data(&mut &b"..hidden\r\n.\r\n"[..]).expect("read"),
            b".hidden"
        );
    }

    #[test]
    fn an_ordinary_line_is_not_stuffed() {
        let mut written = Vec::new();
        write_stuffed(&mut written, b"Subject: one").expect("written");

        assert_eq!(written, b"Subject: one\r\n");
    }
}
