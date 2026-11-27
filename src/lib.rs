#![forbid(unsafe_code)]

//! Streams that arrive as mail. One message body is one Stream.
//!
//! **The envelope is addressing, not content.** The recipient is Send Location
//! configuration, its `to`, and the relay is where Xmip hands the message
//! over, its address. That split is why a send's target is a mailbox and not
//! a host: the Location's own target names none (`transport::destination`),
//! and its `to` is the mailbox; a target naming another still wins.
//!
//! ```text
//! session.rs    saying and hearing one line at a time
//! server.rs     answering a client's commands
//! receiving.rs  a Receive Location's clients, answered after the cycle
//! client.rs     relaying messages on a session kept with the relay
//! ```
//!
//! **Both ends keep the session.** A Receive Location keeps its clients'
//! connections ([`receiving`]); a Send Location keeps its session with the
//! relay in the capability's pool (`transport::Pool`), one mail transaction
//! a message ([`client::Session`]). Until 2026-10-21 every message sent
//! connected, greeted and said `QUIT` of its own.
//!
//! **A message is taken only after the runtime's whole receive cycle.** The
//! client waits for the reply to the end of its `DATA`: `250` on
//! `Accepted`, `451` on `Refused` — a temporary failure, so it keeps the
//! message and sends it again.
//!
//! **Built with its `declaration` feature alone** the crate is
//! [`declaration`].

#[cfg(feature = "default")]
pub mod client;
#[cfg(feature = "declaration")]
pub mod declaration;
#[cfg(feature = "default")]
pub mod receiving;
#[cfg(feature = "default")]
pub mod server;
#[cfg(feature = "default")]
pub mod session;

#[cfg(feature = "default")]
use std::net::TcpListener;
#[cfg(feature = "default")]
use std::time::Duration;

#[cfg(feature = "default")]
use context::property::{PEER_ADDRESS, SMTP_MAIL_FROM};
#[cfg(feature = "default")]
use net::authority::with_default_port;
#[cfg(feature = "default")]
use transport::error::Result;
#[cfg(feature = "default")]
use transport::listening::Listening;
#[cfg(feature = "default")]
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
#[cfg(feature = "default")]
use transport::serving::Serving;
#[cfg(feature = "default")]
use transport::{ArrivalIdentity, Arrived, Configured, Directions, Pool, Transport};
#[cfg(feature = "default")]
use transport::{destination, socket};
#[cfg(feature = "default")]
use xcore::settings::{Read, Settings};

#[cfg(feature = "default")]
use crate::client::{Recipient, Session};
#[cfg(feature = "default")]
use crate::receiving::Client;

#[cfg(feature = "default")]
pub struct SmtpTransport {
    bind: String,
    relay: String,
    from: String,
    /// The mailbox a send goes to where its target names none: a Send
    /// Location's `to`.
    to: String,
    timeout: Option<Duration>,
    /// The listener its open binds, and the clients' connections
    /// kept open on it between messages.
    serving: Serving<Client>,
    /// The session kept with the relay between messages.
    sessions: Pool<Session>,
}

#[cfg(feature = "default")]
impl SmtpTransport {
    /// A receiving transport. There is nothing to relay through.
    #[must_use]
    pub fn receiving(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            relay: String::new(),
            from: String::new(),
            to: String::new(),
            timeout: None,
            serving: Serving::new(),
            sessions: Pool::new(),
        }
    }

    /// A sending transport, relaying through one server as one sender.
    #[must_use]
    pub fn sending(relay: impl Into<String>, from: impl Into<String>) -> Self {
        Self {
            bind: String::new(),
            relay: relay.into(),
            from: from.into(),
            to: String::new(),
            timeout: None,
            serving: Serving::new(),
            sessions: Pool::new(),
        }
    }

    /// This sender's mail going to `mailbox` where a send names none, as a
    /// Send Location's `to` says (`transport::destination`).
    #[must_use]
    pub fn addressed_to(mut self, mailbox: impl Into<String>) -> Self {
        self.to = mailbox.into();
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

#[cfg(feature = "default")]
impl Transport for SmtpTransport {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    /// The callers turned away at their own connection (`transport::serving`).
    fn turned_away(&self) -> transport::Turned {
        self.serving.turned_away()
    }

    fn arrivals(&self) -> transport::Arrivals {
        transport::answer::ONE_AT_A_TIME
    }

    fn waiting(&self) -> transport::Waiting {
        transport::Waiting::Waits("the accept waits for a client and its message")
    }

    /// One message, from a new client on the listener its open
    /// bound and kept, or the next from a client kept connected. Its
    /// client waits for the reply to the end of its `DATA` until the
    /// verdict: `250` on `Accepted`, `451` on `Refused`
    /// ([`receiving`]).
    fn receive_up_to(&self, _most: usize) -> Result<Vec<Arrived>> {
        let arrived = self
            .serving
            .next(self.timeout, Client::greeted, Client::turn)?;
        Ok(vec![arrived])
    }

    fn sending(&self) -> transport::Sending {
        transport::Sending::Gathered("the DATA block is dot-stuffed whole")
    }

    /// One mail transaction on the session kept with the relay, or a new
    /// one greeted for it, to the mailbox `target` names, or the Location's
    /// `to` where it names none. A refusal the relay answered fails the
    /// send — retryable on a `4xx`, permanent on a `5xx` — and keeps the
    /// session.
    fn send_from(&self, target: &str, outgoing: transport::Outgoing<'_>) -> Result<()> {
        let bytes: &[u8] = &outgoing.gather()?;
        let recipient = Recipient::parse(destination::named(target).unwrap_or(&self.to))?;
        let address = with_default_port(&self.relay, 25);
        self.sessions.exchange(
            address.as_str(),
            || Session::open(&address, self.timeout),
            |session| session.deliver(&self.from, &recipient, bytes),
        )?
    }
}

#[cfg(feature = "default")]
impl Configured for SmtpTransport {
    const SETTINGS: &'static Settings = declaration::SETTINGS;
    const SCHEMES: &'static [&'static str] = declaration::SCHEMES;

    /// The `to` setting: a Send Location's mail goes to the mailbox it
    /// names, through the relay its address names (`transport::destination`).
    fn own_target(_address: &str) -> &str {
        destination::OWN
    }

    fn timeout_kept(&mut self) -> Option<&mut dyn transport::configured::TimeoutKept> {
        Some(&mut self.timeout)
    }

    /// A Send Location is the one with a sender, which its declaration makes
    /// required there and refuses on a Receive Location.
    fn configured(address: &str, settings: &Read) -> Result<Self> {
        Ok(match settings.optional_text("from") {
            Some(from) => Self::sending(address, from).addressed_to(settings.text("to")),
            None => Self::receiving(address),
        })
    }

    /// Each client's turn on a Receive Location taking at most `turn`: one
    /// command, or one message's `DATA` to its end.
    fn turning_within(mut self, turn: Duration) -> Self {
        self.serving = self.serving.turning_within(turn);
        self
    }

    /// Each callers turn on a Receive Location counted on clock.
    fn timed_by(mut self, clock: &std::sync::Arc<dyn xcore::Clock>) -> Self {
        self.serving = self.serving.timed_by(clock);
        self
    }

    /// The listener every receive takes from, bound where a Receive
    /// Location listens for clients.
    fn listening(&self) -> Result<Option<&str>> {
        self.serving.bound(|| self.bind()).map(Some)
    }
}

#[cfg(feature = "default")]
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

#[cfg(feature = "default")]
impl Loopback for SmtpTransport {
    fn arrival_identity(&self) -> ArrivalIdentity {
        ArrivalIdentity::Named(&[PEER_ADDRESS, SMTP_MAIL_FROM])
    }

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
    use xcore::settings::Applies;

    #[test]
    fn smtp_declares_its_settings_and_reads_through_them() {
        use xcore::settings::Given;
        assert_eq!(SmtpTransport::SETTINGS.problems(), Vec::<String>::new());
        let given = [
            (
                "from".to_string(),
                Given::Text("xmip@example.com".to_string()),
            ),
            (
                "to".to_string(),
                Given::Text("orders@example.com".to_string()),
            ),
            ("timeout".to_string(), Given::Text("2s".to_string())),
        ];
        let sending = SmtpTransport::open("relay:25", Applies::Send, &given).expect("sending");
        assert_eq!(
            (
                sending.relay.as_str(),
                sending.from.as_str(),
                sending.to.as_str()
            ),
            ("relay:25", "xmip@example.com", "orders@example.com")
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

    /// A Send Location as a node builds it, from its address and settings,
    /// sends where the node sends it: its own target, the mailbox its `to`
    /// names (the owner, 2026-11-24: *go through each item and sort it*).
    /// Until 2026-11-24 its target was the relay's address, and every send
    /// was refused as no mailbox.
    #[test]
    fn a_send_location_sends_to_the_mailbox_its_to_names() {
        use xcore::settings::Given;
        let far = SmtpTransport::loopback().far_end().expect("a far end");
        let address = format!("smtp://{}", far.address());
        let given = [("from", "xmip@example.com"), ("to", "orders@example.com")]
            .map(|(name, value)| (name.to_string(), Given::Text(value.to_string())));
        let sending = SmtpTransport::open(&address, Applies::Send, &given)
            .expect("sending")
            .timing_out_after(LOOPBACK_TIMEOUT);
        let taking = std::thread::spawn(move || far.take_one());

        let sent = sending.send(SmtpTransport::own_target(&address), b"Subject: order");
        // The far end hands its message over at the session's QUIT, which
        // the kept session says as it is let go.
        drop(sending);

        let taken = taking.join().expect("the far end");
        sent.expect("sent to the mailbox its to names");
        assert_eq!(taken.expect("taken").bytes, b"Subject: order");
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
    fn a_hundred_messages_through_one_relay_are_one_session_and_every_receive_takes_from_it() {
        // Until 2026-10-21 every message connected, greeted, said EHLO and
        // QUIT of its own.
        const ROUNDS: usize = 100;
        let receiver = SmtpTransport::loopback();
        let address = receiver
            .serving
            .bound(|| receiver.bind())
            .expect("bound")
            .to_string();
        let sender = std::thread::spawn(move || {
            let sending = SmtpTransport::sending(address, "xmip@example.com")
                .timing_out_after(LOOPBACK_TIMEOUT);
            for round in 0..ROUNDS {
                let body = format!("Subject: {round}");
                sending
                    .send("mailto:a@example.com", body.as_bytes())
                    .expect("a 250");
            }
            sending.sessions.opened()
        });
        let mut origins = std::collections::BTreeSet::new();
        for round in 0..ROUNDS {
            let arrived =
                transport::arrived::one_arrival(receiver.receive().expect("received"), "one")
                    .expect("one");
            let taken = arrived.taken().expect("accepted");
            assert_eq!(taken.bytes, format!("Subject: {round}").as_bytes());
            origins.insert(taken.origin_uri);
        }
        assert_eq!(sender.join().expect("sender"), 1, "one session");
        assert_eq!(origins.len(), 1, "every message on the kept connection");
        assert_eq!(receiver.serving.open(), 1);
    }

    #[test]
    fn a_transaction_refused_before_its_end_is_reset_and_the_session_carries_the_next() {
        use std::io::{BufRead, Write};
        let (listener, address) = socket::bind_tcp("127.0.0.1:0").expect("bound");
        // A relay that refuses the first recipient: what it hears, in order.
        let relay = std::thread::spawn(move || {
            let (stream, _) = socket::accept_tcp(&listener, Some(LOOPBACK_TIMEOUT)).expect("in");
            let (mut reader, mut writer) = socket::split(stream).expect("split");
            let mut heard = Vec::new();
            writer.write_all(b"220 relay\r\n").expect("greeted");
            for answer in [
                "250 relay",
                "250 ok",
                "550 no such user",
                "250 reset",
                "250 ok",
            ]
            .into_iter()
            .chain(["250 ok", "354 go on"])
            {
                let mut line = String::new();
                reader.read_line(&mut line).expect("a command");
                heard.push(
                    line.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
                writer
                    .write_all(format!("{answer}\r\n").as_bytes())
                    .expect("answered");
            }
            let data = session::read_data(&mut reader).expect("the data");
            writer.write_all(b"250 taken\r\n").expect("taken");
            let mut line = String::new();
            reader.read_line(&mut line).expect("goodbye");
            heard.push(line.trim_end().to_string());
            (heard, data)
        });
        let sending =
            SmtpTransport::sending(address, "xmip@example.com").timing_out_after(LOOPBACK_TIMEOUT);
        let refused = sending
            .send("mailto:nobody@example.com", b"first")
            .expect_err("a 550");
        assert!(!refused.retryable, "{refused}");
        sending
            .send("mailto:a@example.com", b"second")
            .expect("a 250");
        assert_eq!(sending.sessions.opened(), 1);
        drop(sending);
        let (heard, data) = relay.join().expect("relay");
        assert_eq!(
            heard,
            [
                "EHLO", "MAIL", "RCPT", "RSET", "MAIL", "RCPT", "DATA", "QUIT"
            ]
        );
        assert_eq!(data, b"second");
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
    fn a_mailbox_has_no_artifact_to_claim_on_the_sending_side() {
        assert!(
            SmtpTransport::sending("relay:25", "x@example.com")
                .claims()
                .is_none()
        );
    }
}
