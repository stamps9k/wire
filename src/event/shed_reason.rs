//! [`ShedReason`]: why a connection was refused before anything was read.

use serde::Serialize;

/// Why a connection was closed as soon as it was accepted, before anything
/// was read from it.
///
/// Written to JSON in snake case.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShedReason {
  /// The client's address already had the most connections open that one
  /// address may have.
  IpLimit,
  /// The server was already serving the most connections it serves at
  /// once, from all addresses together.
  Capacity,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn shed_reasons_are_snake_case() -> Result<(), serde_json::Error> {
    let json = |reason: ShedReason| serde_json::to_string(&reason);
    assert_eq!(json(ShedReason::IpLimit)?, r#""ip_limit""#);
    assert_eq!(json(ShedReason::Capacity)?, r#""capacity""#);
    Ok(())
  }
}
