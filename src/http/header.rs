use std::str;

use crate::http::RequestError;
use crate::http::RequestError::Malformed;
use crate::http::grammar::{is_field_value, is_token, split_once, trim_ows};

/// One header field from a request.
///
/// `name` is a validated token, lowercased so lookups can compare directly.
/// `value` is the raw bytes with surrounding whitespace removed; it may
/// contain obs-text bytes (0x80–0xFF), so it is not guaranteed to be UTF-8.
pub struct Header {
  pub name: String,   // validated token, lowercased
  pub value: Vec<u8>, // raw bytes, OWS trimmed
}
impl Header {
  /// Parses one field line, without its trailing CRLF.
  ///
  /// Splits at the first colon, so the value may itself contain colons.
  ///
  /// # Errors
  ///
  /// Returns [`RequestError::Malformed`] if the line has no colon, the name
  /// is not a token (which also rejects whitespace before the colon and
  /// obsolete line folding), or the value contains a control byte.
  pub fn parse(header: &[u8]) -> Result<Header, RequestError> {
    let (name_raw, value_raw) =
      split_once(header, b":").ok_or(Malformed("field line has no colon"))?;

    if !is_token(name_raw) {
      return Err(Malformed("field name is not a token"));
    }

    if !is_field_value(value_raw) {
      return Err(Malformed("field value has an invalid byte"));
    }

    Ok(Header {
      name: str::from_utf8(name_raw)
        .map_err(|_| Malformed("field name is not ASCII"))?
        .to_ascii_lowercase(),
      value: trim_ows(value_raw).to_vec(),
    })
  }

  /// Reports whether this field's value lists `token`.
  ///
  /// The value is read as a comma-separated list (RFC 9110 section 5.6.1).
  /// Whitespace around an element is ignored and the comparison ignores
  /// case.
  pub fn has_token(&self, token: &[u8]) -> bool {
    self
      .value
      .split(|&b| b == b',')
      .any(|element| trim_ows(element).eq_ignore_ascii_case(token))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Parses a field line and returns its name and value, or `None` on error.
  fn parsed(line: &[u8]) -> Option<(String, Vec<u8>)> {
    Header::parse(line).ok().map(|h| (h.name, h.value))
  }

  fn pair(name: &str, value: &[u8]) -> (String, Vec<u8>) {
    (name.to_owned(), value.to_vec())
  }

  #[test]
  fn valid_simple() {
    assert_eq!(
      parsed(b"Host: example.com"),
      Some(pair("host", b"example.com"))
    );
  }

  #[test]
  fn name_is_lowercased() {
    assert_eq!(
      parsed(b"CoNtEnT-TyPe: text/html"),
      Some(pair("content-type", b"text/html"))
    );
  }

  #[test]
  fn value_case_is_preserved() {
    assert_eq!(parsed(b"X-Token: AbCdEf"), Some(pair("x-token", b"AbCdEf")));
  }

  #[test]
  fn valid_without_space_after_colon() {
    assert_eq!(
      parsed(b"Host:example.com"),
      Some(pair("host", b"example.com"))
    );
  }

  #[test]
  fn trims_ows_around_value() {
    assert_eq!(
      parsed(b"Host: \t example.com \t "),
      Some(pair("host", b"example.com"))
    );
  }

  #[test]
  fn keeps_inner_whitespace_in_value() {
    assert_eq!(
      parsed(b"User-Agent: a  b\tc"),
      Some(pair("user-agent", b"a  b\tc"))
    );
  }

  #[test]
  fn empty_value_is_allowed() {
    assert_eq!(parsed(b"X-Empty:"), Some(pair("x-empty", b"")));
  }

  #[test]
  fn whitespace_only_value_becomes_empty() {
    assert_eq!(parsed(b"X-Empty:  \t "), Some(pair("x-empty", b"")));
  }

  #[test]
  fn splits_at_first_colon_only() {
    assert_eq!(
      parsed(b"Host: example.com:8080"),
      Some(pair("host", b"example.com:8080"))
    );
  }

  #[test]
  fn obs_text_in_value_is_kept() {
    assert_eq!(parsed(b"X-Name: caf\xE9"), Some(pair("x-name", b"caf\xE9")));
  }

  #[test]
  fn rejects_missing_colon() {
    assert!(Header::parse(b"Host example.com").is_err());
  }

  #[test]
  fn rejects_empty_line() {
    assert!(Header::parse(b"").is_err());
  }

  #[test]
  fn rejects_empty_name() {
    assert!(Header::parse(b": value").is_err());
  }

  #[test]
  fn rejects_space_before_colon() {
    assert!(Header::parse(b"Host : example.com").is_err());
  }

  #[test]
  fn rejects_leading_whitespace_obs_fold() {
    assert!(Header::parse(b" Host: example.com").is_err());
    assert!(Header::parse(b"\tcontinued: value").is_err());
  }

  #[test]
  fn rejects_whitespace_inside_name() {
    assert!(Header::parse(b"Ho st: x").is_err());
    assert!(Header::parse(b"Ho\tst: x").is_err());
  }

  #[test]
  fn rejects_non_ascii_name() {
    assert!(Header::parse("Hé: x".as_bytes()).is_err());
    assert!(Header::parse(b"H\xFFst: x").is_err());
  }

  #[test]
  fn rejects_nul_in_value() {
    assert!(Header::parse(b"X: a\x00b").is_err());
  }

  #[test]
  fn rejects_bare_cr_in_value() {
    assert!(Header::parse(b"X: a\rb").is_err());
  }

  #[test]
  fn rejects_bare_lf_in_value() {
    assert!(Header::parse(b"X: a\nb").is_err());
  }

  #[test]
  fn rejects_del_in_value() {
    assert!(Header::parse(b"X: a\x7Fb").is_err());
  }

  /// Reports whether a `Connection` header with this value lists `token`.
  fn lists(value: &[u8], token: &[u8]) -> bool {
    let header = Header {
      name: "connection".to_owned(),
      value: value.to_vec(),
    };
    header.has_token(token)
  }

  #[test]
  fn has_token_finds_a_single_token() {
    assert!(lists(b"upgrade", b"upgrade"));
  }

  #[test]
  fn has_token_finds_a_token_anywhere_in_a_list() {
    assert!(lists(b"upgrade, keep-alive", b"upgrade"));
    assert!(lists(b"keep-alive, upgrade", b"upgrade"));
    assert!(lists(b"a, upgrade, b", b"upgrade"));
  }

  #[test]
  fn has_token_ignores_case() {
    assert!(lists(b"Upgrade", b"upgrade"));
    assert!(lists(b"UPGRADE", b"upgrade"));
    assert!(lists(b"upgrade", b"UpGrAdE"));
  }

  #[test]
  fn has_token_ignores_whitespace_around_elements() {
    assert!(lists(b"keep-alive ,upgrade", b"upgrade"));
    assert!(lists(b"keep-alive,  upgrade  ", b"upgrade"));
    assert!(lists(b"keep-alive,\tupgrade", b"upgrade"));
    assert!(lists(b"keep-alive, \t upgrade \t", b"upgrade"));
  }

  #[test]
  fn has_token_tolerates_empty_elements() {
    assert!(lists(b",upgrade", b"upgrade"));
    assert!(lists(b"upgrade,", b"upgrade"));
    assert!(lists(b"keep-alive, , upgrade", b"upgrade"));
    assert!(lists(b",,upgrade,,", b"upgrade"));
  }

  #[test]
  fn has_token_needs_a_whole_element() {
    assert!(!lists(b"upgrades", b"upgrade"));
    assert!(!lists(b"no-upgrade", b"upgrade"));
    assert!(!lists(b"upgrade-later", b"upgrade"));
  }

  #[test]
  fn has_token_keeps_inner_whitespace() {
    assert!(!lists(b"up grade", b"upgrade"));
  }

  #[test]
  fn has_token_rejects_an_absent_token() {
    assert!(!lists(b"keep-alive", b"upgrade"));
    assert!(!lists(b"keep-alive, close", b"upgrade"));
  }

  #[test]
  fn has_token_rejects_an_empty_value() {
    assert!(!lists(b"", b"upgrade"));
    assert!(!lists(b" , ,", b"upgrade"));
  }

  #[test]
  fn has_token_handles_bytes_that_are_not_utf8() {
    assert!(lists(b"caf\xE9, upgrade", b"upgrade"));
    assert!(!lists(b"upgrad\xE9", b"upgrade"));
  }
}
