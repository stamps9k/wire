use std::fmt;

use crate::request_error::RequestError::Malformed;

use super::request_error::RequestError;

pub struct Version {
  pub major: u8,
  pub minor: u8,
}

impl Version {
  pub fn parse(version_raw: &[u8]) -> Result<Version, RequestError> {
    let Some(rest) = version_raw.strip_prefix(b"HTTP/") else {
      return Err(Malformed("version missing HTTP/ prefix"));
    };
    let [major @ b'0'..=b'9', b'.', minor @ b'0'..=b'9'] = rest else {
      return Err(Malformed("version is not digit.digit"));
    };

    let version = Version {
      major: major.wrapping_sub(b'0'),
      minor: minor.wrapping_sub(b'0'),
    };

    Ok(version)
  }
}

impl fmt::Display for Version {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "HTTP/{}.{}", self.major, self.minor)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn valid_http_1_1() {
    assert!(matches!(
      Version::parse(b"HTTP/1.1"),
      Ok(Version { major: 1, minor: 1 })
    ));
  }

  #[test]
  fn valid_http_1_0() {
    assert!(matches!(
      Version::parse(b"HTTP/1.0"),
      Ok(Version { major: 1, minor: 0 })
    ));
  }

  #[test]
  fn valid_unsupported_version_still_parses() {
    assert!(matches!(
      Version::parse(b"HTTP/2.0"),
      Ok(Version { major: 2, minor: 0 })
    ));
  }

  #[test]
  fn invalid_lowercase_name() {
    assert!(Version::parse(b"http/1.1").is_err());
  }

  #[test]
  fn invalid_missing_prefix() {
    assert!(Version::parse(b"1.1").is_err());
  }

  #[test]
  fn invalid_multi_digit_minor() {
    assert!(Version::parse(b"HTTP/1.10").is_err());
  }

  #[test]
  fn invalid_multi_digit_major() {
    assert!(Version::parse(b"HTTP/10.1").is_err());
  }

  #[test]
  fn invalid_signed_digit() {
    assert!(Version::parse(b"HTTP/+1.1").is_err());
  }

  #[test]
  fn invalid_missing_minor() {
    assert!(Version::parse(b"HTTP/1").is_err());
  }

  #[test]
  fn invalid_missing_dot() {
    assert!(Version::parse(b"HTTP/11").is_err());
  }

  #[test]
  fn invalid_trailing_bytes() {
    assert!(Version::parse(b"HTTP/1.1x").is_err());
  }

  #[test]
  fn invalid_non_digit() {
    assert!(Version::parse(b"HTTP/a.b").is_err());
  }

  #[test]
  fn invalid_empty_string() {
    assert!(Version::parse(b"").is_err());
  }

  #[test]
  fn invalid_high_byte_digit() {
    assert!(Version::parse(b"HTTP/1.\xB1").is_err());
  }

  #[test]
  fn display_version() {
    assert_eq!(Version { major: 1, minor: 1 }.to_string(), "HTTP/1.1");
  }
}
