use crate::header::Header;
use crate::request::Request;
use crate::status::Status;

use std::fmt::Write as _;

/// An HTTP response: a status, extra headers and an HTML body.
///
/// `headers` holds only the headers specific to this response, such as
/// `Allow`. The fixed ones are written by [`Response::to_bytes`].
pub struct Response {
  pub status: Status,
  pub headers: Vec<Header>,
  pub body: Vec<u8>,
}

impl Response {
  /// Builds a response whose body is a small page naming status.
  pub fn from_status(status: Status) -> Response {
    Response {
      status,
      headers: Vec::new(),
      body: error_body(status),
    }
  }

  /// Serialises the response as it is sent on the wire.
  ///
  /// Writes the status line, the `Content-Type`, `Content-Length` and
  /// `Connection: close` headers, then the extra headers in order, a blank
  /// line, and the body. `Content-Length` is the body's length in bytes.
  pub fn to_bytes(&self) -> Vec<u8> {
    let mut headers = String::new();
    for header in &self.headers {
      let _ = write!(
        headers,
        "{}: {}\r\n",
        header.name,
        String::from_utf8_lossy(&header.value)
      );
    }

    let mut response_text = format!(
      "HTTP/1.1 {} {}\r\n\
     Content-Type: text/html; charset=utf-8\r\n\
     Content-Length: {}\r\n\
     Connection: close\r\n\
     {headers}\
     \r\n",
      self.status.code(),
      self.status.reason(),
      self.body.len()
    )
    .into_bytes();
    response_text.extend_from_slice(&self.body);
    response_text
  }
}

/// Builds an HTML page that echoes the request head back to the client.
///
/// The method, target and version are inserted exactly as they were parsed,
/// followed by the header fields in the order they were received. Header
/// values are raw bytes, so any that are not valid UTF-8 are shown with
/// replacement characters.
//
// TODO: escape the values for HTML before this is reachable from the public
// address. The target, method and header fields are client-controlled.
pub fn echo_body(request: &Request) -> Vec<u8> {
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

/// Builds an HTML page showing the code and reason phrase of status.
pub fn error_body(status: Status) -> Vec<u8> {
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
  use crate::method::Method;
  use crate::target::Target;
  use crate::version::Version;

  #[test]
  fn echo_body_contains_request_line_parts() {
    let request = Request {
      method: Method::Get,
      target: Target::Origin("/a?b=c".to_owned()),
      version: Version { major: 1, minor: 1 },
      headers: Vec::new(),
    };
    let body = echo_body(&request);
    let body = String::from_utf8_lossy(&body);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("<p>Method: GET</p>"));
    assert!(body.contains("<p>Target: /a?b=c</p>"));
    assert!(body.contains("<p>Version: HTTP/1.1</p>"));
  }

  #[test]
  fn from_status_keeps_the_status() {
    let response = Response::from_status(Status::BadRequest);
    assert!(matches!(response.status, Status::BadRequest));
  }

  #[test]
  fn from_status_body_shows_code_and_reason() {
    let response = Response::from_status(Status::BadRequest);
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("<p>Error Code: 400</p>"));
    assert!(body.contains("<p>Reason: Bad Request</p>"));
  }

  #[test]
  fn to_bytes_writes_head_then_body() {
    let response = Response {
      status: Status::Ok,
      headers: Vec::new(),
      body: b"hello".to_vec(),
    };
    let bytes = response.to_bytes();
    assert_eq!(
      String::from_utf8_lossy(&bytes),
      "HTTP/1.1 200 OK\r\n\
       Content-Type: text/html; charset=utf-8\r\n\
       Content-Length: 5\r\n\
       Connection: close\r\n\
       \r\n\
       hello"
    );
  }

  #[test]
  fn to_bytes_counts_content_length_in_bytes() {
    let response = Response {
      status: Status::Ok,
      headers: Vec::new(),
      body: "\u{e9}".as_bytes().to_vec(),
    };
    let bytes = response.to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("Content-Length: 2\r\n"));
  }

  #[test]
  fn to_bytes_uses_the_status_for_the_status_line() {
    let bytes = Response::from_status(Status::BadRequest).to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
  }

  #[test]
  fn to_bytes_writes_extra_headers_before_the_blank_line() {
    let response = Response {
      status: Status::MethodNotAllowed,
      headers: vec![Header {
        name: "Allow".to_owned(),
        value: b"GET".to_vec(),
      }],
      body: b"hi".to_vec(),
    };
    let bytes = response.to_bytes();
    assert_eq!(
      String::from_utf8_lossy(&bytes),
      "HTTP/1.1 405 Method Not Allowed\r\n\
       Content-Type: text/html; charset=utf-8\r\n\
       Content-Length: 2\r\n\
       Connection: close\r\n\
       Allow: GET\r\n\
       \r\n\
       hi"
    );
  }

  #[test]
  fn to_bytes_writes_extra_headers_in_order() {
    let response = Response {
      status: Status::Ok,
      headers: vec![
        Header {
          name: "X-First".to_owned(),
          value: b"1".to_vec(),
        },
        Header {
          name: "X-Second".to_owned(),
          value: b"2".to_vec(),
        },
      ],
      body: Vec::new(),
    };
    let bytes = response.to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\r\nX-First: 1\r\nX-Second: 2\r\n\r\n"));
  }
}
