//! Responses: the message sent back to the client and its wire format.
//!
//! A [`Response`] holds a status, any extra headers and a body.
//! [`Response::to_bytes`] adds the fixed headers and serialises it. The
//! body is supplied by the caller.

use std::fmt::Write as _;

use crate::http::Header;
use crate::http::Status;

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
  /// Builds a response with `status` and `body` and no extra headers.
  pub fn new(status: Status, body: Vec<u8>) -> Response {
    Response {
      status,
      headers: Vec::new(),
      body,
    }
  }

  /// Serialises the response as it is sent on the wire.
  ///
  /// Writes the status line, the `Content-Type`, `Content-Length` and
  /// `Connection: close` headers, then the extra headers in order, a blank
  /// line, and the body. `Content-Length` is the body's length in bytes.
  ///
  /// A 101 response is the exception: it gets only the status line, the
  /// extra headers and the blank line. The fixed headers and the body are
  /// left out, because the bytes that follow belong to the new protocol.
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

    if matches!(self.status, Status::SwitchingProtocols) {
      format!(
        "HTTP/1.1 {} {}\r\n\
          {headers}\
          \r\n",
        self.status.code(),
        self.status.reason(),
      )
      .into_bytes()
    } else {
      let mut s = format!(
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
      s.extend_from_slice(&self.body);
      s
    }
  }

  /// Builds a 200 response with `body` and no extra headers.
  pub fn ok(body: Vec<u8>) -> Response {
    Response::new(Status::Ok, body)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ok_is_200_with_the_body_and_no_extra_headers() {
    let response = Response::ok(b"hello".to_vec());
    assert!(matches!(response.status, Status::Ok));
    assert!(response.headers.is_empty());
    assert_eq!(response.body, b"hello");
  }

  #[test]
  fn new_keeps_the_status_and_body_with_no_extra_headers() {
    let response = Response::new(Status::BadRequest, b"oops".to_vec());
    assert!(matches!(response.status, Status::BadRequest));
    assert!(response.headers.is_empty());
    assert_eq!(response.body, b"oops");
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
    let bytes = Response::new(Status::BadRequest, Vec::new()).to_bytes();
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

  #[test]
  fn to_bytes_writes_only_extra_headers_for_switching_protocols() {
    let response = Response {
      status: Status::SwitchingProtocols,
      headers: vec![
        Header {
          name: "Upgrade".to_owned(),
          value: b"websocket".to_vec(),
        },
        Header {
          name: "Connection".to_owned(),
          value: b"Upgrade".to_vec(),
        },
      ],
      body: Vec::new(),
    };
    let bytes = response.to_bytes();
    assert_eq!(
      String::from_utf8_lossy(&bytes),
      "HTTP/1.1 101 Switching Protocols\r\n\
       Upgrade: websocket\r\n\
       Connection: Upgrade\r\n\
       \r\n"
    );
  }

  #[test]
  fn to_bytes_writes_no_body_for_switching_protocols() {
    let response = Response {
      status: Status::SwitchingProtocols,
      headers: Vec::new(),
      body: b"hello".to_vec(),
    };
    let bytes = response.to_bytes();
    assert_eq!(
      String::from_utf8_lossy(&bytes),
      "HTTP/1.1 101 Switching Protocols\r\n\r\n"
    );
  }
}
