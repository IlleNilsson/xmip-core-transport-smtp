# xmip-core-transport-smtp

SMTP transport: one mail body is one Stream, the envelope is addressing. A technology of
[xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport), which
owns the direction-neutral `Transport` trait this crate implements (ADR-0010).

Lifted out of the capability crate on 2026-09-07, where it had lived as
`src/smtp` since 2026-08-27 waiting for this repository. The capability keeps
the trait, the error vocabulary and the shared wire helpers; nothing in it names
a protocol. The head a line-oriented protocol reads — lines, then a blank
line — left it on 2026-09-25 for `net::head` in
[xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net).
Each command and reply line is read there too (`net::read::line`, under its
ceiling, refused where it is not UTF-8), and a reply's code (`net::reply`);
until 2026-09-27 this crate read both itself, the code without its range.

A Receive Location keeps its listener, bound on the first receive: a peer that connects between two receives is queued and taken by the next, where until 2026-09-27 each receive bound a listener of its own and a peer between receives was refused. Since 2026-10-02 a client's connection is kept too (`transport::serving::Serving`), so a client that sends several messages in one session has each taken by a receive.

## Acknowledgement

A message is taken only after the runtime's whole receive cycle. The client waits for the reply to the end of its `DATA`, and its connection takes no next command meanwhile: `Accepted` answers `250`. `Refused` answers a permanent reply the client does not retry, among those RFC 5321 section 4.3.2 allows after `DATA`: `550 5.7.1` (RFC 3463, delivery not authorized) for a sender not identified or not permitted, `554 5.6.0` for content refused. `Failed` answers `451` — a temporary failure, RFC 5321 section 4.2.5, so the client keeps the message and sends it again (`receiving::reply`). A message dropped without a verdict shuts the connection unanswered (`transport::answer::Answer`), which a client also retries. `DATA` is read whole, dot-unstuffed by `transport::stuffed`, so the body is whole in memory. Until 2026-10-02 the end of `DATA` was answered `250` as soon as it was read.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
