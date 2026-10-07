//! Per-connection handling: reads the request head, writes the response and
//! publishes an event describing the connection.

use crate::event;
use crate::event::Event;
use crate::event::Kind;
use crate::event::RejectReason;
use crate::request_error::RequestError;
use crate::response::Response;
use crate::router;
use crate::status::Status;

use std::io;
use std::net::SocketAddr;
use std::time::Instant;
use std::time::SystemTime;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::broadcast::Sender;
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
/// One [`Event`] describing the connection is sent on `tx` after the write,
/// whatever the outcome. `connection_id` is the number given to this
/// connection when it was accepted.
///
/// # Errors
///
/// Returns the [`RequestError`] from reading, parsing or validating the
/// request, after any error response has been written. Returns
/// [`RequestError::Io`] if writing the response fails.
pub async fn handle(
  mut stream: TcpStream,
  addr: SocketAddr,
  tx: Sender<Event>,
  connection_id: u64,
) -> Result<(), RequestError> {
  let start_time = SystemTime::now();
  let started = Instant::now();
  let mut buffer: Vec<u8> = Vec::new();

  let result = serve(&mut stream, &mut buffer).await;

  let bytes = match &result {
    Ok((_, response)) => Some(response.to_bytes()),
    Err(e) => e.status().map(|s| Response::from_status(s).to_bytes()),
  };

  let written = match &bytes {
    Some(bytes) => stream.write_all(bytes).await,
    None => Ok(()),
  };

  publish_event(
    connection_id,
    addr,
    &tx,
    result.as_ref(),
    buffer.len(),
    &start_time,
    started,
  );

  written?;
  result.map(|_| ())
}

/// Reads one request from `stream` and decides the response.
///
/// Reads the head into `buffer` with a 10-second deadline, then parses,
/// validates and routes it. Returns the request together with the response
/// chosen for it. Nothing is written to the stream.
///
/// `buffer` belongs to the caller so that the bytes received can still be
/// counted when this function fails. That includes a timeout, where the
/// read is cancelled part-way.
///
/// # Errors
///
/// Returns [`RequestError::RequestTimeout`] if the head is not complete
/// within the deadline, [`RequestError::Closed`], [`RequestError::TooLarge`]
/// or [`RequestError::Io`] if it cannot be read, and
/// [`RequestError::Malformed`] if it fails parsing or validation.
async fn serve(
  stream: &mut TcpStream,
  buffer: &mut Vec<u8>,
) -> Result<(Request, Response), RequestError> {
  let Ok(read) =
    timeout(Duration::from_secs(10), read_head(stream, buffer)).await
  else {
    return Err(RequestError::RequestTimeout);
  };
  read?;

  let request = Request::parse(buffer)?;
  request.validate()?;

  let result = router::route(&request);

  Ok((request, result))
}

/// Builds the [`Event`] for a finished connection and sends it on `tx`.
///
/// `result` is the outcome of `serve`. A request with its response becomes
/// [`Kind::Request`], and an error becomes [`Kind::Rejected`]. Text taken
/// from the client is cut to [`event::CAP_SIZE`] bytes, and its original
/// length is recorded separately.
///
/// `request_length` is the number of bytes read from the client.
/// `start_time` gives the event its timestamp, and `started` is used to
/// measure how long the connection took.
///
/// A failed send only means that nobody is subscribed, so it is ignored.
fn publish_event(
  connection_id: u64,
  addr: SocketAddr,
  tx: &Sender<Event>,
  result: Result<&(Request, Response), &RequestError>,
  request_length: usize,
  start_time: &SystemTime,
  started: Instant,
) {
  let timestamp_ms = u64::try_from(
    start_time
      .duration_since(SystemTime::UNIX_EPOCH)
      .unwrap_or_default()
      .as_millis(),
  )
  .unwrap_or(u64::MAX);

  let kind = match result {
    Ok((request, response)) => {
      let user_agent_opt =
        request.headers.iter().find(|h| h.name == "user-agent");
      let (user_agent_length, user_agent) = match user_agent_opt {
        Some(s) => (
          u32::try_from(s.value.len()).unwrap_or(u32::MAX),
          Some(event::truncate(
            String::from_utf8_lossy(&s.value).into_owned(),
            event::CAP_SIZE,
          )),
        ),
        None => (0, None),
      };

      let target_s = request.target.to_string();
      let target_s_len = target_s.len();
      Kind::Request {
        method: event::truncate(request.method.to_string(), event::CAP_SIZE),
        target: event::truncate(target_s, event::CAP_SIZE),
        target_length: u32::try_from(target_s_len).unwrap_or(u32::MAX),
        version: request.version.to_string(),
        status: response.status.code(),
        user_agent,
        user_agent_length,
        host_class: request.classify_host_class(),
        header_count: u16::try_from(request.headers.len()).unwrap_or(u16::MAX),
      }
    }
    Err(e) => {
      let (rr, detail) = match e {
        RequestError::Malformed(s) => (RejectReason::Malformed, Some(*s)),
        RequestError::Io(_) => (RejectReason::Io, None),
        RequestError::Closed => (RejectReason::Closed, None),
        RequestError::RequestTimeout => (RejectReason::Timeout, None),
        RequestError::TooLarge => (RejectReason::TooLarge, None),
      };

      Kind::Rejected {
        reason: rr,
        detail,
        status: e.status().map(Status::code),
      }
    }
  };

  let bytes_received: u32 = u32::try_from(request_length).unwrap_or(u32::MAX);
  let _ = tx.send(Event {
    connection_id,
    peer_address: addr,
    timestamp_ms,
    duration_ms: u64::try_from(started.elapsed().as_millis())
      .unwrap_or(u64::MAX),
    bytes_received,
    tcp_snapshot: None,
    kind,
  });
}

/// Reads the HTTP request head from `stream` until the blank line that ends it.
///
/// Reads in 1 KiB chunks and appends them to `data` until `\r\n\r\n` is
/// found. `data` may end up holding bytes past the terminator, and it keeps
/// whatever was read if this function fails or is cancelled.
///
/// # Errors
///
/// Returns [`RequestError::Closed`] if the client closes the connection before
/// the head is complete, [`RequestError::TooLarge`] if the head exceeds 8 KiB,
/// and [`RequestError::Io`] if reading from the socket fails, for example on a
/// connection reset.
async fn read_head(
  stream: &mut TcpStream,
  data: &mut Vec<u8>,
) -> Result<(), RequestError> {
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

  Ok(())
}
