#![forbid(unsafe_code)]

//! Streams that arrive as mail. One message body is one Stream.
//!
//! **The envelope is addressing, not content.** The recipient is Send Location
//! configuration and the relay is where Xmip hands the message over. That split
//! is why `send` takes a mailbox as its target and not a host.
//!
//! ```text
//! session.rs  saying and hearing one line at a time
//! server.rs   accepting one message
//! client.rs   relaying one message
//! ```

pub mod client;
pub mod server;
pub mod session;

use std::net::TcpListener;

use transport::Arrived;
use transport::Directions;
use transport::Transport;
use transport::error::Result;
use transport::socket;

pub struct SmtpTransport {
    bind: String,
    relay: String,
    from: String,
}

impl SmtpTransport {
    /// A receiving transport. There is nothing to relay through.
    #[must_use]
    pub fn receiving(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            relay: String::new(),
            from: String::new(),
        }
    }

    /// A sending transport, relaying through one server as one sender.
    #[must_use]
    pub fn sending(relay: impl Into<String>, from: impl Into<String>) -> Self {
        Self {
            bind: String::new(),
            relay: relay.into(),
            from: from.into(),
        }
    }

    /// Bind and report the address actually assigned.
    ///
    /// # Errors
    ///
    /// Where the address is taken, malformed, or not permitted.
    pub fn bind(&self) -> Result<(TcpListener, String)> {
        socket::bind_tcp(&self.bind)
    }

    /// Take one message from an already-bound listener.
    ///
    /// # Errors
    ///
    /// As [`server::accept_one`].
    pub fn accept_one(&self, listener: &TcpListener) -> Result<Arrived> {
        server::accept_one(listener)
    }
}

impl Transport for SmtpTransport {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let (listener, _) = self.bind()?;

        Ok(vec![self.accept_one(&listener)?])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        client::relay(&self.relay, &self.from, target, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_round_trip_survives_a_leading_period() {
        let receiver = SmtpTransport::receiving("127.0.0.1:0");
        let (listener, address) = receiver.bind().expect("binding");

        let sender = std::thread::spawn(move || {
            // The third line starts with a period, which is the one byte
            // sequence that can end a message early if it is not stuffed.
            SmtpTransport::sending(address, "xmip@example.com")
                .send("mailto:orders@example.com", b"Subject: one\r\n\r\n.hidden")
                .expect("sending");
        });

        let arrived = receiver.accept_one(&listener).expect("accepting");
        sender.join().expect("the sending thread panicked");

        assert_eq!(arrived.bytes, b"Subject: one\r\n\r\n.hidden");
        assert!(arrived.origin_uri.starts_with("smtp://127.0.0.1:"));
    }

    #[test]
    fn an_smtp_target_that_is_not_a_mailbox_is_rejected() {
        let failure = SmtpTransport::sending("127.0.0.1:25", "xmip@example.com")
            .send("127.0.0.1:25", b"")
            .expect_err("not a mailbox");

        assert!(!failure.retryable);
    }

    #[test]
    fn a_mailbox_has_no_artefact_to_claim_on_the_sending_side() {
        assert!(
            SmtpTransport::sending("relay:25", "x@example.com")
                .claims()
                .is_none()
        );
    }
}
