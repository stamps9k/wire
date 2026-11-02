//! [`Transport`]: which listener accepted a connection.

use serde::Serialize;

/// How the client reached the server: over plain TCP or over TLS.
///
/// Written to JSON in snake case.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
  /// Accepted on the plain HTTP listener.
  Plain,
  /// Accepted on the TLS listener. The handshake may not have completed.
  Tls,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn transports_are_snake_case() -> Result<(), serde_json::Error> {
    let json = |transport: Transport| serde_json::to_string(&transport);
    assert_eq!(json(Transport::Plain)?, r#""plain""#);
    assert_eq!(json(Transport::Tls)?, r#""tls""#);
    Ok(())
  }
}
