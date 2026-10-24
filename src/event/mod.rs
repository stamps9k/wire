//! Events: one record per connection, broadcast to every subscriber, and the
//! messages built from them for WebSocket viewers.
//!
//! An [`Event`] holds what all connections have in common, and its [`Kind`]
//! holds what depends on how the connection ended. Events are built once,
//! cloned to each subscriber and serialised to JSON. A [`ViewerMessage`] is
//! what viewers receive: either an event or a [`Summary`] of the events the
//! viewer feed dropped.

mod kind;
mod record;
mod reject_reason;
mod shed_reason;
mod tcp_snapshot;
mod viewer_event;

pub use kind::Kind;
pub use record::{CAP_SIZE, Event, truncate};
pub use reject_reason::RejectReason;
pub use shed_reason::ShedReason;
use tcp_snapshot::TcpSnapshot;
pub use viewer_event::Dropped;
pub use viewer_event::Summary;
pub use viewer_event::ViewerMessage;
