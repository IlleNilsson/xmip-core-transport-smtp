#![forbid(unsafe_code)]

//! Streams that arrive as mail. One message body is one Stream.
//!
//! **The envelope is addressing, not content.** The recipient is Send Location
//! configuration and the relay is where Xmip hands the message over. That split
//! is why `send` takes a mailbox as its target and not a host.
//!
//! ```text
//! session.rs    saying and hearing one line at a time
//! server.rs     answering a client's commands
//! receiving.rs  a Receive Location's clients, answered after the cycle
//! client.rs     relaying one message
//! ```
//!
//! **A message is taken only after the runtime's whole receive cycle.** The
//! client waits for the reply to the end of its `DATA`: `250` on
//! `Accepted`, `451` on `Refused` — a temporary failure, so it keeps the
//! message and sends it again.

pub mod client;
pub mod receiving;
pub mod server;
pub mod session;

use std::net::TcpListener;
use std::time::Duration;

use transport::Arrived;
use transport::Configured;
use transport::Directions;
use transport::Transport;
use transport::error::Result;
use transport::listening::Listening;
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::serving::Serving;
use transport::socket;
use xcore::settings::{Applies, Kind, Presence, Read, Setting, Settings};

use crate::receiving::Client;

pub struct SmtpTransport {
    bind: String,
    relay: String,
    from: String,
    timeout: Option<Duration>,
    /// The listener the first receive binds, and the clients' connections
    /// kept open on it between messages.
    serving: Serving<Client>,
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
            serving: Serving::new(),
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
            serving: Serving::new(),
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
}

impl Transport for SmtpTransport {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn arrivals(&self) -> transport::Arrivals {
        transport::Arrivals::Unordered(
            "each request is its own, and a connection waiting for its answer takes no next request",
        )
    }

    /// One message, from a new client on the listener the first receive
    /// bound and kept, or the next from a client kept connected. Its
    /// client waits for the reply to the end of its `DATA` until the
    /// verdict: `250` on `Accepted`, `451` on `Refused`
    /// ([`receiving`]).
    fn receive(&self) -> Result<Vec<Arrived>> {
        let arrived =
            self.serving
                .next(|| self.bind(), self.timeout, Client::greeted, Client::turn)?;
        Ok(vec![arrived])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        client::relay(&self.relay, &self.from, target, bytes, self.timeout)
    }
}

impl Configured for SmtpTransport {
    /// The address is where a Receive Location listens, and the relay a Send
    /// Location hands its mail to; a send's target is the recipient's
    /// mailbox.
    const SETTINGS: &'static Settings = &Settings {
        technology: env!("CARGO_PKG_NAME"),
        settings: &[
            Setting {
                name: "from",
                kind: Kind::Text,
                presence: Presence::Required,
                meaning: "The mailbox a Send Location's mail is from.",
                applies: Applies::Send,
            },
            Setting {
                name: "timeout",
                kind: Kind::Duration,
                presence: Presence::Optional,
                meaning: "How long a connection is waited for, and how long one that stops \
                          sending is waited on; unbounded when left out.",
                applies: Applies::Both,
            },
        ],
    };

    /// A Send Location is the one with a sender, which its declaration makes
    /// required there and refuses on a Receive Location.
    fn configured(address: &str, settings: &Read) -> Result<Self> {
        let transport = match settings.optional_text("from") {
            Some(from) => Self::sending(address, from),
            None => Self::receiving(address),
        };
        Ok(match settings.optional_duration("timeout") {
            Some(timeout) => transport.timing_out_after(timeout),
            None => transport,
        })
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

impl Loopback for SmtpTransport {
    /// The far end's one session: the receiver reads nothing of the
    /// instance but its timeout, so the timeout stands in for it.
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let timeout = self.timeout;
        Ok(Box::new(Listening::new(
            move |listener: &TcpListener| server::accept_one(listener, timeout),
            self.bind()?,
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
    fn smtp_declares_its_settings_and_reads_through_them() {
        use xcore::settings::Given;
        assert_eq!(SmtpTransport::SETTINGS.problems(), Vec::<String>::new());
        let given = [
            (
                "from".to_string(),
                Given::Text("xmip@example.com".to_string()),
            ),
            ("timeout".to_string(), Given::Text("2s".to_string())),
        ];
        let sending = SmtpTransport::open("relay:25", Applies::Send, &given).expect("sending");
        assert_eq!(
            (sending.relay.as_str(), sending.from.as_str()),
            ("relay:25", "xmip@example.com")
        );
        assert_eq!(sending.timeout, Some(Duration::from_secs(2)));
        let receiving =
            SmtpTransport::open("0.0.0.0:25", Applies::Receive, &[]).expect("receiving");
        assert_eq!(receiving.bind, "0.0.0.0:25");
        let Err(refused) = SmtpTransport::open("relay:25", Applies::Send, &[]) else {
            panic!("a Send Location's sender is required");
        };
        assert!(refused.message.contains("\"from\""), "{refused}");
    }

    #[test]
    fn every_receive_takes_from_the_listener_the_first_bound() {
        let receiver = SmtpTransport::loopback();
        let address = receiver
            .serving
            .bound(|| receiver.bind())
            .expect("bound")
            .to_string();
        transport::kept::held_across_receives(&receiver, &address, 5, |at, payload| {
            SmtpTransport::loopback().send_to(at, payload)
        });
    }

    #[test]
    fn the_client_is_answered_250_on_accepted_451_on_failed_and_5xx_on_refused() {
        // Short: the last receive only answers the client's QUIT, then
        // finds nothing more.
        let receiver =
            SmtpTransport::receiving("127.0.0.1:0").timing_out_after(Duration::from_millis(500));
        let address = receiver
            .serving
            .bound(|| receiver.bind())
            .expect("bound")
            .to_string();
        let sender = std::thread::spawn(move || {
            let sending = SmtpTransport::sending(address, "xmip@example.com")
                .timing_out_after(LOOPBACK_TIMEOUT);
            let failed = sending.send("mailto:a@example.com", b"Subject: first");
            let accepted = sending.send("mailto:a@example.com", b"Subject: again");
            let refused = sending.send("mailto:a@example.com", b"Subject: refused");
            (failed, accepted, refused)
        });
        let first = transport::arrived::one_arrival(receiver.receive().expect("received"), "one")
            .expect("one");
        assert!(first.defers());
        first.failed().expect("failed");
        let again = transport::arrived::one_arrival(receiver.receive().expect("received"), "one")
            .expect("one")
            .taken()
            .expect("accepted");
        assert_eq!(again.bytes, b"Subject: again");
        transport::arrived::one_arrival(receiver.receive().expect("received"), "one")
            .expect("one")
            .refused(transport::Refusal::Unacceptable)
            .expect("refused");
        // The client waits for the reply to its QUIT, which the next
        // receive answers.
        assert!(receiver.receive().is_err(), "nothing more was sent");
        let (failed, accepted, refused) = sender.join().expect("sender");
        let failed = failed.expect_err("a 451");
        assert!(failed.retryable, "a 451 is temporary: {failed}");
        assert!(failed.message.contains("451"), "{failed}");
        accepted.expect("a 250");
        let refused = refused.expect_err("a 554");
        assert!(!refused.retryable, "a 554 is permanent: {refused}");
        assert!(refused.message.contains("554"), "{refused}");
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
