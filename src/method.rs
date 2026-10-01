use super::request::is_token;
use std::io::Error;

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
  pub fn parse(method_raw: &str) -> Result<Method, Error> {
    if !is_token(method_raw) {
      return Err(Error::other("Malformed Request"));
    }

    let method = match method_raw {
      "GET" => Method::Get,
      "HEAD" => Method::Head,
      "POST" => Method::Post,
      "PUT" => Method::Put,
      "DELETE" => Method::Delete,
      "CONNECT" => Method::Connect,
      "OPTIONS" => Method::Options,
      "TRACE" => Method::Trace,
      "PATCH" => Method::Patch,
      _ => Method::Other(method_raw.to_string()),
    };

    Ok(method)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn known_method_get() {
    assert!(matches!(Method::parse("GET"), Ok(Method::Get)));
  }

  #[test]
  fn known_method_head() {
    assert!(matches!(Method::parse("HEAD"), Ok(Method::Head)));
  }

  #[test]
  fn known_method_post() {
    assert!(matches!(Method::parse("POST"), Ok(Method::Post)));
  }

  #[test]
  fn known_method_put() {
    assert!(matches!(Method::parse("PUT"), Ok(Method::Put)));
  }

  #[test]
  fn known_method_delete() {
    assert!(matches!(Method::parse("DELETE"), Ok(Method::Delete)));
  }

  #[test]
  fn known_method_connect() {
    assert!(matches!(Method::parse("CONNECT"), Ok(Method::Connect)));
  }

  #[test]
  fn known_method_options() {
    assert!(matches!(Method::parse("OPTIONS"), Ok(Method::Options)));
  }

  #[test]
  fn known_method_trace() {
    assert!(matches!(Method::parse("TRACE"), Ok(Method::Trace)));
  }

  #[test]
  fn known_method_patch() {
    assert!(matches!(Method::parse("PATCH"), Ok(Method::Patch)));
  }

  #[test]
  fn lowercase_method_maps_to_other() {
    assert!(
      matches!(Method::parse("get"), Ok(Method::Other(ref m)) if m == "get")
    );
  }

  #[test]
  fn unknown_token_with_symbols_maps_to_other() {
    assert!(matches!(
      Method::parse("M-SEARCH"),
      Ok(Method::Other(ref m)) if m == "M-SEARCH"
    ));
  }

  #[test]
  fn rejects_empty_string() {
    assert!(Method::parse("").is_err());
  }

  #[test]
  fn rejects_non_token() {
    assert!(Method::parse("@").is_err());
  }

  #[test]
  fn rejects_binary() {
    assert!(Method::parse("G\tET").is_err());
  }

  #[test]
  fn rejects_non_ascii() {
    assert!(Method::parse("GÉT").is_err());
  }
}
