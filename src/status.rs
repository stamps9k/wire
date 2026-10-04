/// An HTTP response status.
#[derive(Clone, Copy)]
pub enum Status {
  Ok,
  BadRequest,
  RequestTooLarge,
  RequestTimeout,
  MethodNotAllowed,
  NotFound,
}

impl Status {
  /// Returns the numeric status code, such as 200 or 400.
  pub fn code(self) -> u16 {
    match self {
      Self::Ok => 200,
      Self::BadRequest => 400,
      Self::RequestTooLarge => 431,
      Self::RequestTimeout => 408,
      Self::MethodNotAllowed => 405,
      Self::NotFound => 404,
    }
  }

  /// Returns the reason phrase that follows the code on the status line.
  pub fn reason(self) -> &'static str {
    match self {
      Self::Ok => "OK",
      Self::BadRequest => "Bad Request",
      Self::RequestTooLarge => "Request Too Large",
      Self::RequestTimeout => "Request Timeout",
      Self::MethodNotAllowed => "Method Not Allowed",
      Self::NotFound => "Not Found",
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn codes_are_correct() {
    assert_eq!(Status::Ok.code(), 200);
    assert_eq!(Status::BadRequest.code(), 400);
    assert_eq!(Status::RequestTimeout.code(), 408);
    assert_eq!(Status::RequestTooLarge.code(), 431);
    assert_eq!(Status::NotFound.code(), 404);
    assert_eq!(Status::MethodNotAllowed.code(), 405);
  }

  #[test]
  fn reasons_are_correct() {
    assert_eq!(Status::Ok.reason(), "OK");
    assert_eq!(Status::BadRequest.reason(), "Bad Request");
    assert_eq!(Status::RequestTimeout.reason(), "Request Timeout");
    assert_eq!(Status::RequestTooLarge.reason(), "Request Too Large");
    assert_eq!(Status::NotFound.reason(), "Not Found");
    assert_eq!(Status::MethodNotAllowed.reason(), "Method Not Allowed");
  }
}
