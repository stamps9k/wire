use std::io::Error;

use super::method::Method;
use super::target::Target;
use super::version::Version;

fn is_tchar(b: u8) -> bool {
  matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'#' | b'$' |
  b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' |
  b'|' | b'~')
}

pub fn is_token(b: &[u8]) -> bool {
  !b.is_empty() && b.iter().all(|bb| is_tchar(*bb))
}

pub struct Request {
  pub method: Method,
  pub target: Target,
  pub version: Version,
  pub _headers: Vec<(String, String)>,
}

impl Request {
  pub fn parse(request_line: &[u8]) -> Result<Request, Error> {
    let mut request_split = request_line.split(|b| *b == b' ');
    let method_raw = request_split
      .next()
      .ok_or(Error::other("Malformed Request"))?;

    let target_raw = request_split
      .next()
      .ok_or(Error::other("Malformed Request"))?;

    let version_raw = request_split
      .next()
      .ok_or(Error::other("Malformed Request"))?;

    //Check that the request did not include extra invalid information
    if request_split.next().is_some() {
      return Err(Error::other("Malformed Request"));
    }

    Ok(Request {
      method: Method::parse(method_raw)?,
      target: Target::parse(target_raw)?,
      version: Version::parse(version_raw)?,
      _headers: Vec::new(),
    })
  }
}
