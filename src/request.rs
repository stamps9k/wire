use std::io::Error;

use super::method::Method;
use super::target::Target;
use super::version::Version;

fn is_tchar(b: u8) -> bool {
  matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'#' | b'$' |
  b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' |
  b'|' | b'~')
}

pub fn is_token(string: &str) -> bool {
  !string.is_empty() && string.bytes().all(is_tchar)
}

pub struct Request {
  pub method: Method,
  pub target: Target,
  pub version: Version,
  pub _headers: Vec<(String, String)>,
}

impl Request {
  pub fn parse(header: &str) -> Result<Request, Error> {
    let mut header_split = header.split("\r\n");

    let request_line =
      header_split.next().ok_or(Error::other("Empty request"))?;

    let mut request_split = request_line.split(' ');
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
