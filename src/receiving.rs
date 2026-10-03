//! A Receive Location's clients, each answered after the receive cycle.
//!
//! A client's connection is kept beside the listener
//! (`transport::serving::Serving`), and every receive takes one message
//! from whichever client finished a `DATA` first. The end of that `DATA`
//! is not answered as it is read: the client waits, and its connection
//! takes no next command, until the runtime gives the message its verdict
//! ([`reply`]) — `250` on `Accepted`; on `Refused` a permanent `5xx` the
//! client does not retry; `451` on `Failed`, a temporary failure the client
//! retries (RFC 5321 section 4.2.5). Every other command is answered as it
//! comes.

use std::io::BufReader;
use std::net::{SocketAddr, TcpStream};

use transport::answer::{Answer, Busy};
use transport::error::{Result, classify};
use transport::serving::{Open, Turn};
use transport::{Acknowledgement, Arrived, Refusal, Verdict};

use crate::server::{self, ACCEPTED, Next};
use crate::session::say;

/// The reply to the end of `DATA` where Xmip could not take the message:
/// a temporary failure (RFC 5321 section 4.2.5), sent again later.
pub const FAILED: &str = "451 4.3.0 not taken now, send it again later";

/// The reply to the end of `DATA` a verdict earns. `Refused` is permanent,
/// among the replies RFC 5321 section 4.3.2 allows there: `550` with the
/// enhanced code `5.7.1`, *delivery not authorized, message refused* (RFC
/// 3463 section 3.8), for a sender not identified or not permitted; `554`
/// with `5.6.0`, *other or undefined media error* (section 3.7), for
/// content refused.
#[must_use]
pub const fn reply(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Accepted => ACCEPTED,
        Verdict::Refused(Refusal::Unidentified | Refusal::Forbidden) => {
            "550 5.7.1 not permitted, message refused"
        }
        Verdict::Refused(Refusal::Unacceptable) => "554 5.6.0 content refused",
        Verdict::Failed => FAILED,
    }
}

/// One client's connection, kept between its messages.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    peer: SocketAddr,
    /// A message taken and its `DATA` not yet answered.
    busy: Busy,
}

impl Client {
    /// A client's new connection, greeted.
    ///
    /// # Errors
    /// Where the greeting could not be written.
    pub fn greeted(stream: TcpStream, peer: SocketAddr) -> Result<Self> {
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| classify("cloning the connection", &e))?,
        );
        let mut client = Self {
            reader,
            writer: stream,
            peer,
            busy: Busy::new(),
        };
        say(&mut client.writer, "220 xmip ESMTP")?;
        Ok(client)
    }

    /// One command answered, or a message whose `DATA` waits for its
    /// verdict.
    ///
    /// # Errors
    /// Where the connection broke, or a `DATA` did not end.
    pub fn turn(&mut self) -> Result<Turn<Arrived>> {
        match server::command(&mut self.writer, &mut self.reader)? {
            Next::Continue => Ok(Turn::Nothing),
            Next::Done => Ok(Turn::Closed),
            Next::Data(bytes) => {
                // Dropped unanswered, the answer shuts the connection, which
                // the client retries.
                let answer = Answer::held(&self.writer)?.busy(&self.busy);
                let acknowledgement = Acknowledgement::deferred(move |verdict| {
                    answer.with(|writer| say(writer, reply(verdict)))
                });
                Ok(Turn::Taken(Arrived::whole(
                    format!("smtp://{}", self.peer),
                    bytes,
                    acknowledgement,
                )))
            }
        }
    }
}

impl Open for Client {
    fn socket(&self) -> &TcpStream {
        &self.writer
    }

    /// A pipelined command already read waits in the reader.
    fn waiting(&self) -> bool {
        !self.reader.buffer().is_empty()
    }

    fn busy(&self) -> bool {
        self.busy.is_busy()
    }
}
