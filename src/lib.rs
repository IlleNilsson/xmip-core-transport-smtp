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
use std::time::Duration;

use transport::Arrived;
use transport::Directions;
use transport::Transport;
use transport::error::Result;
use transport::listening::{Accepting, Listening};
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::socket;

pub struct SmtpTransport {
    bind: String,
    relay: String,
    from: String,
    timeout: Option<Duration>,
}

impl SmtpTransport {
    /// A receiving transport. There is nothing to relay through.
    #[must_use]
    pub fn receiving(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            relay: String::new(),
            from: String::new(),
            timeout: None,
        }
    }

    /// A sending transport, relaying through one server as one sender.
    #[must_use]
    pub fn sending(relay: impl Into<String>, from: impl Into<String>) -> Self {
        Self {
            bind: String::new(),
            relay: relay.into(),
            from: from.into(),
            timeout: None,
        }
    }

    /// Give up on a connection that does not arrive, or stops sending, as
    /// `TcpTransport` does.
    #[must_use]
    pub const fn timing_out_after(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
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
        server::accept_one(listener, self.timeout)
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
        client::relay(&self.relay, &self.from, target, bytes, self.timeout)
    }
}

impl SmtpTransport {
    /// Both ends on this machine: a receiver on an ephemeral local port, and
    /// a sender relaying one message through it, the loopback timeout on
    /// the accept, the connect and the reads. The session ends when the
    /// sender says QUIT or hangs up; the timeout is for when it does neither.
    #[must_use]
    pub fn loopback() -> Self {
        Self::receiving("127.0.0.1:0").timing_out_after(LOOPBACK_TIMEOUT)
    }
}

/// What the far end does with its one session: the receiver reads nothing
/// of the instance but its timeout, so the timeout stands in for it.
struct Receiving(Option<Duration>);

impl Accepting for Receiving {
    fn take_one(&self, listener: &TcpListener) -> Result<Arrived> {
        server::accept_one(listener, self.0)
    }
}

impl Loopback for SmtpTransport {
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let (listener, address) = self.bind()?;
        Ok(Box::new(Listening::new(
            Receiving(self.timeout),
            listener,
            address,
        )))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        Self::sending(address, "xmip@example.com")
            .timing_out_after(LOOPBACK_TIMEOUT)
            .send("mailto:round-trip@example.com", payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport::payload::edge_payloads;

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
