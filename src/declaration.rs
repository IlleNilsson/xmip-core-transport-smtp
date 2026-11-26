//! What a Location of this technology is configured with: its declaration,
//! which `Configured` reads, built with the crate's `declaration` feature
//! alone and the code behind the default (ADR-0064, amendments 2026-10-22
//! and 2026-11-26), for the library the surfaces load to carry (open
//! problem 48).

use xcore::settings::{Applies, Kind, Presence, Setting, Settings};

/// Every setting a Location takes beyond its address: where a Receive
/// Location listens, and the relay a Send Location hands its mail to; a
/// send's target is the recipient's mailbox.
pub const SETTINGS: &Settings = &Settings {
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
            name: "to",
            kind: Kind::Text,
            presence: Presence::Required,
            meaning: "The mailbox a Send Location's mail is sent to.",
            applies: Applies::Send,
        },
        transport::configured::timeout(
            Applies::Both,
            None,
            "How long a connection is waited for, and how long one that stops \
             sending is waited on; unbounded when left out.",
        ),
    ],
};

/// The schemes a Location's address is entered with to choose this technology.
pub const SCHEMES: &[&str] = &["smtp"];
