use std::fmt;
use std::io;

use crate::status::Status;

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
  RequestTimeout,
}

impl RequestError {
  /// Returns the status to send to the client for this error.
  ///
  /// Returns `None` when no response should be sent, because the client has
  /// closed the connection or the socket has failed.
  pub fn status(&self) -> Option<Status> {
    match self {
      Self::Closed | Self::Io(_) => None,
      Self::Malformed(_) => Some(Status::BadRequest),
      Self::TooLarge => Some(Status::RequestTooLarge),
      Self::RequestTimeout => Some(Status::RequestTimeout),
    }
  }
}

impl fmt::Display for RequestError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Closed => write!(f, "closed before head complete"),
      Self::TooLarge => write!(f, "head too large"),
      Self::Malformed(reason) => write!(f, "malformed request: {reason}"),
      Self::Io(e) => write!(f, "i/o error: {e}"),
      Self::RequestTimeout => write!(f, "client timeout"),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn malformed_maps_to_bad_request() {
    assert!(matches!(
      RequestError::Malformed("x").status(),
      Some(Status::BadRequest)
    ));
  }

  #[test]
  fn too_large_maps_to_request_too_large() {
    assert!(matches!(
      RequestError::TooLarge.status(),
      Some(Status::RequestTooLarge)
    ));
  }

  #[test]
  fn timeout_maps_to_request_timeout() {
    assert!(matches!(
      RequestError::RequestTimeout.status(),
      Some(Status::RequestTimeout)
    ));
  }

  #[test]
  fn closed_has_no_status() {
    assert!(RequestError::Closed.status().is_none());
  }

  #[test]
  fn io_has_no_status() {
    let error = RequestError::Io(io::Error::other("x"));
    assert!(error.status().is_none());
  }
}
