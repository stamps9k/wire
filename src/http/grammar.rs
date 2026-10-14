//! Shared character rules from the HTTP grammar (RFC 9110 and RFC 9112).
//!
//! Small byte-level checks such as `tchar` and `token`, used by the parsers
//! for the request line and header fields. They check syntax only; whether a
//! value is supported is decided elsewhere.

/// Returns whether `b` is a `tchar`: a byte allowed in a token.
fn is_tchar(b: u8) -> bool {
  matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'#' | b'$' |
  b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' |
  b'|' | b'~')
}

/// Returns whether `b` is a valid token: one or more `tchar` bytes.
///
/// Methods and header field names are tokens.
pub fn is_token(b: &[u8]) -> bool {
  !b.is_empty() && b.iter().copied().all(is_tchar)
}

/// Returns whether every byte of `b` is allowed in a header field value:
/// visible ASCII, space, horizontal tab, or obs-text (0x80–0xFF).
///
/// An empty value is valid.
pub fn is_field_value(b: &[u8]) -> bool {
  b.iter()
    .all(|&c| matches!(c, b'\t' | b' ' | 0x21..=0x7E | 0x80..=0xFF))
}

/// Trims optional whitespace (OWS) from both ends of `b`.
///
/// OWS in the HTTP grammar is only space and horizontal tab. The standard
/// library's `trim_ascii` also strips CR, LF and form feed, which must be
/// rejected in a field value rather than silently removed.
pub fn trim_ows(mut b: &[u8]) -> &[u8] {
  while let [b' ' | b'\t', rest @ ..] = b {
    b = rest;
  }
  while let [rest @ .., b' ' | b'\t'] = b {
    b = rest;
  }
  b
}

/// Splits `buf` at the first occurrence of `delim`.
///
/// Returns the bytes before and after the delimiter, without the delimiter
/// itself, or `None` if `delim` is empty or does not occur in `buf`.
pub fn split_once<'a>(
  buf: &'a [u8],
  delim: &[u8],
) -> Option<(&'a [u8], &'a [u8])> {
  if delim.is_empty() {
    return None;
  }

  let i = buf.windows(delim.len()).position(|w| w == delim)?;
  let (before, rest) = buf.split_at_checked(i)?;
  let after = rest.strip_prefix(delim)?;
  Some((before, after))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn token_accepts_letters_and_digits() {
    assert!(is_token(b"Content-Length2"));
  }

  #[test]
  fn token_accepts_all_symbols() {
    assert!(is_token(b"!#$%&'*+-.^_`|~"));
  }

  #[test]
  fn token_rejects_empty() {
    assert!(!is_token(b""));
  }

  #[test]
  fn token_rejects_separators_and_whitespace() {
    for &c in b"()<>@,;:\\\"/[]?={} \t" {
      assert!(!is_token(&[c]), "byte {c:#04x} should not be a tchar");
    }
  }

  #[test]
  fn token_rejects_control_and_high_bytes() {
    assert!(!is_token(b"a\x00b"));
    assert!(!is_token(b"a\x7Fb"));
    assert!(!is_token(b"a\x80b"));
    assert!(!is_token(b"a\xFFb"));
  }

  #[test]
  fn field_value_accepts_empty() {
    assert!(is_field_value(b""));
  }

  #[test]
  fn field_value_accepts_visible_ascii_space_and_tab() {
    assert!(is_field_value(b"text/html; q=0.9,\t*/*"));
  }

  #[test]
  fn field_value_accepts_obs_text() {
    assert!(is_field_value(b"caf\xE9 \x80\xFF"));
  }

  #[test]
  fn field_value_rejects_control_bytes() {
    assert!(!is_field_value(b"a\x00b"));
    assert!(!is_field_value(b"a\rb"));
    assert!(!is_field_value(b"a\nb"));
    assert!(!is_field_value(b"a\x7Fb"));
  }

  #[test]
  fn trim_ows_strips_both_ends() {
    assert_eq!(trim_ows(b" \t value\t "), b"value");
  }

  #[test]
  fn trim_ows_keeps_inner_whitespace() {
    assert_eq!(trim_ows(b" a \t b "), b"a \t b");
  }

  #[test]
  fn trim_ows_all_whitespace_gives_empty() {
    assert_eq!(trim_ows(b" \t \t"), b"");
  }

  #[test]
  fn trim_ows_empty_stays_empty() {
    assert_eq!(trim_ows(b""), b"");
  }

  #[test]
  fn trim_ows_leaves_cr_and_lf() {
    assert_eq!(trim_ows(b"\r a"), b"\r a");
    assert_eq!(trim_ows(b"a \n"), b"a \n");
  }

  #[test]
  fn split_at_first_delimiter_only() {
    assert_eq!(
      split_once(b"a\r\nb\r\nc", b"\r\n"),
      Some((b"a".as_slice(), b"b\r\nc".as_slice()))
    );
  }

  #[test]
  fn split_single_byte_delimiter() {
    assert_eq!(
      split_once(b"Host: a:8080", b":"),
      Some((b"Host".as_slice(), b" a:8080".as_slice()))
    );
  }

  #[test]
  fn split_delimiter_at_start() {
    assert_eq!(
      split_once(b"\r\nrest", b"\r\n"),
      Some((b"".as_slice(), b"rest".as_slice()))
    );
  }

  #[test]
  fn split_delimiter_at_end() {
    assert_eq!(
      split_once(b"line\r\n", b"\r\n"),
      Some((b"line".as_slice(), b"".as_slice()))
    );
  }

  #[test]
  fn split_returns_none_when_not_found() {
    assert_eq!(split_once(b"abc", b":"), None);
    assert_eq!(split_once(b"", b":"), None);
  }

  #[test]
  fn split_returns_none_for_partial_delimiter() {
    assert_eq!(split_once(b"a\rb", b"\r\n"), None);
    assert_eq!(split_once(b"a", b"\r\n"), None);
  }

  #[test]
  fn split_returns_none_for_empty_delimiter() {
    assert_eq!(split_once(b"abc", b""), None);
  }
}
