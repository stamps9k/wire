use crate::grammar::split_once;
use crate::request_error::RequestError::Malformed;

use super::header::Header;
use super::method::Method;
use super::request_error::RequestError;
use super::target::Target;
use super::version::Version;

/// A parsed HTTP request head: the request line and the header fields.
pub struct Request {
  pub method: Method,
  pub target: Target,
  pub version: Version,
  pub headers: Vec<Header>,
}

/// Parses the header section that follows the request line.
///
/// Reads field lines until the blank line that ends the section. Any bytes
/// after the blank line are ignored.
///
/// # Errors
///
/// Returns [`RequestError::Malformed`] if a field line is invalid or the
/// input ends before the blank line.
fn parse_headers(mut headers_raw: &[u8]) -> Result<Vec<Header>, RequestError> {
  let mut headers: Vec<Header> = Vec::new();

  loop {
    let (header, rest) = split_once(headers_raw, b"\r\n")
      .ok_or(Malformed("header section not terminated"))?;
    headers_raw = rest;

    if header.is_empty() {
      break; // blank line: end of header section
    }

    headers.push(Header::parse(header)?);
  }

  Ok(headers)
}

impl Request {
  /// Parses a request head: the request line, then the header fields.
  ///
  /// `request_raw` must start at the request line and include the blank line
  /// that ends the header section. Checks syntax only; it does not decide
  /// whether the method, target or version is supported.
  ///
  /// # Errors
  ///
  /// Returns [`RequestError::Malformed`] if the request line or any field
  /// line breaks the HTTP grammar, or if the head is not terminated.
  pub fn parse(request_raw: &[u8]) -> Result<Request, RequestError> {
    let (request_line, rest) = split_once(request_raw, b"\r\n")
      .ok_or(Malformed("request line not terminated"))?;

    let mut request_line_split = request_line.split(|b| *b == b' ');
    let method_raw = request_line_split
      .next()
      .ok_or(Malformed("request line has no method"))?;

    let target_raw = request_line_split
      .next()
      .ok_or(Malformed("request line has no target"))?;

    let version_raw = request_line_split
      .next()
      .ok_or(Malformed("request line has no version"))?;

    //Check that the request did not include extra invalid information
    if request_line_split.next().is_some() {
      return Err(Malformed("request line has extra fields"));
    }

    Ok(Request {
      method: Method::parse(method_raw)?,
      target: Target::parse(target_raw)?,
      version: Version::parse(version_raw)?,
      headers: parse_headers(rest)?,
    })
  }

  /// Checks the rules that apply to the request as a whole, beyond the
  /// grammar that [`Request::parse`] has already checked.
  ///
  /// # Errors
  ///
  /// Returns [`RequestError::Malformed`] if any rule is broken.
  pub fn validate(&self) -> Result<(), RequestError> {
    self.validate_host()?;
    self.validate_target_form()?;

    Ok(())
  }

  /// Checks the number of Host headers (RFC 9112 section 3.2).
  ///
  /// An HTTP/1.0 request may omit Host. Every other version needs exactly
  /// one, and no version may repeat it.
  ///
  /// # Errors
  ///
  /// Returns [`RequestError::Malformed`] if Host is repeated, or if it is
  /// missing from a request that is not HTTP/1.0.
  fn validate_host(&self) -> Result<(), RequestError> {
    let host_headers = self.headers.iter().filter(|h| h.name == "host");

    //Check for all versions that host headers does not exceed 1
    if host_headers.clone().count() > 1 {
      return Err(Malformed("host header repeated"));
    }

    //Check for all versions outside 1.0 that host headers is not 0
    let is_v1_0 = self.version.major == 1 && self.version.minor == 0;
    if !is_v1_0 && host_headers.count() == 0 {
      return Err(Malformed("host header missing"));
    }

    Ok(())
  }

  /// Checks that the target form fits the method (RFC 9112 section 3.2).
  ///
  /// CONNECT must use authority-form and no other method may. Asterisk-form
  /// is only for OPTIONS. Origin-form and absolute-form suit every method
  /// except CONNECT.
  ///
  /// # Errors
  ///
  /// Returns [`RequestError::Malformed`] if the method and target form do
  /// not go together.
  fn validate_target_form(&self) -> Result<(), RequestError> {
    match (&self.method, &self.target) {
      (Method::Connect, Target::Authority(_))
      | (Method::Options, Target::Asterisk) => Ok(()),
      (Method::Connect, _) => {
        Err(Malformed("connect target is not authority-form"))
      }
      (_, Target::Authority(_)) => {
        Err(Malformed("authority-form target without connect"))
      }
      (_, Target::Asterisk) => {
        Err(Malformed("asterisk-form target without options"))
      }
      (_, _) => Ok(()),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Parses a head and returns its headers as pairs, or `None` on error.
  fn headers_of(raw: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    Request::parse(raw)
      .ok()
      .map(|r| r.headers.into_iter().map(|h| (h.name, h.value)).collect())
  }

  fn pair(name: &str, value: &[u8]) -> (String, Vec<u8>) {
    (name.to_owned(), value.to_vec())
  }

  #[test]
  fn parses_request_line_and_header() {
    assert!(matches!(
      Request::parse(b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n"),
      Ok(Request {
        method: Method::Get,
        target: Target::Origin(ref path),
        version: Version { major: 1, minor: 1 },
        ref headers,
      }) if path == "/index.html" && headers.len() == 1
    ));
  }

  #[test]
  fn no_headers_is_valid_syntax() {
    assert_eq!(headers_of(b"GET / HTTP/1.1\r\n\r\n"), Some(vec![]));
  }

  #[test]
  fn keeps_headers_in_order() {
    assert_eq!(
      headers_of(
        b"GET / HTTP/1.1\r\nHost: a\r\nAccept: */*\r\nX-One: 1\r\n\r\n"
      ),
      Some(vec![
        pair("host", b"a"),
        pair("accept", b"*/*"),
        pair("x-one", b"1")
      ])
    );
  }

  #[test]
  fn keeps_duplicate_headers_separate() {
    assert_eq!(
      headers_of(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"),
      Some(vec![pair("host", b"a"), pair("host", b"b")])
    );
  }

  #[test]
  fn ignores_bytes_after_blank_line() {
    assert_eq!(
      headers_of(b"POST /x HTTP/1.1\r\nHost: a\r\n\r\nbody: not-a-header\r\n"),
      Some(vec![pair("host", b"a")])
    );
  }

  #[test]
  fn rejects_empty_input() {
    assert!(Request::parse(b"").is_err());
  }

  #[test]
  fn rejects_request_line_without_crlf() {
    assert!(Request::parse(b"GET / HTTP/1.1").is_err());
  }

  #[test]
  fn rejects_bare_lf_line_endings() {
    assert!(Request::parse(b"GET / HTTP/1.1\nHost: a\n\n").is_err());
  }

  #[test]
  fn rejects_missing_version() {
    assert!(Request::parse(b"GET /\r\n\r\n").is_err());
  }

  #[test]
  fn rejects_extra_request_line_field() {
    assert!(Request::parse(b"GET / HTTP/1.1 extra\r\n\r\n").is_err());
  }

  #[test]
  fn rejects_double_space_in_request_line() {
    assert!(Request::parse(b"GET  / HTTP/1.1\r\n\r\n").is_err());
  }

  #[test]
  fn rejects_malformed_header() {
    assert!(Request::parse(b"GET / HTTP/1.1\r\nHost : a\r\n\r\n").is_err());
  }

  #[test]
  fn rejects_obs_fold_continuation_line() {
    assert!(Request::parse(b"GET / HTTP/1.1\r\nX: a\r\n b\r\n\r\n").is_err());
  }

  // The next two fail until parse_headers treats a missing blank line as an
  // error (loop + ok_or instead of while let).
  #[test]
  fn rejects_unterminated_header_section() {
    assert!(Request::parse(b"GET / HTTP/1.1\r\nHost: a\r\n").is_err());
  }

  #[test]
  fn rejects_header_without_line_ending() {
    assert!(Request::parse(b"GET / HTTP/1.1\r\nHost: a").is_err());
  }

  /// Parses a head and runs `validate` on the result.
  fn validated(raw: &[u8]) -> Result<(), RequestError> {
    Request::parse(raw)?.validate()
  }

  #[test]
  fn single_host_is_valid() {
    assert!(validated(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
  }

  #[test]
  fn host_name_case_is_ignored() {
    assert!(validated(b"GET / HTTP/1.1\r\nhOsT: a\r\n\r\n").is_ok());
  }

  #[test]
  fn missing_host_on_http_1_1_is_rejected() {
    assert!(matches!(
      validated(b"GET / HTTP/1.1\r\nAccept: */*\r\n\r\n"),
      Err(Malformed("host header missing"))
    ));
  }

  #[test]
  fn missing_host_on_http_1_2_is_rejected() {
    assert!(matches!(
      validated(b"GET / HTTP/1.2\r\n\r\n"),
      Err(Malformed("host header missing"))
    ));
  }

  #[test]
  fn missing_host_on_http_1_0_is_valid() {
    assert!(validated(b"GET / HTTP/1.0\r\nAccept: */*\r\n\r\n").is_ok());
  }

  #[test]
  fn single_host_on_http_1_0_is_valid() {
    assert!(validated(b"GET / HTTP/1.0\r\nHost: a\r\n\r\n").is_ok());
  }

  #[test]
  fn repeated_host_is_rejected() {
    assert!(matches!(
      validated(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"),
      Err(Malformed("host header repeated"))
    ));
  }

  #[test]
  fn repeated_host_in_mixed_case_is_rejected() {
    assert!(matches!(
      validated(b"GET / HTTP/1.1\r\nHost: a\r\nHOST: a\r\n\r\n"),
      Err(Malformed("host header repeated"))
    ));
  }

  #[test]
  fn repeated_host_on_http_1_0_is_rejected() {
    assert!(matches!(
      validated(b"GET / HTTP/1.0\r\nHost: a\r\nHost: b\r\n\r\n"),
      Err(Malformed("host header repeated"))
    ));
  }

  #[test]
  fn connect_with_authority_is_valid() {
    assert!(validated(b"CONNECT a:443 HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
  }

  #[test]
  fn connect_with_origin_is_rejected() {
    assert!(matches!(
      validated(b"CONNECT / HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("connect target is not authority-form"))
    ));
  }

  #[test]
  fn connect_with_absolute_is_rejected() {
    assert!(matches!(
      validated(b"CONNECT http://a/ HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("connect target is not authority-form"))
    ));
  }

  #[test]
  fn connect_with_asterisk_is_rejected() {
    assert!(matches!(
      validated(b"CONNECT * HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("connect target is not authority-form"))
    ));
  }

  #[test]
  fn get_with_authority_is_rejected() {
    assert!(matches!(
      validated(b"GET a:443 HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("authority-form target without connect"))
    ));
  }

  #[test]
  fn options_with_authority_is_rejected() {
    assert!(matches!(
      validated(b"OPTIONS a:443 HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("authority-form target without connect"))
    ));
  }

  #[test]
  fn other_method_with_authority_is_rejected() {
    assert!(matches!(
      validated(b"PURGE a:443 HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("authority-form target without connect"))
    ));
  }

  #[test]
  fn options_with_asterisk_is_valid() {
    assert!(validated(b"OPTIONS * HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
  }

  #[test]
  fn options_with_origin_is_valid() {
    assert!(validated(b"OPTIONS /x HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
  }

  #[test]
  fn get_with_asterisk_is_rejected() {
    assert!(matches!(
      validated(b"GET * HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("asterisk-form target without options"))
    ));
  }

  #[test]
  fn other_method_with_asterisk_is_rejected() {
    assert!(matches!(
      validated(b"PURGE * HTTP/1.1\r\nHost: a\r\n\r\n"),
      Err(Malformed("asterisk-form target without options"))
    ));
  }

  #[test]
  fn get_with_absolute_is_valid() {
    assert!(validated(b"GET http://a/ HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
  }

  #[test]
  fn other_method_with_origin_is_valid() {
    assert!(validated(b"PURGE / HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
  }
}
