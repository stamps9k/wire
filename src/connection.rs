//! Per-connection handling: reads the request head and writes the response.

use crate::request_error::RequestError;
use crate::response;
use crate::response::Response;
use crate::status::Status;

use std::io;
use std::net::SocketAddr;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio::time::timeout;

use super::request::Request;

/// Serves one client connection from start to finish.
///
/// Calls `serve` to read the request and decide the response, writes that
/// response, and closes the connection when this function returns.
///
/// If the request fails, the error is mapped to a status with
/// [`RequestError::status`] and an error page is sent. Errors with no status,
/// where the client has gone or the socket is broken, send nothing.
///
/// # Errors
///
/// Returns the [`RequestError`] from reading, parsing or validating the
/// request, after any error response has been written. Returns
/// [`RequestError::Io`] if writing the response fails.
pub async fn handle(
  mut stream: TcpStream,
  addr: SocketAddr,
) -> Result<(), RequestError> {
  println!("new client: {addr:?}");

  let result = serve(&mut stream).await;
  let (response, outcome) = match result {
    Ok(response) => (Some(response), Ok(())),
    Err(e) => (e.status().map(Response::from_status), Err(e)),
  };

  if let Some(response) = response {
    stream.write_all(&response.to_bytes()).await?;
  }

  outcome
}

/// Reads one request from `stream` and decides the response.
///
/// Reads the head with a 10-second deadline, parses and validates it, and
/// returns a page that echoes the request. Nothing is written to the stream.
///
/// # Errors
///
/// Returns [`RequestError::RequestTimeout`] if the head is not complete
/// within the deadline, [`RequestError::Closed`], [`RequestError::TooLarge`]
/// or [`RequestError::Io`] if it cannot be read, and
/// [`RequestError::Malformed`] if it fails parsing or validation.
async fn serve(stream: &mut TcpStream) -> Result<Response, RequestError> {
  let Ok(result) = timeout(Duration::from_secs(10), read_head(stream)).await
  else {
    return Err(RequestError::RequestTimeout);
  };
  let header = result?;

  println!("Header is {:?}", String::from_utf8_lossy(&header));

  let request = Request::parse(&header)?;
  request.validate()?;

  Ok(Response {
    status: Status::Ok,
    body: response::echo_body(&request),
  })
}

/// Reads the HTTP request head from `stream` until the blank line that ends it.
///
/// Reads in 1 KiB chunks and accumulates them in a buffer until `\r\n\r\n`
/// is found. The returned buffer may contain bytes past the terminator.
///
/// # Errors
///
/// Returns [`RequestError::Closed`] if the client closes the connection before
/// the head is complete, [`RequestError::TooLarge`] if the head exceeds 8 KiB,
/// and [`RequestError::Io`] if reading from the socket fails, for example on a
/// connection reset.
async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, RequestError> {
  let mut data: Vec<u8> = Vec::new();

  let mut chunk = [0u8; 1024];
  loop {
    let n = stream.read(&mut chunk).await?;
    if n == 0 {
      return Err(RequestError::Closed); // client closed before finishing the head
    }

    let Some(bytes) = chunk.get(..n) else {
      return Err(
        io::Error::other("read returned more bytes than the buffer").into(),
      );
    };
    data.extend_from_slice(bytes);

    if data.len() > 8 * 1024 {
      return Err(RequestError::TooLarge);
    }

    if let Some(_pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
      break;
    }
  }

  Ok(data)
}
