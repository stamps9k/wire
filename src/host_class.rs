//! Classification of the `Host` header.

use serde::Serialize;

/// The host name this server is reached by.
pub const HOSTNAME: &str = "wire.stampatron.com";

/// What the `Host` header of a request named.
///
/// Scanners that sweep address ranges ask for the bare IP address, while
/// visitors who followed a link ask for the server by name. Written to JSON
/// in snake case.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostClass {
  /// An IP address.
  Ip,
  /// This server's own host name.
  Own,
  /// Any other name.
  Other,
  /// No `Host` header was sent, which only HTTP/1.0 allows.
  Absent,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn variants_are_snake_case() -> Result<(), serde_json::Error> {
    let json = |class: HostClass| serde_json::to_string(&class);
    assert_eq!(json(HostClass::Ip)?, r#""ip""#);
    assert_eq!(json(HostClass::Own)?, r#""own""#);
    assert_eq!(json(HostClass::Other)?, r#""other""#);
    assert_eq!(json(HostClass::Absent)?, r#""absent""#);
    Ok(())
  }
}
