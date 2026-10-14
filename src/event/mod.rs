//! Events: one record per connection, broadcast to every subscriber.
//!
//! An [`Event`] holds what all connections have in common, and its [`Kind`]
//! holds what depends on how the connection ended. Events are built once,
//! cloned to each subscriber and serialised to JSON for the browser.

mod kind;
mod record;
mod reject_reason;
mod tcp_snapshot;

pub use kind::Kind;
pub use record::{CAP_SIZE, Event, truncate};
pub use reject_reason::RejectReason;
use tcp_snapshot::TcpSnapshot;
