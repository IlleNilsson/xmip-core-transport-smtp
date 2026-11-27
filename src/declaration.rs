//! This technology's declaration: the transport guide's "The declaration",
//! `module/core/capability/transport/doc/adding-a-transport.md`.

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

/// This technology's schemes, as [`transport::Configured::SCHEMES`] defines
/// them.
pub const SCHEMES: &[&str] = &["smtp"];
