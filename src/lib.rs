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
use transport::loopback::{FarEnd, Loopback};
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

impl SmtpTransport {
    /// Both ends on this machine: a receiver on an ephemeral local port, and
    /// a sender relaying one message through it. The session has no timeout
    /// to set: it ends when the sender says QUIT or hangs up.
    #[must_use]
    pub fn loopback() -> Self {
        Self::receiving("127.0.0.1:0")
    }
}

/// A bound receiver waiting for its one session.
struct Listening {
    listener: TcpListener,
    address: String,
}

impl FarEnd for Listening {
    fn address(&self) -> &str {
        &self.address
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        server::accept_one(&self.listener)
    }
}

impl Loopback for SmtpTransport {
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let (listener, address) = self.bind()?;
        Ok(Box::new(Listening { listener, address }))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        Self::sending(address, "xmip@example.com").send("mailto:pingpong@example.com", payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes a transport is most likely to change: nothing, one byte,
    /// every byte value, a run of NULs, high bytes, and line endings alone.
    fn edge_payloads() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("empty", Vec::new()),
            ("one byte", vec![0x2a]),
            ("every byte", (0..=255).collect()),
            ("nul run", vec![0; 512]),
            ("high bytes", vec![0xff; 512]),
            ("crlf storm", b"\r\n".repeat(400)),
        ]
    }

    #[test]
    fn smtp_round_trip_survives_a_leading_period() {
        // The third line starts with a period, which is the one byte sequence
        // that can end a message early if it is not stuffed.
        let arrived = SmtpTransport::loopback()
            .round(b"Subject: one\r\n\r\n.hidden")
            .expect("round");

        assert_eq!(arrived.bytes, b"Subject: one\r\n\r\n.hidden");
        assert!(arrived.origin_uri.starts_with("smtp://127.0.0.1:"));
    }

    #[test]
    fn the_loopback_returns_the_edge_payloads_whole() {
        let smtp = SmtpTransport::loopback();
        assert!(smtp.ceiling().is_none());
        for (name, bytes) in edge_payloads() {
            assert!(smtp.refuses(&bytes).is_none(), "{name}");
            assert_eq!(smtp.round(&bytes).expect(name).bytes, bytes, "{name}");
        }
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
