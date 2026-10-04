use std::fmt;
use std::str;

use crate::request_error::RequestError::{self, Malformed};

pub enum Target {
  Origin(String),    // "/search?q=foo"
  Absolute(String),  // "http://example.com/x"
  Authority(String), // "example.com:443"
  Asterisk,          // "*" — nothing to store
}

impl Target {
  pub fn parse(target_raw: &[u8]) -> Result<Target, RequestError> {
    //First reject empty strings and non ascii characters
    if target_raw.is_empty() || !target_raw.iter().all(u8::is_ascii_graphic) {
      return Err(Malformed("target is empty or has a non-visible byte"));
    }

    let t_contents = str::from_utf8(target_raw)
      .map_err(|_| Malformed("target is not ASCII"))?
      .to_owned();

    let target: Target = match target_raw {
      b"*" => Target::Asterisk,
      t if t.starts_with(b"/") => Target::Origin(t_contents),
      t if Target::is_absolute(t) => Target::Absolute(t_contents),
      t if Target::is_authority(t) => Target::Authority(t_contents),
      _ => return Err(Malformed("target matches no known form")),
    };

    Ok(target)
  }

  fn is_absolute(s: &[u8]) -> bool {
    // Split at the :// separator.
    let Some(i) = s.iter().position(|&b| b == b':') else {
      return false;
    };
    let Some((scheme, rest)) = s.split_at_checked(i) else {
      return false;
    };
    let Some(rest) = rest.strip_prefix(b"://") else {
      return false;
    };

    let scheme_check = Target::is_scheme(scheme);
    let rest_check = !rest.is_empty();

    scheme_check && rest_check
  }

  fn is_scheme(s: &[u8]) -> bool {
    /* first is a letter, every byte in rest is allowed */
    match s {
      [b'a'..=b'z' | b'A'..=b'Z', rest @ ..] => rest
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')),
      _ => false,
    }
  }

  fn is_authority(s: &[u8]) -> bool {
    let Some(i) = s.iter().rposition(|&b| b == b':') else {
      return false;
    };
    let Some((host, rest)) = s.split_at_checked(i) else {
      return false;
    };
    let Some(port) = rest.strip_prefix(b":") else {
      return false;
    };

    let port_ok = !port.is_empty() && port.iter().all(u8::is_ascii_digit);

    let host_ok =
      match host.strip_prefix(b"[").and_then(|h| h.strip_suffix(b"]")) {
        // Bracketed IPv6 literal: loose check, fine for classification.
        Some(ip6) => {
          !ip6.is_empty()
            && ip6
              .iter()
              .all(|&b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        }
        // Hostname or IPv4.
        None => {
          !host.is_empty() && host.iter().copied().all(Target::is_reg_name_char)
        }
      };

    port_ok && host_ok
  }

  fn is_reg_name_char(b: u8) -> bool {
    matches!(b,
      b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
      | b'-' | b'.' | b'_' | b'~' | b'%'
      | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
      | b'*' | b'+' | b',' | b';' | b'='
    )
  }
}

impl fmt::Display for Target {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      Self::Origin(t) | Self::Absolute(t) | Self::Authority(t) => t,
      Self::Asterisk => "*",
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn valid_origin() {
    assert!(matches!(
      Target::parse(b"/example"),
      Ok(Target::Origin(ref m)) if m == "/example"
    ));
  }

  #[test]
  fn invalid_origin_noslash() {
    assert!(Target::parse(b"example").is_err());
  }

  #[test]
  fn invalid_origin_nonascii() {
    assert!(Target::parse("/é".as_bytes()).is_err());
  }

  #[test]
  fn invalid_origin_invalid_utf8() {
    assert!(Target::parse(b"/\xFF").is_err());
  }

  #[test]
  fn invalid_absolute_invalid_utf8() {
    assert!(Target::parse(b"http://ex\xFEample.com").is_err());
  }

  #[test]
  fn invalid_authority_invalid_utf8() {
    assert!(Target::parse(b"ex\xC3ample.com:443").is_err());
  }

  #[test]
  fn invalid_origin_control_byte() {
    assert!(Target::parse(b"/foo\x01").is_err());
  }

  #[test]
  fn valid_absolute() {
    assert!(matches!(
      Target::parse(b"http://example.com"),
      Ok(Target::Absolute(ref m)) if m == "http://example.com"
    ));
  }

  #[test]
  fn invalid_absolute_malformed_scheme() {
    assert!(Target::parse(b"h@p://x").is_err());
  }

  #[test]
  fn invalid_absolute_numeric_start() {
    assert!(Target::parse(b"1http://example.com").is_err());
  }

  #[test]
  fn invalid_absolute_empty_rest() {
    assert!(Target::parse(b"http://").is_err());
  }

  #[test]
  fn valid_absolute_scheme_symbols() {
    assert!(matches!(
      Target::parse(b"a+b-c.d://x"),
      Ok(Target::Absolute(ref m)) if m == "a+b-c.d://x"
    ));
  }

  #[test]
  fn valid_absolute_uppercase_scheme() {
    assert!(matches!(
      Target::parse(b"HTTP://example.com:443"),
      Ok(Target::Absolute(ref m)) if m == "HTTP://example.com:443"
    ));
  }

  #[test]
  fn valid_authority_alphanumeric() {
    assert!(matches!(
      Target::parse(b"example.com:443"),
      Ok(Target::Authority(ref m)) if m == "example.com:443"
    ));
  }

  #[test]
  fn valid_authority_ipv6() {
    assert!(matches!(
      Target::parse(b"[::1]:8080"),
      Ok(Target::Authority(ref m)) if m == "[::1]:8080"
    ));
  }

  #[test]
  fn invalid_authority_noport() {
    assert!(Target::parse(b"example.com:").is_err());
  }

  #[test]
  fn invalid_authority_malformed_port() {
    assert!(Target::parse(b"example.com:$").is_err());
  }

  #[test]
  fn invalid_authority_nohost() {
    assert!(Target::parse(b":443").is_err());
  }

  #[test]
  fn invalid_authority_malformed_host() {
    assert!(Target::parse(b"@:443").is_err());
  }

  #[test]
  fn valid_asterisk() {
    assert!(matches!(Target::parse(b"*"), Ok(Target::Asterisk)));
  }

  #[test]
  fn invalid_asterisk() {
    assert!(Target::parse(b"*x").is_err());
  }

  #[test]
  fn invalid_empty_string() {
    assert!(Target::parse(b"").is_err());
  }

  #[test]
  fn display_origin() {
    assert_eq!(Target::Origin("/a?b=c".to_owned()).to_string(), "/a?b=c");
  }

  #[test]
  fn display_asterisk() {
    assert_eq!(Target::Asterisk.to_string(), "*");
  }
}
