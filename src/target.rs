use std::io::Error;

pub enum Target {
  Origin(String),    // "/search?q=foo"
  Absolute(String),  // "http://example.com/x"
  Authority(String), // "example.com:443"
  Asterisk,          // "*" — nothing to store
}

impl Target {
  pub fn parse(target_raw: &str) -> Result<Target, Error> {
    //First reject empty strings and non ascii characters
    if target_raw.is_empty()
      || !target_raw.bytes().all(|b| b.is_ascii_graphic())
    {
      return Err(Error::other("Malformed Request"));
    }

    let target: Target = match target_raw {
      "*" => Target::Asterisk,
      t if t.starts_with('/') => Target::Origin(t.to_string()),
      t if Target::is_absolute(t) => Target::Absolute(t.to_string()),
      t if Target::is_authority(t) => Target::Authority(t.to_string()),
      _ => return Err(Error::other("Malformed Request")),
    };

    Ok(target)
  }

  fn is_absolute(s: &str) -> bool {
    // Split at the :// separator.
    let Some((scheme, rest)) = s.split_once("://") else {
      return false;
    };

    let scheme_check = Target::is_scheme(scheme);
    let rest_check = !rest.is_empty();

    scheme_check && rest_check
  }

  fn is_scheme(s: &str) -> bool {
    /* first is a letter, every byte in rest is allowed */
    match s.as_bytes() {
      [b'a'..=b'z' | b'A'..=b'Z', rest @ ..] => rest
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')),
      _ => false,
    }
  }

  fn is_authority(s: &str) -> bool {
    // Split at the LAST colon so IPv6 literals keep their inner colons.
    let Some((host, port)) = s.rsplit_once(':') else {
      return false;
    };

    let port_ok = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());

    let host_ok = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']'))
    {
      // Bracketed IPv6 literal: loose check, fine for classification.
      Some(ip6) => {
        !ip6.is_empty()
          && ip6
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
      }
      // Hostname or IPv4.
      None => !host.is_empty() && host.bytes().all(Target::is_reg_name_char),
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn valid_origin() {
    assert!(matches!(
      Target::parse("/example"),
      Ok(Target::Origin(ref m)) if m == "/example"
    ));
  }

  #[test]
  fn invalid_origin_noslash() {
    assert!(Target::parse("example").is_err());
  }

  #[test]
  fn invalid_origin_nonascii() {
    assert!(Target::parse("/é").is_err());
  }

  #[test]
  fn invalid_origin_control_byte() {
    assert!(Target::parse("/foo\x01").is_err());
  }

  #[test]
  fn valid_absolute() {
    assert!(matches!(
      Target::parse("http://example.com"),
      Ok(Target::Absolute(ref m)) if m == "http://example.com"
    ));
  }

  #[test]
  fn invalid_absolute_malformed_scheme() {
    assert!(Target::parse("h@p://x").is_err());
  }

  #[test]
  fn invalid_absolute_numeric_start() {
    assert!(Target::parse("1http://example.com").is_err());
  }

  #[test]
  fn invalid_absolute_empty_rest() {
    assert!(Target::parse("http://").is_err());
  }

  #[test]
  fn valid_absolute_scheme_symbols() {
    assert!(matches!(
      Target::parse("a+b-c.d://x"),
      Ok(Target::Absolute(ref m)) if m == "a+b-c.d://x"
    ));
  }

  #[test]
  fn valid_absolute_uppercase_scheme() {
    assert!(matches!(
      Target::parse("HTTP://example.com:443"),
      Ok(Target::Absolute(ref m)) if m == "HTTP://example.com:443"
    ));
  }

  #[test]
  fn valid_authority_alphanumeric() {
    assert!(matches!(
      Target::parse("example.com:443"),
      Ok(Target::Authority(ref m)) if m == "example.com:443"
    ));
  }

  #[test]
  fn valid_authority_ipv6() {
    assert!(matches!(
      Target::parse("[::1]:8080"),
      Ok(Target::Authority(ref m)) if m == "[::1]:8080"
    ));
  }

  #[test]
  fn invalid_authority_noport() {
    assert!(Target::parse("example.com:").is_err());
  }

  #[test]
  fn invalid_authority_malformed_port() {
    assert!(Target::parse("example.com:$").is_err());
  }

  #[test]
  fn invalid_authority_nohost() {
    assert!(Target::parse(":443").is_err());
  }

  #[test]
  fn invalid_authority_malformed_host() {
    assert!(Target::parse("@:443").is_err());
  }

  #[test]
  fn valid_asterisk() {
    assert!(matches!(Target::parse("*"), Ok(Target::Asterisk)));
  }

  #[test]
  fn invalid_asterisk() {
    assert!(Target::parse("*x").is_err());
  }

  #[test]
  fn invalid_empty_string() {
    assert!(Target::parse("").is_err());
  }
}
