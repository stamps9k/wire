//! [`Kind`]: the part of an event that depends on how the connection ended.

use serde::Serialize;

use crate::event::RejectReason;
use crate::event::ShedReason;
use crate::http::HostClass;

/// How a connection ended.
///
/// Serialised with the variant name as a `kind` field, in snake case, next
/// to the variant's own fields.
#[derive(Clone, Serialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum Kind {
  /// A request was parsed and validated, and a response was chosen for it.
  Request {
    /// The request method as text, such as `GET`. Truncated.
    method: String,
    /// The request target as text, such as `/index.html`. Truncated.
    target: String,
    /// Length of the target in bytes before it was truncated.
    target_length: u32,
    /// The HTTP version as text.
    version: String,
    /// Status code of the response.
    status: u16,
    /// Value of the `User-Agent` header, converted lossily to UTF-8 and
    /// truncated. `None` if the header is absent.
    user_agent: Option<String>,
    /// Length of the `User-Agent` value in bytes before it was truncated,
    /// or 0 if the header is absent.
    user_agent_length: u32,
    /// What the `Host` header named.
    host_class: HostClass,
    /// Number of header fields in the request.
    header_count: u16,
  },
  /// No valid request was obtained from the client.
  Rejected {
    /// Why the request was rejected.
    reason: RejectReason,
    /// Which part of the request was at fault. Only set when `reason` is
    /// [`RejectReason::Malformed`].
    detail: Option<&'static str>,
    /// Status code of the error response, or `None` if nothing was sent.
    status: Option<u16>,
  },
  /// The connection was closed as soon as it was accepted, because a
  /// connection limit was reached. Nothing was read from it and no response
  /// was sent.
  Shed {
    /// Which limit was reached.
    reason: ShedReason,
  },
}
