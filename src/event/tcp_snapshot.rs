//! `TCP_INFO` readings for a connection.

use serde::Serialize;

/// The kernel's `TCP_INFO` for one connection at one moment.
///
/// A placeholder with no fields yet. The readings (round-trip time,
/// congestion window, retransmits and so on) are added with the
/// `getsockopt` call.
#[derive(Clone, Copy, Serialize)]
pub struct TcpSnapshot {}
