use std::io::Error;

pub struct Version {
  pub major: u8,
  pub minor: u8,
}

impl Version {
  pub fn parse(version_raw: &str) -> Result<Version, Error> {
    let Some(rest) = version_raw.strip_prefix("HTTP/") else {
      return Err(Error::other("Malformed Request"));
    };
    let [major @ b'0'..=b'9', b'.', minor @ b'0'..=b'9'] = rest.as_bytes()
    else {
      return Err(Error::other("Malformed Request"));
    };

    let version = Version {
      major: major.wrapping_sub(b'0'),
      minor: minor.wrapping_sub(b'0'),
    };

    Ok(version)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn valid_http_1_1() {
    assert!(matches!(
      Version::parse("HTTP/1.1"),
      Ok(Version { major: 1, minor: 1 })
    ));
  }

  #[test]
  fn valid_http_1_0() {
    assert!(matches!(
      Version::parse("HTTP/1.0"),
      Ok(Version { major: 1, minor: 0 })
    ));
  }

  #[test]
  fn valid_unsupported_version_still_parses() {
    assert!(matches!(
      Version::parse("HTTP/2.0"),
      Ok(Version { major: 2, minor: 0 })
    ));
  }

  #[test]
  fn invalid_lowercase_name() {
    assert!(Version::parse("http/1.1").is_err());
  }

  #[test]
  fn invalid_missing_prefix() {
    assert!(Version::parse("1.1").is_err());
  }

  #[test]
  fn invalid_multi_digit_minor() {
    assert!(Version::parse("HTTP/1.10").is_err());
  }

  #[test]
  fn invalid_multi_digit_major() {
    assert!(Version::parse("HTTP/10.1").is_err());
  }

  #[test]
  fn invalid_signed_digit() {
    assert!(Version::parse("HTTP/+1.1").is_err());
  }

  #[test]
  fn invalid_missing_minor() {
    assert!(Version::parse("HTTP/1").is_err());
  }

  #[test]
  fn invalid_missing_dot() {
    assert!(Version::parse("HTTP/11").is_err());
  }

  #[test]
  fn invalid_trailing_bytes() {
    assert!(Version::parse("HTTP/1.1x").is_err());
  }

  #[test]
  fn invalid_non_digit() {
    assert!(Version::parse("HTTP/a.b").is_err());
  }

  #[test]
  fn invalid_empty_string() {
    assert!(Version::parse("").is_err());
  }
}
