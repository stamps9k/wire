//! HTML bodies for the pages wire serves.
//!
//! Each function returns only a body. The status and headers are chosen by
//! the router or by [`Response::from_status`](crate::response::Response::from_status).

use std::fmt::Write as _;

use crate::request::Request;
use crate::status::Status;

/// Builds an HTML page that echoes the request head back to the client.
///
/// The method, target and version are inserted exactly as they were parsed,
/// followed by the header fields in the order they were received. Header
/// values are raw bytes, so any that are not valid UTF-8 are shown with
/// replacement characters.
//
// TODO: escape the values for HTML before this is reachable from the public
// address. The target, method and header fields are client-controlled.
pub fn index(request: &Request) -> Vec<u8> {
  let mut headers = String::new();
  for header in &request.headers {
    // Writing to a `String` cannot fail, so the result is discarded.
    let _ = write!(
      headers,
      "<li>{}: {}</li>",
      header.name,
      String::from_utf8_lossy(&header.value)
    );
  }

  include_str!("../public/index.html")
    .replace("__METHOD__", &request.method.to_string())
    .replace("__TARGET__", &request.target.to_string())
    .replace("__VERSION__", &request.version.to_string())
    .replace("__HEADERS__", headers.as_str())
    .into_bytes()
}

/// Builds an HTML page showing the code and reason phrase of `status`.
pub fn error(status: Status) -> Vec<u8> {
  format!(
    "<!doctype html>\
     <title>wire</title>\
     <h1>wire</h1>\
     <p>Error Code: {}</p>\
     <p>Reason: {}</p>",
    status.code(),
    status.reason()
  )
  .into_bytes()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::header::Header;
  use crate::method::Method;
  use crate::target::Target;
  use crate::version::Version;

  fn request(headers: Vec<Header>) -> Request {
    Request {
      method: Method::Get,
      target: Target::Origin("/a?b=c".to_owned()),
      version: Version { major: 1, minor: 1 },
      headers,
    }
  }

  fn header(name: &str, value: &[u8]) -> Header {
    Header {
      name: name.to_owned(),
      value: value.to_vec(),
    }
  }

  fn index_text(request: &Request) -> String {
    String::from_utf8_lossy(&index(request)).into_owned()
  }

  fn error_text(status: Status) -> String {
    String::from_utf8_lossy(&error(status)).into_owned()
  }

  #[test]
  fn index_contains_request_line_parts() {
    let body = index_text(&request(Vec::new()));
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("<p>Method: GET</p>"));
    assert!(body.contains("<p>Target: /a?b=c</p>"));
    assert!(body.contains("<p>Version: HTTP/1.1</p>"));
  }

  #[test]
  fn index_shows_other_methods_targets_and_versions() {
    let request = Request {
      method: Method::Other("PURGE".to_owned()),
      target: Target::Asterisk,
      version: Version { major: 1, minor: 0 },
      headers: Vec::new(),
    };
    let body = index_text(&request);
    assert!(body.contains("<p>Method: PURGE</p>"));
    assert!(body.contains("<p>Target: *</p>"));
    assert!(body.contains("<p>Version: HTTP/1.0</p>"));
  }

  #[test]
  fn index_replaces_every_placeholder() {
    let body = index_text(&request(vec![header("host", b"a")]));
    assert!(!body.contains("__"));
  }

  #[test]
  fn index_with_no_headers_has_an_empty_list() {
    let body = index_text(&request(Vec::new()));
    assert!(body.contains("<ul></ul>"));
  }

  #[test]
  fn index_lists_headers_in_order() {
    let headers =
      vec![header("host", b"example.com"), header("accept", b"*/*")];
    let body = index_text(&request(headers));
    assert!(
      body.contains("<ul><li>host: example.com</li><li>accept: */*</li></ul>")
    );
  }

  #[test]
  fn index_keeps_repeated_headers() {
    let headers = vec![header("x-a", b"1"), header("x-a", b"2")];
    let body = index_text(&request(headers));
    assert!(body.contains("<li>x-a: 1</li><li>x-a: 2</li>"));
  }

  #[test]
  fn index_shows_header_values_that_are_not_utf8_as_replacements() {
    let body = index_text(&request(vec![header("x-raw", b"a\xffb")]));
    assert!(body.contains("<li>x-raw: a\u{fffd}b</li>"));
  }

  #[test]
  fn index_shows_an_empty_header_value() {
    let body = index_text(&request(vec![header("x-empty", b"")]));
    assert!(body.contains("<li>x-empty: </li>"));
  }

  #[test]
  fn error_shows_code_and_reason() {
    let body = error_text(Status::NotFound);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("<p>Error Code: 404</p>"));
    assert!(body.contains("<p>Reason: Not Found</p>"));
  }

  #[test]
  fn error_follows_the_status() {
    let body = error_text(Status::RequestTimeout);
    assert!(body.contains("<p>Error Code: 408</p>"));
    assert!(body.contains("<p>Reason: Request Timeout</p>"));
    assert!(!body.contains("404"));
  }
}
