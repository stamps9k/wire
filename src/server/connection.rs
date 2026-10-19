//! Per-connection handling: reads the request head, writes the response,
//! publishes an event describing the connection, and hands a connection that
//! upgraded to WebSocket over to [`viewer`]. Also publishes the event for a
//! connection refused as soon as it was accepted ([`shed`]).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use std::time::SystemTime;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::sync::broadcast::Sender;
use tokio::time::Duration;
use tokio::time::timeout;

use crate::event;
use crate::event::Event;
use crate::event::Kind;
use crate::event::RejectReason;
use crate::event::ShedReason;
use crate::http::Request;
use crate::http::RequestError;
use crate::http::Response;
use crate::http::Status;
use crate::server::viewer;
use crate::site;

/// The longest a client may take to send its whole request head.
///
/// The deadline covers the head as a whole, not each read, so a client that
/// sends a byte at a time is cut off as surely as one that sends nothing. A
/// real client sends its head at once, well within this even on a slow
/// mobile link. Until the deadline, a silent client holds a connection
/// permit and one of its address's slots, so a short deadline frees them
/// sooner during a flood. When it passes, the client gets 408 Request
/// Timeout and the event is rejected with reason `timeout`.
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

/// Serves one client connection from start to finish.
///
/// Calls `serve` to read the request and decide the response, then writes
/// that response.
///
/// If the request fails, the error is mapped to a status with
/// [`RequestError::status`] and an error page is sent. Errors with no status,
/// where the client has gone or the socket is broken, send nothing.
///
/// One [`Event`] describing the connection is sent on `tx` after the write,
/// whatever the outcome. `connection_id` is the number given to this
/// connection when it was accepted. `viewer_pool` holds the slots for
/// WebSocket viewers.
///
/// If the response is 101 Switching Protocols, a slot is taken from
/// `viewer_pool` before anything is written. If none is free, the response
/// is replaced with 503 Service Unavailable and the connection is closed
/// like any other. Otherwise, once the 101 is written successfully, the
/// connection becomes a viewer and keeps its slot until this function
/// returns. It subscribes to `tx` before its own event is sent, so that
/// event is the first one it receives. Then [`viewer::stream_events`] keeps
/// the connection open and streams every event to the client until the
/// client goes away. Any other connection is closed when this function
/// returns.
///
/// # Errors
///
/// Returns the [`RequestError`] from reading, parsing or validating the
/// request, after any error response has been written. Returns
/// [`RequestError::Io`] if writing the response fails, or if reading from or
/// writing to a viewer fails.
pub async fn handle(
  mut stream: TcpStream,
  addr: SocketAddr,
  tx: Sender<Event>,
  connection_id: u64,
  viewer_pool: Arc<Semaphore>,
) -> Result<(), RequestError> {
  let start_time = SystemTime::now();
  let started = Instant::now();
  let mut buffer: Vec<u8> = Vec::new();

  let mut result = serve(&mut stream, &mut buffer).await;

  // Held until handle returns, so the viewer slot stays taken while streaming.
  let mut _viewer_permit = None;
  if let Ok((_, response)) = &mut result {
    if matches!(response.status, Status::SwitchingProtocols) {
      match viewer_pool.try_acquire_owned() {
        Ok(permit) => _viewer_permit = Some(permit),
        Err(_) => *response = site::error_response(Status::ServiceUnavailable),
      }
    }
  }

  let bytes = match &result {
    Ok((_, response)) => Some(response.to_bytes()),
    Err(e) => e.status().map(|s| site::error_response(s).to_bytes()),
  };

  let written = match &bytes {
    Some(bytes) => stream.write_all(bytes).await,
    None => Ok(()),
  };

  let upgraded = written.is_ok()
    && matches!(
      &result,
      Ok((_, response))
        if matches!(response.status, Status::SwitchingProtocols)
    );
  let viewer = upgraded.then(|| tx.subscribe());

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

  if let Some(rx) = viewer {
    viewer::stream_events(&mut stream, rx).await?;
  }

  result.map(|_| ())
}

/// Reads one request from `stream` and decides the response.
///
/// Reads the head into `buffer` within [`HEAD_TIMEOUT`], then parses,
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
  let Ok(read) = timeout(HEAD_TIMEOUT, read_head(stream, buffer)).await else {
    return Err(RequestError::RequestTimeout);
  };
  read?;

  let request = Request::parse(buffer)?;
  request.validate()?;

  let result = site::route(&request);

  Ok((request, result))
}

/// Publishes the [`Event`] for a connection that was closed as soon as it
/// was accepted.
///
/// The event is [`Kind::Shed`] with `reason`. Nothing was read from the
/// connection or written to it, so its duration and byte count are 0 and it
/// has no TCP snapshot. `connection_id` is the number given to the
/// connection when it was accepted, and the timestamp is the current time.
///
/// A failed send only means that nobody is subscribed, so it is ignored.
pub fn shed(
  connection_id: u64,
  addr: SocketAddr,
  reason: ShedReason,
  tx: &Sender<Event>,
) {
  let timestamp_ms = u64::try_from(
    SystemTime::now()
      .duration_since(SystemTime::UNIX_EPOCH)
      .unwrap_or_default()
      .as_millis(),
  )
  .unwrap_or(u64::MAX);

  let kind = Kind::Shed { reason };

  let _ = tx.send(Event {
    connection_id,
    peer_address: addr,
    timestamp_ms,
    duration_ms: 0,
    bytes_received: 0,
    tcp_snapshot: None,
    kind,
  });
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

#[cfg(test)]
mod tests {
  use super::*;

  use std::error::Error;

  use serde_json::Value;
  use tokio::net::TcpListener;
  use tokio::sync::broadcast;

  type TestResult = Result<(), Box<dyn Error>>;

  const UPGRADE: &[u8] = b"GET /ws HTTP/1.1\r\n\
    Host: wire.stampatron.com\r\n\
    Upgrade: websocket\r\n\
    Connection: Upgrade\r\n\
    Sec-WebSocket-Version: 13\r\n\
    Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    \r\n";

  /// Starts `handle` on one loopback connection, as connection 1, with one
  /// free viewer slot, and returns the client's end of it.
  async fn connect(tx: Sender<Event>) -> io::Result<TcpStream> {
    connect_with(tx, Arc::new(Semaphore::new(1))).await
  }

  /// Like [`connect`], but with the viewer slots given by `viewers`.
  async fn connect_with(
    tx: Sender<Event>,
    viewers: Arc<Semaphore>,
  ) -> io::Result<TcpStream> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
      if let Ok((stream, peer)) = listener.accept().await {
        let _ = handle(stream, peer, tx, 1, viewers).await;
      }
    });
    TcpStream::connect(address).await
  }

  /// Reads up to and including the blank line that ends a response head,
  /// one byte at a time so that nothing after it is consumed.
  async fn read_response_head(client: &mut TcpStream) -> io::Result<String> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
      head.push(client.read_u8().await?);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
  }

  /// Reads one unmasked text frame and parses its payload as JSON.
  async fn read_event(client: &mut TcpStream) -> Result<Value, Box<dyn Error>> {
    if client.read_u8().await? != 0x81 {
      return Err("not a final text frame".into());
    }
    let length = match client.read_u8().await? {
      126 => usize::from(client.read_u16().await?),
      short if short < 126 => usize::from(short),
      _ => return Err("frame is masked or too long for this test".into()),
    };
    let mut payload = vec![0; length];
    client.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
  }

  /// Waits until `handle` has subscribed to the channel.
  async fn subscribed(tx: &Sender<Event>) -> Result<(), Box<dyn Error>> {
    for _ in 0..500 {
      if tx.receiver_count() > 0 {
        return Ok(());
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the connection never subscribed to the channel".into())
  }

  fn event(connection_id: u64) -> Event {
    Event {
      connection_id,
      peer_address: SocketAddr::from(([203, 0, 113, 9], 51_234)),
      timestamp_ms: 1_700_000_000_000,
      duration_ms: 12,
      bytes_received: 78,
      tcp_snapshot: None,
      kind: Kind::Rejected {
        reason: RejectReason::Closed,
        detail: None,
        status: None,
      },
    }
  }

  fn id(event: &Value) -> Option<u64> {
    event.get("connection_id").and_then(Value::as_u64)
  }

  #[test]
  fn shed_publishes_an_event_with_nothing_read() -> TestResult {
    let (tx, mut rx) = broadcast::channel::<Event>(16);
    let peer = SocketAddr::from(([203, 0, 113, 9], 51_234));
    shed(5, peer, ShedReason::IpLimit, &tx);
    let event = rx.try_recv()?;
    assert_eq!(event.connection_id, 5);
    assert_eq!(event.peer_address, peer);
    assert!(event.timestamp_ms > 0);
    assert_eq!(event.duration_ms, 0);
    assert_eq!(event.bytes_received, 0);
    assert!(event.tcp_snapshot.is_none());
    assert!(matches!(
      event.kind,
      Kind::Shed {
        reason: ShedReason::IpLimit
      }
    ));
    Ok(())
  }

  #[tokio::test]
  async fn a_plain_request_is_answered_and_the_connection_closed() -> TestResult
  {
    let (tx, _) = broadcast::channel::<Event>(16);
    let mut client = connect(tx).await?;
    client
      .write_all(b"GET / HTTP/1.1\r\nHost: wire.stampatron.com\r\n\r\n")
      .await?;

    // Reading to the end only returns once the server has closed.
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), client.read_to_end(&mut response))
      .await??;

    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    Ok(())
  }

  #[tokio::test]
  async fn a_refused_handshake_is_answered_and_the_connection_closed()
  -> TestResult {
    let (tx, _) = broadcast::channel::<Event>(16);
    let mut client = connect(tx).await?;
    client
      .write_all(b"GET /ws HTTP/1.1\r\nHost: wire.stampatron.com\r\n\r\n")
      .await?;

    let mut response = Vec::new();
    timeout(Duration::from_secs(5), client.read_to_end(&mut response))
      .await??;

    assert!(response.starts_with(b"HTTP/1.1 400 Bad Request\r\n"));
    Ok(())
  }

  #[tokio::test]
  async fn an_upgraded_connection_is_sent_later_events_as_frames() -> TestResult
  {
    let (tx, _) = broadcast::channel::<Event>(16);
    let mut client = connect(tx.clone()).await?;
    client.write_all(UPGRADE).await?;

    let head = timeout(Duration::from_secs(5), read_response_head(&mut client))
      .await??;
    assert!(head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    timeout(Duration::from_secs(10), subscribed(&tx)).await??;
    let _ = tx.send(event(41));
    let _ = tx.send(event(42));

    // The connection's own handshake event (id 1) may or may not come
    // first, depending on when `handle` subscribes. Skip it if it does.
    let mut seen = Vec::new();
    while seen.len() < 2 {
      let event =
        timeout(Duration::from_secs(5), read_event(&mut client)).await??;
      if id(&event) != Some(1) {
        seen.push(id(&event));
      }
    }
    assert_eq!(seen, [Some(41), Some(42)]);
    Ok(())
  }

  #[tokio::test]
  async fn an_upgraded_connection_publishes_its_handshake_as_a_request()
  -> TestResult {
    let (tx, mut rx) = broadcast::channel::<Event>(16);
    let mut client = connect(tx).await?;
    client.write_all(UPGRADE).await?;

    let event = timeout(Duration::from_secs(5), rx.recv()).await??;

    assert_eq!(event.connection_id, 1);
    assert!(matches!(event.kind, Kind::Request { status: 101, .. }));
    Ok(())
  }

  #[tokio::test]
  async fn an_upgraded_connection_stops_when_the_client_has_gone() -> TestResult
  {
    let (tx, _) = broadcast::channel::<Event>(16);
    let mut client = connect(tx.clone()).await?;
    client.write_all(UPGRADE).await?;
    timeout(Duration::from_secs(5), read_response_head(&mut client)).await??;
    timeout(Duration::from_secs(10), subscribed(&tx)).await??;

    drop(client);

    // The server only notices on a write, and the first write after the
    // client closes can still succeed, so keep sending until it lets go.
    for connection_id in 100..600 {
      if tx.receiver_count() == 0 {
        return Ok(());
      }
      let _ = tx.send(event(connection_id));
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the connection was still subscribed after the client left".into())
  }

  #[tokio::test]
  async fn an_upgrade_is_refused_with_503_when_no_viewer_slot_is_free()
  -> TestResult {
    let (tx, _) = broadcast::channel::<Event>(16);
    let mut client = connect_with(tx, Arc::new(Semaphore::new(0))).await?;
    client.write_all(UPGRADE).await?;

    // Reading to the end only returns once the server has closed.
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), client.read_to_end(&mut response))
      .await??;

    assert!(response.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"));
    Ok(())
  }

  #[tokio::test]
  async fn a_refused_upgrade_publishes_a_503() -> TestResult {
    let (tx, mut rx) = broadcast::channel::<Event>(16);
    let mut client = connect_with(tx, Arc::new(Semaphore::new(0))).await?;
    client.write_all(UPGRADE).await?;

    let event = timeout(Duration::from_secs(5), rx.recv()).await??;

    assert!(matches!(event.kind, Kind::Request { status: 503, .. }));
    Ok(())
  }

  #[tokio::test]
  async fn a_plain_request_does_not_need_a_viewer_slot() -> TestResult {
    let (tx, _) = broadcast::channel::<Event>(16);
    let mut client = connect_with(tx, Arc::new(Semaphore::new(0))).await?;
    client
      .write_all(b"GET / HTTP/1.1\r\nHost: wire.stampatron.com\r\n\r\n")
      .await?;

    let mut response = Vec::new();
    timeout(Duration::from_secs(5), client.read_to_end(&mut response))
      .await??;

    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    Ok(())
  }

  #[tokio::test]
  async fn a_second_viewer_is_refused_while_the_first_holds_the_slot()
  -> TestResult {
    let (tx, _) = broadcast::channel::<Event>(16);
    let viewers = Arc::new(Semaphore::new(1));

    let mut first = connect_with(tx.clone(), viewers.clone()).await?;
    first.write_all(UPGRADE).await?;
    let head =
      timeout(Duration::from_secs(5), read_response_head(&mut first)).await??;
    assert!(head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut second = connect_with(tx, viewers).await?;
    second.write_all(UPGRADE).await?;
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), second.read_to_end(&mut response))
      .await??;

    assert!(response.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"));
    Ok(())
  }

  #[tokio::test]
  async fn a_viewer_holds_its_slot_until_the_client_has_gone() -> TestResult {
    let (tx, _) = broadcast::channel::<Event>(16);
    let viewers = Arc::new(Semaphore::new(1));
    let mut client = connect_with(tx.clone(), viewers.clone()).await?;
    client.write_all(UPGRADE).await?;
    timeout(Duration::from_secs(5), read_response_head(&mut client)).await??;
    timeout(Duration::from_secs(10), subscribed(&tx)).await??;

    assert_eq!(viewers.available_permits(), 0);

    drop(client);

    // As in the test above, the server only notices on a write, so keep
    // sending until it lets go of the slot.
    for connection_id in 100..600 {
      if viewers.available_permits() == 1 {
        return Ok(());
      }
      let _ = tx.send(event(connection_id));
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the viewer slot was not released after the client left".into())
  }
}
