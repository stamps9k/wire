//! WebSocket protocol (RFC 6455): the opening handshake, which checks an
//! upgrade request and builds the response that accepts or refuses it, and
//! the framing of messages the server sends.

use crate::grammar;
use crate::header::Header;
use crate::request::Request;
use crate::response::Response;
use crate::status::Status;

use base64::engine::Engine;
use sha1::{Digest, Sha1};

/// Answers a WebSocket upgrade request.
///
/// A request that passes `validate` gets 101 Switching Protocols with the
/// `Upgrade`, `Connection` and `Sec-WebSocket-Accept` headers and no body.
/// Any other request gets an error page with the status that `validate`
/// returned.
///
/// The method is not checked here. The router only passes on `GET`
/// requests.
pub fn upgrade(request: &Request) -> Response {
  match validate(request) {
    Ok(key) => Response {
      status: Status::SwitchingProtocols,
      headers: vec![
        Header {
          name: "Upgrade".to_string(),
          value: Vec::from("websocket"),
        },
        Header {
          name: "Connection".to_string(),
          value: Vec::from("upgrade"),
        },
        Header {
          name: "Sec-WebSocket-Accept".to_string(),
          value: response_key(key),
        },
      ],
      body: Vec::new(),
    },
    Err(e) => Response::from_status(e),
  }
}

/// Computes the `Sec-WebSocket-Accept` value for a client's key.
///
/// The fixed GUID from RFC 6455 is appended to the key exactly as the client
/// sent it, the result is hashed with SHA-1, and the hash is base64-encoded.
/// A matching value shows the client that the server understood the
/// handshake.
fn response_key(request_key: &[u8]) -> Vec<u8> {
  let mut hasher = Sha1::new();
  hasher.update(request_key);
  hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
  let hash = hasher.finalize();
  base64::engine::general_purpose::STANDARD
    .encode(hash)
    .into_bytes()
}

/// Checks that `request` is a valid WebSocket upgrade request.
///
/// The version must be HTTP/1.1 or later, `Upgrade` must list `websocket`,
/// `Connection` must list `upgrade`, `Sec-WebSocket-Version` must be `13`,
/// and `Sec-WebSocket-Key` must be valid. Returns the key as the client
/// sent it.
///
/// # Errors
///
/// Returns the status to answer with if any check fails. Every failure is
/// currently [`Status::BadRequest`]. A more specific status, such as 426
/// for an unsupported version, can be added later.
fn validate(request: &Request) -> Result<&[u8], Status> {
  if request.version.major < 1
    || (request.version.major == 1 && request.version.minor < 1)
  {
    return Err(Status::BadRequest);
  }

  if !header_has_token(&request.headers, "upgrade", b"websocket") {
    return Err(Status::BadRequest);
  }

  if !header_has_token(&request.headers, "connection", b"upgrade") {
    return Err(Status::BadRequest);
  }

  // Check that the websocket version is valid
  if !request.headers.iter().any(|h| {
    h.name.eq_ignore_ascii_case("Sec-WebSocket-Version") && h.value == b"13"
  }) {
    return Err(Status::BadRequest);
  }

  websocket_key(&request.headers)
}

/// Reports whether any header called `name` lists `token`.
///
/// Each value is read as a comma-separated list. Whitespace around an
/// element is ignored and the comparison ignores case. Every header line
/// with that name is searched, because a list may be split over several
/// lines.
fn header_has_token(headers: &[Header], name: &str, token: &[u8]) -> bool {
  headers
    .iter()
    .filter(|h| h.name.eq_ignore_ascii_case(name))
    .any(|h| {
      h.value
        .split(|&i| i == b',')
        .any(|s| grammar::trim_ows(s).eq_ignore_ascii_case(token))
    })
}

/// Finds the `Sec-WebSocket-Key` header and checks its value.
///
/// The value must be base64 that decodes to 16 bytes. The decoded bytes are
/// only used for this check. The value is returned as the client sent it,
/// because that text is what the accept value is computed from.
///
/// # Errors
///
/// Returns [`Status::BadRequest`] if the header is missing, is not valid
/// base64 or decodes to any other length.
fn websocket_key(headers: &[Header]) -> Result<&[u8], Status> {
  let Some(header) = headers
    .iter()
    .find(|h| h.name.eq_ignore_ascii_case("Sec-WebSocket-Key"))
  else {
    return Err(Status::BadRequest);
  };

  let Ok(decode) =
    base64::engine::general_purpose::STANDARD.decode(&header.value)
  else {
    return Err(Status::BadRequest);
  };

  if decode.len() != 16 {
    return Err(Status::BadRequest);
  }

  Ok(&header.value)
}

/// Wraps `payload` in a single WebSocket text frame, as sent by a server.
///
/// The first byte sets FIN and the text opcode, because each payload is a
/// whole message. The length follows in the shortest form RFC 6455 allows:
/// one byte up to 125, then `126` and a big-endian `u16` up to 65535, then
/// `127` and a big-endian `u64`. Server frames are never masked, so the
/// mask bit is clear and the payload is copied unchanged.
///
/// The caller must make sure the payload is valid UTF-8, because a text
/// frame must hold UTF-8.
pub fn text_frame(payload: &[u8]) -> Vec<u8> {
  let len = u64::try_from(payload.len()).unwrap_or(u64::MAX);
  let mut out = Vec::with_capacity(payload.len().saturating_add(10));
  out.push(0x81);

  match (u8::try_from(len), u16::try_from(len)) {
    (Ok(short), _) if short < 126 => out.push(short),
    (_, Ok(medium)) => {
      out.push(126);
      out.extend_from_slice(&medium.to_be_bytes());
    }
    _ => {
      out.push(127);
      out.extend_from_slice(&len.to_be_bytes());
    }
  }
  out.extend_from_slice(payload);
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::method::Method;
  use crate::target::Target;
  use crate::version::Version;

  /// The example key from RFC 6455. It decodes to 16 bytes.
  const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

  fn header(name: &str, value: &str) -> Header {
    Header {
      name: name.to_owned(),
      value: value.as_bytes().to_vec(),
    }
  }

  /// The headers of a correct upgrade request, named as the parser stores
  /// them (lowercase).
  fn valid_headers() -> Vec<Header> {
    vec![
      header("host", "wire.stampatron.com"),
      header("upgrade", "websocket"),
      header("connection", "Upgrade"),
      header("sec-websocket-key", KEY),
      header("sec-websocket-version", "13"),
    ]
  }

  fn request(minor: u8, headers: Vec<Header>) -> Request {
    Request {
      method: Method::Get,
      target: Target::Origin("/ws".to_owned()),
      version: Version { major: 1, minor },
      headers,
    }
  }

  /// A correct upgrade request with the header `name` removed.
  fn without(name: &str) -> Request {
    let mut headers = valid_headers();
    headers.retain(|h| h.name != name);
    request(1, headers)
  }

  /// A correct upgrade request with the value of `name` replaced.
  fn with(name: &str, value: &str) -> Request {
    let mut request = without(name);
    request.headers.push(header(name, value));
    request
  }

  fn accepts(request: &Request) -> bool {
    validate(request).is_ok()
  }

  // ---- The request as a whole ----

  #[test]
  fn accepts_a_correct_upgrade_request() {
    assert!(accepts(&request(1, valid_headers())));
  }

  #[test]
  fn ignores_headers_it_does_not_need() {
    let mut request = request(1, valid_headers());
    request
      .headers
      .push(header("origin", "https://stampatron.com"));
    request
      .headers
      .push(header("sec-websocket-protocol", "chat"));
    request
      .headers
      .push(header("sec-websocket-extensions", "permessage-deflate"));
    assert!(accepts(&request));
  }

  #[test]
  fn rejects_http_1_0() {
    assert!(!accepts(&request(0, valid_headers())));
  }

  #[test]
  fn rejects_a_plain_get() {
    assert!(!accepts(&request(1, vec![header("host", "example.com")])));
  }

  // ---- Upgrade ----

  #[test]
  fn rejects_a_missing_upgrade_header() {
    assert!(!accepts(&without("upgrade")));
  }

  #[test]
  fn upgrade_value_ignores_case() {
    assert!(accepts(&with("upgrade", "WebSocket")));
    assert!(accepts(&with("upgrade", "WEBSOCKET")));
  }

  #[test]
  fn rejects_an_upgrade_to_another_protocol() {
    assert!(!accepts(&with("upgrade", "h2c")));
    assert!(!accepts(&with("upgrade", "")));
  }

  // ---- Connection ----

  #[test]
  fn rejects_a_missing_connection_header() {
    assert!(!accepts(&without("connection")));
  }

  #[test]
  fn connection_token_ignores_case() {
    assert!(accepts(&with("connection", "upgrade")));
    assert!(accepts(&with("connection", "UPGRADE")));
  }

  #[test]
  fn connection_may_list_other_tokens() {
    assert!(accepts(&with("connection", "keep-alive, Upgrade")));
    assert!(accepts(&with("connection", "Upgrade, keep-alive")));
    assert!(accepts(&with("connection", "keep-alive,Upgrade")));
  }

  #[test]
  fn connection_allows_whitespace_around_commas() {
    assert!(accepts(&with("connection", "keep-alive ,\tUpgrade")));
    assert!(accepts(&with("connection", "keep-alive  ,  Upgrade")));
  }

  #[test]
  fn connection_tolerates_empty_list_elements() {
    assert!(accepts(&with("connection", "keep-alive, , Upgrade,")));
    assert!(accepts(&with("connection", ",Upgrade")));
  }

  #[test]
  fn connection_may_be_split_over_several_lines() {
    let mut request = without("connection");
    request.headers.push(header("connection", "keep-alive"));
    request.headers.push(header("connection", "Upgrade"));
    assert!(accepts(&request));
  }

  #[test]
  fn rejects_a_connection_header_without_upgrade() {
    assert!(!accepts(&with("connection", "keep-alive")));
    assert!(!accepts(&with("connection", "close")));
    assert!(!accepts(&with("connection", "")));
  }

  #[test]
  fn rejects_connection_tokens_that_only_contain_upgrade() {
    assert!(!accepts(&with("connection", "upgrades")));
    assert!(!accepts(&with("connection", "no-upgrade")));
    assert!(!accepts(&with("connection", "up grade")));
  }

  #[test]
  fn rejects_websocket_as_a_connection_token() {
    // `websocket` belongs in Upgrade. Connection must name `upgrade`.
    assert!(!accepts(&with("connection", "websocket")));
  }

  #[test]
  fn rejects_a_connection_value_that_is_not_utf8() {
    let mut request = without("connection");
    request.headers.push(Header {
      name: "connection".to_owned(),
      value: vec![0xff, 0xfe],
    });
    assert!(!accepts(&request));
  }

  // ---- Sec-WebSocket-Version ----

  #[test]
  fn rejects_a_missing_version_header() {
    assert!(!accepts(&without("sec-websocket-version")));
  }

  #[test]
  fn rejects_other_versions() {
    assert!(!accepts(&with("sec-websocket-version", "8")));
    assert!(!accepts(&with("sec-websocket-version", "12")));
    assert!(!accepts(&with("sec-websocket-version", "14")));
    assert!(!accepts(&with("sec-websocket-version", "130")));
    assert!(!accepts(&with("sec-websocket-version", "")));
  }

  // ---- Sec-WebSocket-Key ----

  #[test]
  fn rejects_a_missing_key() {
    assert!(!accepts(&without("sec-websocket-key")));
  }

  #[test]
  fn rejects_a_key_that_is_not_base64() {
    assert!(!accepts(&with("sec-websocket-key", "not base64!")));
    assert!(!accepts(&with("sec-websocket-key", "")));
  }

  #[test]
  fn rejects_a_key_without_its_padding() {
    assert!(!accepts(&with(
      "sec-websocket-key",
      "dGhlIHNhbXBsZSBub25jZQ"
    )));
  }

  #[test]
  fn rejects_a_key_of_the_wrong_length() {
    // 5, 15, 17 and 20 bytes once decoded.
    assert!(!accepts(&with("sec-websocket-key", "c2hvcnQ=")));
    assert!(!accepts(&with("sec-websocket-key", "AAECAwQFBgcICQoLDA0O")));
    assert!(!accepts(&with(
      "sec-websocket-key",
      "AAECAwQFBgcICQoLDA0ODxA="
    )));
    assert!(!accepts(&with(
      "sec-websocket-key",
      "AAECAwQFBgcICQoLDA0ODxAREhM="
    )));
  }

  #[test]
  fn accepts_any_key_of_sixteen_bytes() {
    assert!(accepts(&with(
      "sec-websocket-key",
      "AAAAAAAAAAAAAAAAAAAAAA=="
    )));
    assert!(accepts(&with(
      "sec-websocket-key",
      "/+/+/+/+/+/+/+/+/+/+/w=="
    )));
  }

  #[test]
  fn connection_allows_mixed_spaces_and_tabs() {
    assert!(accepts(&with("connection", "keep-alive,\t Upgrade")));
    assert!(accepts(&with("connection", "Upgrade \t, keep-alive")));
  }

  // ---- upgrade ----

  fn value<'a>(response: &'a Response, name: &str) -> Option<&'a [u8]> {
    response
      .headers
      .iter()
      .find(|h| h.name.eq_ignore_ascii_case(name))
      .map(|h| h.value.as_slice())
  }

  #[test]
  fn upgrade_answers_a_correct_request_with_101() {
    let response = upgrade(&request(1, valid_headers()));
    assert_eq!(response.status.code(), 101);
    assert!(response.body.is_empty());
  }

  #[test]
  fn upgrade_computes_the_accept_value_of_the_rfc_example() {
    let response = upgrade(&request(1, valid_headers()));
    assert_eq!(
      value(&response, "sec-websocket-accept"),
      Some(b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".as_slice())
    );
  }

  #[test]
  fn upgrade_computes_the_accept_value_from_the_key_sent() {
    let key = "AAAAAAAAAAAAAAAAAAAAAA==";
    let response = upgrade(&with("sec-websocket-key", key));
    assert_eq!(
      value(&response, "sec-websocket-accept"),
      Some(b"ICX+Yqv66kxgM0FcWaLWlFLwTAI=".as_slice())
    );
  }

  #[test]
  fn upgrade_sends_exactly_the_three_handshake_headers() {
    let response = upgrade(&request(1, valid_headers()));
    assert_eq!(response.headers.len(), 3);
    assert!(
      value(&response, "upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case(b"websocket"))
    );
    assert!(
      value(&response, "connection")
        .is_some_and(|v| v.eq_ignore_ascii_case(b"upgrade"))
    );
  }

  #[test]
  fn upgrade_answers_a_failed_handshake_with_400() {
    let response = upgrade(&without("sec-websocket-key"));
    assert_eq!(response.status.code(), 400);
    assert!(value(&response, "sec-websocket-accept").is_none());
  }

  // ---- text_frame ----

  #[test]
  fn text_frame_of_an_empty_payload_is_two_bytes() {
    assert_eq!(text_frame(b""), [0x81, 0x00]);
  }

  #[test]
  fn text_frame_matches_the_rfc_hello_example() {
    // RFC 6455 section 5.7: a single-frame unmasked text message.
    assert_eq!(
      text_frame(b"Hello"),
      [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]
    );
  }

  #[test]
  fn text_frame_uses_one_length_byte_up_to_125() {
    let payload = vec![b'a'; 125];
    let frame = text_frame(&payload);
    assert!(frame.starts_with(&[0x81, 125]));
    assert_eq!(frame.len(), 127);
    assert_eq!(frame.get(2..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_switches_to_two_length_bytes_at_126() {
    let payload = vec![b'a'; 126];
    let frame = text_frame(&payload);
    assert!(frame.starts_with(&[0x81, 126, 0x00, 0x7e]));
    assert_eq!(frame.len(), 130);
    assert_eq!(frame.get(4..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_writes_the_two_byte_length_big_endian() {
    // RFC 6455 section 5.7 gives 256 bytes as 0x7e 0x0100.
    let frame = text_frame(&vec![b'a'; 256]);
    assert!(frame.starts_with(&[0x81, 126, 0x01, 0x00]));
    assert_eq!(frame.len(), 260);
  }

  #[test]
  fn text_frame_uses_two_length_bytes_up_to_65535() {
    let payload = vec![b'a'; 65_535];
    let frame = text_frame(&payload);
    assert!(frame.starts_with(&[0x81, 126, 0xff, 0xff]));
    assert_eq!(frame.len(), 65_539);
    assert_eq!(frame.get(4..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_switches_to_eight_length_bytes_at_65536() {
    // RFC 6455 section 5.7 gives 64 KiB as 0x7f 0x0000000000010000.
    let payload = vec![b'a'; 65_536];
    let frame = text_frame(&payload);
    assert!(frame.starts_with(&[0x81, 127, 0, 0, 0, 0, 0, 1, 0, 0]));
    assert_eq!(frame.len(), 65_546);
    assert_eq!(frame.get(10..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_never_sets_the_mask_bit() {
    // A server must not mask the frames it sends.
    for length in [0, 1, 125, 126, 127, 128, 255, 256, 65_535, 65_536] {
      let frame = text_frame(&vec![0xff; length]);
      assert_eq!(frame.get(1).map(|byte| byte & 0x80), Some(0), "{length}");
    }
  }

  #[test]
  fn text_frame_always_starts_with_fin_and_the_text_opcode() {
    for length in [0, 1, 125, 126, 65_535, 65_536] {
      let frame = text_frame(&vec![b'a'; length]);
      assert_eq!(frame.first(), Some(&0x81), "{length}");
    }
  }

  #[test]
  fn text_frame_copies_the_payload_unchanged() {
    // Bytes that look like frame headers must not be touched.
    let payload = [0x81, 0x7e, 0x7f, 0x00, 0xff, b'\r', b'\n'];
    let frame = text_frame(&payload);
    assert_eq!(frame.get(..2), Some([0x81, 0x07].as_slice()));
    assert_eq!(frame.get(2..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_counts_the_length_in_bytes() {
    // Two characters, five bytes in UTF-8.
    let frame = text_frame("é€".as_bytes());
    assert_eq!(frame.get(1), Some(&5));
    assert_eq!(frame.len(), 7);
  }
}
