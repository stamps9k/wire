use std::fmt;
use std::io;

impl From<io::Error> for RequestError {
  fn from(e: io::Error) -> Self {
    Self::Io(e)
  }
}

#[derive(Debug)]
pub enum RequestError {
  Closed,
  Io(io::Error),
  TooLarge,
  Malformed(&'static str),
}

impl fmt::Display for RequestError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Closed => write!(f, "closed before head complete"),
      Self::TooLarge => write!(f, "head too large"),
      Self::Malformed(reason) => write!(f, "malformed request: {reason}"),
      Self::Io(e) => write!(f, "i/o error: {e}"),
    }
  }
}
