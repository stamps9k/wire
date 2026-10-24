//! HTML bodies for the pages wire serves.
//!
//! Each page function returns only a body. The status and headers are chosen by
//! the router.

use std::fmt::Write as _;

use crate::http::Method;
use crate::http::Request;
use crate::http::Status;
use crate::http::Target;
use crate::http::Version;
use crate::site::html::escape;
use crate::site::html::render;

/// Request headers whose values the index page never shows, because they
/// carry credentials.
///
/// The page is served over plain HTTP on a subdomain of stampatron.com, so
/// a browser can send it cookies set for the whole domain. Echoing them
/// would put session values into a page anyone on the path can read. Each
/// of these headers is still listed, with its value replaced by its length.
///
/// Names must be lowercase, because the parser lowercases every header
/// name before the page sees it.
const REDACTED_HEADERS: &[&str] =
  &["authorization", "cookie", "proxy-authorization"];

/// Renders every page that is built from a template, and discards the
/// result.
///
/// Whether rendering succeeds depends only on the template and the
/// placeholder names, which are fixed at compile time, never on the request.
/// So one render with a dummy request proves that every later render will
/// succeed. Call this at startup, before accepting connections, so that a
/// broken template stops the server instead of making every request for
/// that page return an empty reply.
///
/// Add each new templated page here.
///
/// # Panics
///
/// Panics if any template is invalid. See [`index`].
pub fn validate_templates() {
  let test_request = Request {
    method: Method::Get,
    target: Target::Asterisk,
    version: Version { major: 1, minor: 1 },
    headers: Vec::new(),
  };

  let _ = index(&test_request);
}

/// Builds an HTML page that echoes the request head back to the client.
///
/// Shows the method, target and version, followed by the header fields in
/// the order they were received. Every value comes from the client, so each
/// one is HTML-escaped with [`escape`] before it is inserted into
/// `public/index.html`. Header values are raw bytes, so any that are not
/// valid UTF-8 are shown with replacement characters. The value of any
/// header in [`REDACTED_HEADERS`] is never shown: the header is listed as
/// `[redacted, N bytes]` instead.
///
/// # Panics
///
/// Panics if `public/index.html` contains a placeholder other than the four
/// this function supplies, or an unclosed `{{`. The template and the keys are
/// both fixed at compile time, so this cannot depend on the request. The
/// `index_*` tests and the startup check catch it before the server accepts
/// connections.
pub fn index(request: &Request) -> Vec<u8> {
  let mut headers = String::new();
  for header in &request.headers {
    // Writing to a `String` cannot fail, so the result is discarded.
    if REDACTED_HEADERS.contains(&header.name.as_str()) {
      // Never show credentials
      let _ = write!(
        headers,
        "<li>{}: [redacted, {} bytes]</li>",
        escape(&header.name),
        header.value.len()
      );
    } else {
      let _ = write!(
        headers,
        "<li>{}: {}</li>",
        escape(&header.name),
        escape(&String::from_utf8_lossy(&header.value))
      );
    }
  }

  let template = include_str!("../../public/index.html");
  let method = escape(&request.method.to_string());
  let target = escape(&request.target.to_string());
  let version = escape(&request.version.to_string());
  let values = [
    ("METHOD", method.as_str()),
    ("TARGET", target.as_str()),
    ("VERSION", version.as_str()),
    ("HEADERS", headers.as_str()),
  ];

  #[allow(
    clippy::expect_used,
    reason = "the template and keys are fixed; tested by index_* tests and tested on startup"
  )]
  render(template, &values)
    .expect("public/index.html has only the placeholders index supplies")
    .into_bytes()
}

/// Returns the debug page, which shows the event stream from `/ws` as text.
///
/// The page is static: it does not depend on the request.
pub fn debug() -> Vec<u8> {
  include_str!("../../public/debug.html").as_bytes().to_vec()
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
  use crate::http::Header;
  use crate::http::Method;
  use crate::http::RequestError;
  use crate::http::Target;
  use crate::http::Version;

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
  fn debug_is_the_debug_page() {
    let body = String::from_utf8_lossy(&debug()).into_owned();
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("<title>wire debug</title>"));
    assert!(body.contains("/ws"));
  }

  #[test]
  fn debug_has_no_placeholders() {
    let body = String::from_utf8_lossy(&debug()).into_owned();
    assert!(!body.contains("{{"));
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
    assert!(!body.contains("{{"));
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
  fn index_redacts_the_value_of_every_credential_header() {
    for name in ["authorization", "cookie", "proxy-authorization"] {
      let body = index_text(&request(vec![header(name, b"secret-value")]));
      assert!(
        body.contains(&format!("<li>{name}: [redacted, 12 bytes]</li>")),
        "{name} was not redacted"
      );
      assert!(!body.contains("secret-value"), "{name} value was shown");
    }
  }

  #[test]
  fn index_redacts_credentials_whatever_case_they_were_sent_in()
  -> Result<(), RequestError> {
    // The parser lowercases names, so the page only has to match lowercase.
    let headers = vec![
      Header::parse(b"COOKIE: session=abc")?,
      Header::parse(b"Authorization: Bearer xyz")?,
    ];
    let body = index_text(&request(headers));
    assert!(body.contains("<li>cookie: [redacted, 11 bytes]</li>"));
    assert!(body.contains("<li>authorization: [redacted, 10 bytes]</li>"));
    assert!(!body.contains("session=abc"));
    assert!(!body.contains("Bearer xyz"));
    Ok(())
  }

  #[test]
  fn index_counts_the_bytes_of_a_redacted_value() {
    let empty = index_text(&request(vec![header("cookie", b"")]));
    assert!(empty.contains("<li>cookie: [redacted, 0 bytes]</li>"));
    let raw = index_text(&request(vec![header("cookie", b"a\xffb")]));
    assert!(raw.contains("<li>cookie: [redacted, 3 bytes]</li>"));
  }

  #[test]
  fn index_shows_other_headers_in_full_around_a_redacted_one() {
    let headers = vec![
      header("host", b"example.com"),
      header("cookie", b"session=abc"),
      header("accept", b"*/*"),
    ];
    let body = index_text(&request(headers));
    assert!(body.contains(
      "<ul><li>host: example.com</li>\
       <li>cookie: [redacted, 11 bytes]</li>\
       <li>accept: */*</li></ul>"
    ));
  }

  #[test]
  fn index_only_redacts_exact_header_names() {
    let headers = vec![
      header("x-cookie", b"shown"),
      header("cookies", b"also shown"),
      header("authorization-hint", b"visible"),
    ];
    let body = index_text(&request(headers));
    assert!(body.contains("<li>x-cookie: shown</li>"));
    assert!(body.contains("<li>cookies: also shown</li>"));
    assert!(body.contains("<li>authorization-hint: visible</li>"));
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

  #[test]
  fn index_escapes_the_target() {
    let request = Request {
      method: Method::Get,
      target: Target::Origin("/?<script>alert(1)</script>".to_owned()),
      version: Version { major: 1, minor: 1 },
      headers: Vec::new(),
    };
    let body = index_text(&request);
    assert!(
      body.contains("<p>Target: /?&lt;script&gt;alert(1)&lt;/script&gt;</p>")
    );
    assert!(!body.contains("<script>"));
  }

  #[test]
  fn index_escapes_header_values() {
    let body = index_text(&request(vec![header("x-a", b"<b>\"'&")]));
    assert!(body.contains("<li>x-a: &lt;b&gt;&quot;&#39;&amp;</li>"));
  }

  #[test]
  fn index_does_not_expand_placeholders_from_the_request() {
    let request = Request {
      method: Method::Get,
      target: Target::Origin("/{{HEADERS}}".to_owned()),
      version: Version { major: 1, minor: 1 },
      headers: vec![header("host", b"a")],
    };
    let body = index_text(&request);
    assert!(body.contains("<p>Target: /{{HEADERS}}</p>"));
  }

  #[test]
  fn validate_templates_passes() {
    validate_templates();
  }
}
