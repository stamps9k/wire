//! Shared character rules from the HTTP grammar (RFC 9110 and RFC 9112).
//!
//! Small byte-level checks such as `tchar` and `token`, used by the parsers
//! for the request line and header fields. They check syntax only; whether a
//! value is supported is decided elsewhere.

fn is_tchar(b: u8) -> bool {
  matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'#' | b'$' |
  b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' |
  b'|' | b'~')
}

pub fn is_token(b: &[u8]) -> bool {
  !b.is_empty() && b.iter().all(|bb| is_tchar(*bb))
}
