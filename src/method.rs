use super::grammar::is_token;
use std::fmt;
use std::str;

use super::request_error::RequestError;

pub enum Method {
  Get,
  Head,
  Post,
  Put,
  Delete,
  Connect,
  Options,
  Trace,
  Patch,
  Other(String),
}

impl Method {
  pub fn parse(method_raw: &[u8]) -> Result<Method, RequestError> {
    if !is_token(method_raw) {
      return Err(RequestError::Malformed("method is not a token"));
    }

    let method = match method_raw {
      b"GET" => Method::Get,
      b"HEAD" => Method::Head,
      b"POST" => Method::Post,
      b"PUT" => Method::Put,
      b"DELETE" => Method::Delete,
      b"CONNECT" => Method::Connect,
      b"OPTIONS" => Method::Options,
      b"TRACE" => Method::Trace,
      b"PATCH" => Method::Patch,
      _ => Method::Other(
        str::from_utf8(method_raw)
          .map_err(|_| RequestError::Malformed("method is not ASCII"))?
          .to_owned(),
      ),
    };

    Ok(method)
  }
}

impl fmt::Display for Method {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      Self::Get => "GET",
      Self::Head => "HEAD",
      Self::Post => "POST",
      Self::Put => "PUT",
      Self::Delete => "DELETE",
      Self::Connect => "CONNECT",
      Self::Options => "OPTIONS",
      Self::Trace => "TRACE",
      Self::Patch => "PATCH",
      Self::Other(m) => m,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn known_method_get() {
    assert!(matches!(Method::parse(b"GET"), Ok(Method::Get)));
  }

  #[test]
  fn known_method_head() {
    assert!(matches!(Method::parse(b"HEAD"), Ok(Method::Head)));
  }

  #[test]
  fn known_method_post() {
    assert!(matches!(Method::parse(b"POST"), Ok(Method::Post)));
  }

  #[test]
  fn known_method_put() {
    assert!(matches!(Method::parse(b"PUT"), Ok(Method::Put)));
  }

  #[test]
  fn known_method_delete() {
    assert!(matches!(Method::parse(b"DELETE"), Ok(Method::Delete)));
  }

  #[test]
  fn known_method_connect() {
    assert!(matches!(Method::parse(b"CONNECT"), Ok(Method::Connect)));
  }

  #[test]
  fn known_method_options() {
    assert!(matches!(Method::parse(b"OPTIONS"), Ok(Method::Options)));
  }

  #[test]
  fn known_method_trace() {
    assert!(matches!(Method::parse(b"TRACE"), Ok(Method::Trace)));
  }

  #[test]
  fn known_method_patch() {
    assert!(matches!(Method::parse(b"PATCH"), Ok(Method::Patch)));
  }

  #[test]
  fn lowercase_method_maps_to_other() {
    assert!(
      matches!(Method::parse(b"get"), Ok(Method::Other(ref m)) if m == "get")
    );
  }

  #[test]
  fn unknown_token_with_symbols_maps_to_other() {
    assert!(matches!(
      Method::parse(b"M-SEARCH"),
      Ok(Method::Other(ref m)) if m == "M-SEARCH"
    ));
  }

  #[test]
  fn rejects_empty_string() {
    assert!(Method::parse(b"").is_err());
  }

  #[test]
  fn rejects_non_token() {
    assert!(Method::parse(b"@").is_err());
  }

  #[test]
  fn rejects_binary() {
    assert!(Method::parse(b"G\tET").is_err());
  }

  #[test]
  fn rejects_non_ascii() {
    assert!(Method::parse("GÉT".as_bytes()).is_err());
  }

  #[test]
  fn rejects_invalid_utf8() {
    assert!(Method::parse(b"G\xFFT").is_err());
  }

  #[test]
  fn rejects_lone_continuation_byte() {
    assert!(Method::parse(b"\x80GET").is_err());
  }

  #[test]
  fn display_known_method() {
    assert_eq!(Method::Get.to_string(), "GET");
  }

  #[test]
  fn display_other_method_is_verbatim() {
    assert_eq!(Method::Other("M-SEARCH".to_owned()).to_string(), "M-SEARCH");
  }
}
