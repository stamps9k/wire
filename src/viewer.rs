//! WebSocket viewers: after a successful upgrade, a connection subscribes to
//! the event channel and receives every event as a text frame of JSON, while
//! the frames the client sends are read and answered, until either side ends
//! the connection.

use std::io::Error;

use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use crate::event::Event;
use crate::frame::Frame;
use crate::frame::FrameBuffer;
use crate::frame::FrameError;
use crate::frame::OpCode;

/// The largest payload accepted from a viewer: enough for any control frame,
/// since a viewer has nothing else to send.
const MAX_FRAME_PAYLOAD: usize = 125;

/// Streams events to a WebSocket viewer and answers the frames it sends,
/// until either side ends the connection.
///
/// Each event received on `rx` is serialised to JSON and written to
/// `stream` as one text frame (see [`Frame::text`] and [`Frame::to_bytes`]).
/// Only events sent after `rx` was subscribed are seen. If the viewer falls
/// behind and the channel drops events, the missed ones are skipped and
/// streaming carries on with the next. An event that cannot be serialised
/// is skipped.
///
/// At the same time, bytes from the client are collected in a
/// [`FrameBuffer`] and taken from it one frame at a time, with a payload cap
/// of [`MAX_FRAME_PAYLOAD`]. Between frames the loop yields, so a client
/// sending many frames at once cannot keep other tasks off the thread. A
/// ping is answered with a pong carrying the same payload, and a pong is
/// ignored. A close is answered with a close echoing its status code, which
/// ends the loop. Text and binary frames have no meaning for a viewer and
/// are ignored. A frame that breaks the protocol or exceeds the cap is
/// answered with a close carrying the code from [`FrameError::close_code`],
/// which also ends the loop.
///
/// Returns `Ok(())` after a closing handshake, after the client
/// disconnects, or when the channel closes. In the server the channel
/// never closes while the connection holds a sender.
///
/// # Errors
///
/// Returns the error from a failed read or write on `stream`.
pub async fn stream_events<S>(
  stream: &mut S,
  mut rx: Receiver<Event>,
) -> Result<(), Error>
where
  S: AsyncWrite + AsyncRead + Unpin,
{
  let mut buffer = FrameBuffer::default();
  let mut chunk = [0u8; 1024];
  loop {
    tokio::select! {
      result = rx.recv() => {
        let event = match result {
          Ok(v) => v,
          Err(RecvError::Lagged(_)) => continue,
          Err(RecvError::Closed) => break,
        };
        let Ok(event_s) = serde_json::to_string(&event) else {
          continue;
        };
        stream
          .write_all(&Frame::text(event_s.as_bytes()).to_bytes())
          .await?;
      },
      result = stream.read(&mut chunk) => {
        let size = result?;
        if size == 0 {
          return Ok(());
        }
        let Some(bytes) = chunk.get(..size) else {
          return Err(Error::other("read returned more bytes than the buffer"));
        };

        buffer.extend(bytes);
        loop {
          match buffer.next_frame(MAX_FRAME_PAYLOAD) {
            Ok(None) => break,
            Ok(Some(frame)) => {
              match frame.opcode {
                OpCode::Close => {
                  let code = frame
                    .payload
                    .first_chunk::<2>()
                    .map_or(1000, |bytes| u16::from_be_bytes(*bytes));
                  stream.write_all(&Frame::close(code, "").to_bytes()).await?;
                  return Ok(());
                },
                OpCode::Ping => {
                  stream.write_all(&Frame {
                    fin: true,
                    opcode: OpCode::Pong,
                    payload: frame.payload,
                  }.to_bytes()).await?;
                },
                _ => {}
              }
            },
            Err(e) => {
              let reason = match e {
                FrameError::Protocol(reason) => reason,
                FrameError::TooLarge => "",
              };
              stream
                .write_all(&Frame::close(e.close_code(), reason).to_bytes())
                .await?;
              return Ok(());
            }
          }
          // Let other tasks run between frames, so one busy viewer cannot
          // hold the thread.
          tokio::task::yield_now().await;
        }
      }
    };
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::event::Event;
  use crate::event::Kind;
  use crate::event::RejectReason;

  use std::error::Error;
  use std::net::SocketAddr;
  use std::sync::Arc;
  use std::sync::atomic::AtomicUsize;
  use std::sync::atomic::Ordering;
  use std::time::Duration;

  use serde_json::Value;
  use tokio::io::AsyncReadExt;
  use tokio::io::AsyncWriteExt;
  use tokio::io::DuplexStream;
  use tokio::sync::broadcast;
  use tokio::sync::broadcast::Receiver;
  use tokio::sync::broadcast::Sender;
  use tokio::sync::oneshot;
  use tokio::time::timeout;

  type TestResult = Result<(), Box<dyn Error>>;

  /// Where a viewer started by [`start`] reports what [`stream_events`]
  /// returned.
  type Viewer = oneshot::Receiver<Result<(), std::io::Error>>;

  /// The masking key used for every frame the test client sends.
  const KEY: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

  /// How long to wait for something that should happen.
  const WAIT: Duration = Duration::from_secs(5);

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

  /// Splits `bytes` into the payloads of the frames it holds.
  ///
  /// Understands the one-byte and two-byte lengths only. Returns `None` if
  /// a frame is not an unmasked text frame or is cut short.
  fn payloads(mut bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let mut found = Vec::new();
    while !bytes.is_empty() {
      let (head, rest) = bytes.split_at_checked(2)?;
      let (length, rest) = match head {
        [0x81, 126] => {
          let (size, rest) = rest.split_at_checked(2)?;
          (usize::from(u16::from_be_bytes(size.try_into().ok()?)), rest)
        }
        [0x81, short] if *short < 126 => (usize::from(*short), rest),
        _ => return None,
      };
      let (payload, rest) = rest.split_at_checked(length)?;
      found.push(payload);
      bytes = rest;
    }
    Some(found)
  }

  /// Reads the `connection_id` out of each payload, in order.
  fn ids(payloads: &[&[u8]]) -> Result<Vec<Option<u64>>, serde_json::Error> {
    payloads
      .iter()
      .map(|payload| {
        let json: Value = serde_json::from_slice(payload)?;
        Ok(json.get("connection_id").and_then(Value::as_u64))
      })
      .collect()
  }

  /// Runs [`stream_events`] on one end of an in-memory pipe until it
  /// returns, then gives back everything it wrote to the other end.
  ///
  /// The client end stays open and silent the whole time, so only the
  /// channel closing (or a failure) can end the loop.
  async fn run(rx: Receiver<Event>) -> Result<Vec<u8>, Box<dyn Error>> {
    let (mut server, mut client) = tokio::io::duplex(65_536);
    timeout(WAIT, stream_events(&mut server, rx)).await??;
    drop(server);

    let mut written = Vec::new();
    client.read_to_end(&mut written).await?;
    Ok(written)
  }

  /// Runs `future` on its own thread with its own single-threaded runtime,
  /// and returns where its output will arrive.
  ///
  /// This is the guard against a viewer that stops making progress: if a
  /// bug ever makes it loop without yielding, only that thread is stuck.
  /// The test's own timeouts still fire, so the test fails instead of
  /// hanging the whole suite.
  fn isolated<F>(future: F) -> oneshot::Receiver<F::Output>
  where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
  {
    let (report, output) = oneshot::channel();
    std::thread::spawn(move || {
      let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
      else {
        return;
      };
      let _ = report.send(runtime.block_on(future));
    });
    output
  }

  /// Starts [`stream_events`] on one end of an in-memory pipe, in a runtime
  /// of its own (see [`isolated`]), and returns the client end, the
  /// channel's sender and where its result will arrive.
  ///
  /// The sender must be kept alive, or the closed channel ends the loop.
  fn start() -> (DuplexStream, Sender<Event>, Viewer) {
    let (tx, rx) = broadcast::channel::<Event>(16);
    let (mut server, client) = tokio::io::duplex(65_536);
    let viewer = isolated(async move { stream_events(&mut server, rx).await });
    (client, tx, viewer)
  }

  /// Builds a frame as a client sends it: `first` as the first byte, a
  /// one-byte length with the mask bit set, [`KEY`], then `payload` masked
  /// with it. The payload must be shorter than 126 bytes.
  fn client_frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let length = u8::try_from(payload.len()).unwrap_or(u8::MAX);
    let mut out = vec![first, 0x80 | length];
    out.extend_from_slice(&KEY);
    out.extend(
      payload
        .iter()
        .zip(KEY.iter().cycle())
        .map(|(byte, mask)| byte ^ mask),
    );
    out
  }

  /// Reads one frame the server sent and returns its first byte and its
  /// payload. Understands the one-byte and two-byte lengths only, and
  /// fails if the mask bit is set.
  async fn read_frame(
    client: &mut DuplexStream,
  ) -> Result<(u8, Vec<u8>), Box<dyn Error>> {
    let mut head = [0_u8; 2];
    timeout(WAIT, client.read_exact(&mut head)).await??;
    let [first, second] = head;
    let length = match second {
      126 => {
        let mut size = [0_u8; 2];
        timeout(WAIT, client.read_exact(&mut size)).await??;
        usize::from(u16::from_be_bytes(size))
      }
      short if short < 126 => usize::from(short),
      _ => return Err("masked or eight-byte length from the server".into()),
    };
    let mut payload = vec![0_u8; length];
    timeout(WAIT, client.read_exact(&mut payload)).await??;
    Ok((first, payload))
  }

  /// Reads the `connection_id` out of a JSON payload.
  fn id(payload: &[u8]) -> Result<Option<u64>, serde_json::Error> {
    let json: Value = serde_json::from_slice(payload)?;
    Ok(json.get("connection_id").and_then(Value::as_u64))
  }

  /// Waits for the viewer to end and returns what it returned.
  async fn finished(
    viewer: Viewer,
  ) -> Result<Result<(), std::io::Error>, Box<dyn Error>> {
    Ok(timeout(WAIT, viewer).await??)
  }

  // ---- sending events ----

  #[tokio::test]
  async fn writes_nothing_when_the_channel_closes_without_events() -> TestResult
  {
    let (tx, rx) = broadcast::channel::<Event>(16);
    drop(tx);

    let written = run(rx).await?;

    assert!(written.is_empty());
    Ok(())
  }

  #[tokio::test]
  async fn writes_each_event_as_one_text_frame_of_json() -> TestResult {
    let (tx, rx) = broadcast::channel::<Event>(16);
    let first = event(1);
    let second = event(2);
    let expected = [
      serde_json::to_string(&first)?,
      serde_json::to_string(&second)?,
    ];
    let _ = tx.send(first);
    let _ = tx.send(second);
    drop(tx);

    let written = run(rx).await?;

    let payloads = payloads(&written).ok_or("not a run of text frames")?;
    let expected: Vec<&[u8]> = expected.iter().map(String::as_bytes).collect();
    assert_eq!(payloads, expected);
    Ok(())
  }

  #[tokio::test]
  async fn keeps_the_order_in_which_events_were_sent() -> TestResult {
    let (tx, rx) = broadcast::channel::<Event>(16);
    for connection_id in 1..=5 {
      let _ = tx.send(event(connection_id));
    }
    drop(tx);

    let written = run(rx).await?;

    let payloads = payloads(&written).ok_or("not a run of text frames")?;
    assert_eq!(
      ids(&payloads)?,
      [Some(1), Some(2), Some(3), Some(4), Some(5)]
    );
    Ok(())
  }

  #[tokio::test]
  async fn skips_missed_events_and_carries_on_after_lagging() -> TestResult {
    // The channel holds two events, so the first three of these five are
    // gone by the time the viewer starts reading.
    let (tx, rx) = broadcast::channel::<Event>(2);
    for connection_id in 1..=5 {
      let _ = tx.send(event(connection_id));
    }
    drop(tx);

    let written = run(rx).await?;

    let payloads = payloads(&written).ok_or("not a run of text frames")?;
    assert_eq!(ids(&payloads)?, [Some(4), Some(5)]);
    Ok(())
  }

  #[tokio::test]
  async fn only_sees_events_sent_after_it_subscribed() -> TestResult {
    let (tx, _) = broadcast::channel::<Event>(16);
    let _ = tx.send(event(1));
    let rx = tx.subscribe();
    let _ = tx.send(event(2));
    drop(tx);

    let written = run(rx).await?;

    let payloads = payloads(&written).ok_or("not a run of text frames")?;
    assert_eq!(ids(&payloads)?, [Some(2)]);
    Ok(())
  }

  #[tokio::test]
  async fn waits_for_more_events_while_the_channel_is_open() -> TestResult {
    let (tx, rx) = broadcast::channel::<Event>(16);
    let _ = tx.send(event(1));
    let (mut server, mut client) = tokio::io::duplex(65_536);

    let outcome =
      timeout(Duration::from_millis(100), stream_events(&mut server, rx)).await;

    // Timing out is the pass: the loop was still waiting on the channel.
    assert!(outcome.is_err());
    drop(server);
    let mut written = Vec::new();
    client.read_to_end(&mut written).await?;
    assert_eq!(payloads(&written).map(|found| found.len()), Some(1));
    Ok(())
  }

  #[tokio::test]
  async fn keeps_streaming_events_after_answering_a_ping() -> TestResult {
    let (mut client, tx, _viewer) = start();

    let _ = tx.send(event(1));
    let (first, payload) = read_frame(&mut client).await?;
    assert_eq!((first, id(&payload)?), (0x81, Some(1)));

    client.write_all(&client_frame(0x89, b"hi")).await?;
    assert_eq!(read_frame(&mut client).await?, (0x8a, b"hi".to_vec()));

    let _ = tx.send(event(2));
    let (first, payload) = read_frame(&mut client).await?;
    assert_eq!((first, id(&payload)?), (0x81, Some(2)));
    Ok(())
  }

  // ---- answering the client ----

  #[tokio::test]
  async fn answers_a_ping_with_a_pong_carrying_its_payload() -> TestResult {
    let (mut client, _tx, _viewer) = start();

    client.write_all(&client_frame(0x89, b"abc")).await?;

    assert_eq!(read_frame(&mut client).await?, (0x8a, b"abc".to_vec()));
    Ok(())
  }

  #[tokio::test]
  async fn answers_every_ping_that_arrives_in_one_read() -> TestResult {
    let (mut client, _tx, _viewer) = start();
    let mut bytes = client_frame(0x89, b"one");
    bytes.extend(client_frame(0x89, b"two"));

    client.write_all(&bytes).await?;

    assert_eq!(read_frame(&mut client).await?, (0x8a, b"one".to_vec()));
    assert_eq!(read_frame(&mut client).await?, (0x8a, b"two".to_vec()));
    Ok(())
  }

  #[tokio::test]
  async fn answers_each_ping_once_across_reads() -> TestResult {
    let (mut client, _tx, _viewer) = start();

    client.write_all(&client_frame(0x89, b"one")).await?;
    assert_eq!(read_frame(&mut client).await?, (0x8a, b"one".to_vec()));
    client.write_all(&client_frame(0x89, b"two")).await?;
    assert_eq!(read_frame(&mut client).await?, (0x8a, b"two".to_vec()));
    Ok(())
  }

  #[tokio::test]
  async fn waits_for_the_rest_of_a_split_frame() -> TestResult {
    let (mut client, _tx, _viewer) = start();
    let bytes = client_frame(0x89, b"split");
    let (front, back) = bytes.split_at_checked(4).ok_or("frame too short")?;

    client.write_all(front).await?;
    // Nothing comes back while the frame is incomplete.
    let early =
      timeout(Duration::from_millis(100), read_frame(&mut client)).await;
    assert!(early.is_err());

    client.write_all(back).await?;
    assert_eq!(read_frame(&mut client).await?, (0x8a, b"split".to_vec()));
    Ok(())
  }

  #[tokio::test]
  async fn ignores_text_binary_and_pong_frames() -> TestResult {
    let (mut client, _tx, _viewer) = start();
    let mut bytes = client_frame(0x81, b"text");
    bytes.extend(client_frame(0x82, &[0x00, 0xff]));
    bytes.extend(client_frame(0x8a, b"pong"));
    bytes.extend(client_frame(0x89, b"ping"));

    client.write_all(&bytes).await?;

    // The only answer is the pong to the ping at the end.
    assert_eq!(read_frame(&mut client).await?, (0x8a, b"ping".to_vec()));
    Ok(())
  }

  // ---- closing ----

  #[tokio::test]
  async fn echoes_the_code_of_a_close_and_stops() -> TestResult {
    let (mut client, _tx, viewer) = start();

    // 1001 (going away) as a big-endian u16, with a reason.
    client
      .write_all(&client_frame(0x88, b"\x03\xe9tab closed"))
      .await?;

    assert_eq!(read_frame(&mut client).await?, (0x88, vec![0x03, 0xe9]));
    assert!(finished(viewer).await?.is_ok());
    Ok(())
  }

  #[tokio::test]
  async fn answers_a_close_without_a_code_with_1000() -> TestResult {
    let (mut client, _tx, viewer) = start();

    client.write_all(&client_frame(0x88, b"")).await?;

    assert_eq!(read_frame(&mut client).await?, (0x88, vec![0x03, 0xe8]));
    assert!(finished(viewer).await?.is_ok());
    Ok(())
  }

  #[tokio::test]
  async fn closes_with_1002_on_a_protocol_error() -> TestResult {
    let (mut client, _tx, viewer) = start();

    // An unmasked text frame: clients must mask.
    client.write_all(&[0x81, 0x00]).await?;

    let (first, payload) = read_frame(&mut client).await?;
    assert_eq!(first, 0x88);
    assert!(payload.starts_with(&[0x03, 0xea]));
    assert!(finished(viewer).await?.is_ok());
    Ok(())
  }

  #[tokio::test]
  async fn closes_with_1009_on_an_oversized_frame() -> TestResult {
    let (mut client, _tx, viewer) = start();

    // A binary frame announcing 126 bytes, one more than a viewer accepts.
    client.write_all(&[0x82, 0xfe, 0x00, 0x7e]).await?;

    assert_eq!(read_frame(&mut client).await?, (0x88, vec![0x03, 0xf1]));
    assert!(finished(viewer).await?.is_ok());
    Ok(())
  }

  #[tokio::test]
  async fn ignores_frames_after_a_close() -> TestResult {
    let (mut client, _tx, viewer) = start();
    let mut bytes = client_frame(0x88, &[0x03, 0xe8]);
    bytes.extend(client_frame(0x89, b"late"));

    client.write_all(&bytes).await?;

    assert_eq!(read_frame(&mut client).await?, (0x88, vec![0x03, 0xe8]));
    assert!(finished(viewer).await?.is_ok());
    // The ping after the close was never answered.
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await?;
    assert!(rest.is_empty());
    Ok(())
  }

  // ---- making progress ----

  #[tokio::test]
  async fn never_gets_stuck_behind_ignored_frames() -> TestResult {
    // The safety net: 400 frames the viewer has no answer for, spread over
    // several reads, then a ping. A viewer that stops taking frames off its
    // buffer, or takes the same one again, never reaches the ping, and this
    // fails on the timeout in `read_frame` instead of hanging.
    let (mut client, _tx, _viewer) = start();
    let mut bytes = Vec::new();
    for _ in 0..400 {
      bytes.extend(client_frame(0x81, b""));
    }
    bytes.extend(client_frame(0x89, b"still here"));

    client.write_all(&bytes).await?;

    let answer = read_frame(&mut client).await?;
    assert_eq!(answer, (0x8a, b"still here".to_vec()));
    Ok(())
  }

  #[tokio::test]
  async fn lets_other_tasks_run_while_it_works_through_frames() -> TestResult {
    // 150 empty text frames and a ping arrive in a single read. A second
    // task on the viewer's thread counts how often it gets to run while
    // the viewer works through them. A viewer that yields between frames
    // lets it run about once per frame; one that does not lets it run
    // once at most.
    let ticks = isolated(async {
      let (_tx, rx) = broadcast::channel::<Event>(16);
      let (mut server, mut client) = tokio::io::duplex(65_536);
      let count = Arc::new(AtomicUsize::new(0));
      let counter = Arc::clone(&count);
      tokio::spawn(async move {
        loop {
          counter.fetch_add(1, Ordering::Relaxed);
          tokio::task::yield_now().await;
        }
      });
      tokio::spawn(async move { stream_events(&mut server, rx).await });

      let mut bytes = Vec::new();
      for _ in 0..150 {
        bytes.extend(client_frame(0x81, b""));
      }
      bytes.extend(client_frame(0x89, b""));
      client.write_all(&bytes).await.ok()?;
      let before = count.load(Ordering::Relaxed);
      read_frame(&mut client).await.ok()?;
      Some(count.load(Ordering::Relaxed).saturating_sub(before))
    });

    let ticks = timeout(WAIT, ticks)
      .await??
      .ok_or("viewer did not answer")?;
    assert!(ticks >= 100, "other task ran only {ticks} times");
    Ok(())
  }

  // ---- the client going away ----

  #[tokio::test]
  async fn returns_ok_when_the_client_disconnects() -> TestResult {
    let (client, _tx, viewer) = start();

    drop(client);

    assert!(finished(viewer).await?.is_ok());
    Ok(())
  }

  #[tokio::test]
  async fn stops_when_the_client_has_gone_with_an_event_pending() -> TestResult
  {
    // `tx` stays alive, so only the client going can end the loop. Whether
    // that shows up as a failed write or as end of file on the read side,
    // the loop must not hang.
    let (tx, rx) = broadcast::channel::<Event>(16);
    let (mut server, client) = tokio::io::duplex(64);
    drop(client);
    let _ = tx.send(event(1));

    let outcome = timeout(WAIT, stream_events(&mut server, rx)).await;

    assert!(outcome.is_ok());
    Ok(())
  }
}
