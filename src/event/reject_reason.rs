//! [`RejectReason`]: why a connection produced no valid request.

use serde::Serialize;

/// Why no valid request was obtained from a connection.
///
/// Mirrors the variants of [`RequestError`](crate::http::RequestError)
/// without their data, so that it can be cloned and serialised. Written to
/// JSON in snake case.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
  /// The client closed the connection before the request head was complete.
  Closed,
  /// Reading from or writing to the socket failed.
  Io,
  /// The request head exceeded the size limit.
  TooLarge,
  /// The request failed parsing or validation.
  Malformed,
  /// The request head was not complete within the deadline.
  Timeout,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reject_reasons_are_snake_case() -> Result<(), serde_json::Error> {
    let json = |reason: RejectReason| serde_json::to_string(&reason);
    assert_eq!(json(RejectReason::Closed)?, r#""closed""#);
    assert_eq!(json(RejectReason::Io)?, r#""io""#);
    assert_eq!(json(RejectReason::TooLarge)?, r#""too_large""#);
    assert_eq!(json(RejectReason::Malformed)?, r#""malformed""#);
    assert_eq!(json(RejectReason::Timeout)?, r#""timeout""#);
    Ok(())
  }
}
