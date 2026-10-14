//! The [`Event`] record, and the truncation that caps text taken from the
//! client before it goes into one.

use std::net::SocketAddr;

use serde::Serialize;

use crate::event::Kind;
use crate::event::TcpSnapshot;

/// Longest text, in bytes, that an event carries for any one field taken
/// from the client.
pub const CAP_SIZE: usize = 128;

/// Cuts `s` to at most `max_length` bytes.
///
/// The cut is moved back to the nearest character boundary, so the result
/// is always valid UTF-8 and may be slightly shorter than `max_length`.
/// Text already within the limit is returned unchanged.
pub fn truncate(mut s: String, max_length: usize) -> String {
  let end = (0..=max_length).rev().find(|&i| s.is_char_boundary(i));

  s.truncate(end.unwrap_or_default());
  s
}

/// What happened on one client connection.
///
/// Serialises to a single flat JSON object: the common fields below, then a
/// `kind` tag and the fields of that [`Kind`]. Absent values are written as
/// `null`, so every key is always present for its kind.
///
/// All text taken from the client is truncated before it is stored here,
/// because each event is sent to every subscriber.
#[derive(Clone, Serialize)]
pub struct Event {
  /// Counter assigned when the connection is accepted. Ties together
  /// events that belong to the same connection.
  pub connection_id: u64,
  /// The client's IP address and port.
  pub peer_address: SocketAddr,
  /// When the connection was accepted, in milliseconds since the Unix
  /// epoch.
  pub timestamp_ms: u64,
  /// Time from accepting the connection to finishing with it, in
  /// milliseconds.
  pub duration_ms: u64,
  /// Number of bytes read from the client.
  pub bytes_received: u32,
  /// `TCP_INFO` for the connection, taken just before it is closed. `None`
  /// if it could not be read.
  pub tcp_snapshot: Option<TcpSnapshot>,
  /// How the connection ended, with the data specific to that outcome.
  #[serde(flatten)]
  pub kind: Kind,
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::event::RejectReason;
  use crate::http::HostClass;

  fn assert_broadcastable<T: Clone + Send + 'static>() {}

  fn event(kind: Kind) -> Event {
    Event {
      connection_id: 7,
      peer_address: SocketAddr::from(([203, 0, 113, 9], 51_234)),
      timestamp_ms: 1_700_000_000_000,
      duration_ms: 12,
      bytes_received: 78,
      tcp_snapshot: None,
      kind,
    }
  }

  fn request(target: &str, user_agent: Option<&str>) -> Kind {
    Kind::Request {
      method: "GET".to_owned(),
      target: target.to_owned(),
      target_length: 1,
      version: "HTTP/1.1".to_owned(),
      status: 200,
      user_agent: user_agent.map(str::to_owned),
      user_agent_length: 8,
      host_class: HostClass::Own,
      header_count: 3,
    }
  }

  fn rejected(
    reason: RejectReason,
    detail: Option<&'static str>,
    status: Option<u16>,
  ) -> Kind {
    Kind::Rejected {
      reason,
      detail,
      status,
    }
  }

  const COMMON: &str = concat!(
    r#"{"connection_id":7,"#,
    r#""peer_address":"203.0.113.9:51234","#,
    r#""timestamp_ms":1700000000000,"#,
    r#""duration_ms":12,"#,
    r#""bytes_received":78,"#,
    r#""tcp_snapshot":null,"#,
  );

  #[test]
  fn event_can_be_broadcast() {
    assert_broadcastable::<Event>();
  }

  #[test]
  fn request_is_one_flat_object() -> Result<(), serde_json::Error> {
    let json = serde_json::to_string(&event(request("/", Some("curl/8.0"))))?;
    let kind = concat!(
      r#""kind":"request","#,
      r#""method":"GET","#,
      r#""target":"/","#,
      r#""target_length":1,"#,
      r#""version":"HTTP/1.1","#,
      r#""status":200,"#,
      r#""user_agent":"curl/8.0","#,
      r#""user_agent_length":8,"#,
      r#""host_class":"own","#,
      r#""header_count":3}"#,
    );
    assert_eq!(json, format!("{COMMON}{kind}"));
    Ok(())
  }

  #[test]
  fn rejected_is_one_flat_object() -> Result<(), serde_json::Error> {
    let kind = rejected(RejectReason::Malformed, Some("bad target"), Some(400));
    let json = serde_json::to_string(&event(kind))?;
    let kind = concat!(
      r#""kind":"rejected","#,
      r#""reason":"malformed","#,
      r#""detail":"bad target","#,
      r#""status":400}"#,
    );
    assert_eq!(json, format!("{COMMON}{kind}"));
    Ok(())
  }

  #[test]
  fn absent_user_agent_is_null() -> Result<(), serde_json::Error> {
    let json = serde_json::to_string(&event(request("/", None)))?;
    assert!(json.contains(r#""user_agent":null,"#));
    Ok(())
  }

  #[test]
  fn absent_detail_and_status_are_null() -> Result<(), serde_json::Error> {
    let kind = rejected(RejectReason::Closed, None, None);
    let json = serde_json::to_string(&event(kind))?;
    assert!(
      json.ends_with(r#""reason":"closed","detail":null,"status":null}"#)
    );
    Ok(())
  }

  #[test]
  fn present_snapshot_is_an_object() -> Result<(), serde_json::Error> {
    let mut event = event(rejected(RejectReason::Closed, None, None));
    event.tcp_snapshot = Some(TcpSnapshot {});
    let json = serde_json::to_string(&event)?;
    assert!(json.contains(r#""tcp_snapshot":{"#));
    Ok(())
  }

  #[test]
  fn quote_and_backslash_are_escaped() -> Result<(), serde_json::Error> {
    let json = serde_json::to_string(&event(request(r#"/a"b\c"#, None)))?;
    assert!(json.contains(r#""target":"/a\"b\\c","#));
    Ok(())
  }

  #[test]
  fn control_bytes_are_escaped() -> Result<(), serde_json::Error> {
    let json = serde_json::to_string(&event(request("/a\u{1}\n", None)))?;
    assert!(json.contains(r#""target":"/a\u0001\n","#));
    Ok(())
  }

  #[test]
  fn injected_fields_stay_inside_the_string() -> Result<(), serde_json::Error> {
    let target = r#"/","status":999,"x":""#;
    let json = serde_json::to_string(&event(request(target, None)))?;
    assert!(json.contains(r#""target":"/\",\"status\":999,\"x\":\"","#));
    assert!(json.contains(r#""status":200,"#));
    assert!(!json.contains(r#""status":999"#));
    Ok(())
  }

  #[test]
  fn non_ascii_text_is_kept() -> Result<(), serde_json::Error> {
    let json = serde_json::to_string(&event(request("/", Some("café"))))?;
    assert!(json.contains(r#""user_agent":"café","#));
    Ok(())
  }

  #[test]
  fn ipv6_peer_address_is_bracketed() -> Result<(), serde_json::Error> {
    let mut event = event(rejected(RejectReason::Closed, None, None));
    event.peer_address = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 8080));
    let json = serde_json::to_string(&event)?;
    assert!(json.contains(r#""peer_address":"[::1]:8080","#));
    Ok(())
  }

  #[test]
  fn truncate_leaves_short_text_alone() {
    assert_eq!(truncate("abc".to_owned(), 5), "abc");
  }

  #[test]
  fn truncate_leaves_text_at_the_limit_alone() {
    assert_eq!(truncate("abcde".to_owned(), 5), "abcde");
  }

  #[test]
  fn truncate_cuts_long_text_to_the_limit() {
    assert_eq!(truncate("abcdefgh".to_owned(), 5), "abcde");
  }

  #[test]
  fn truncate_never_splits_a_two_byte_character() {
    // 'é' occupies bytes 4 and 5, so a limit of 5 falls inside it.
    assert_eq!(truncate("abcdé".to_owned(), 5), "abcd");
    assert_eq!(truncate("abcdé".to_owned(), 6), "abcdé");
  }

  #[test]
  fn truncate_never_splits_a_replacement_character() {
    // U+FFFD, which lossy conversion inserts, occupies bytes 2 to 4.
    assert_eq!(truncate("ab\u{fffd}cd".to_owned(), 3), "ab");
    assert_eq!(truncate("ab\u{fffd}cd".to_owned(), 4), "ab");
    assert_eq!(truncate("ab\u{fffd}cd".to_owned(), 5), "ab\u{fffd}");
  }

  #[test]
  fn truncate_handles_empty_text_and_a_zero_limit() {
    assert_eq!(truncate(String::new(), 5), "");
    assert_eq!(truncate("abc".to_owned(), 0), "");
    assert_eq!(truncate("é".to_owned(), 1), "");
  }

  #[test]
  fn truncate_to_cap_size_gives_exactly_cap_size_bytes() {
    let long = "a".repeat(CAP_SIZE.saturating_mul(2));
    assert_eq!(truncate(long, CAP_SIZE).len(), CAP_SIZE);
  }
}
