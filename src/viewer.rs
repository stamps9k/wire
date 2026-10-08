//! WebSocket viewers: after a successful upgrade, a connection subscribes to
//! the event channel and receives every event as a text frame of JSON until
//! the client goes away.

use std::io;

use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use crate::event::Event;
use crate::websocket;

/// Sends every event received on `rx` to `writer` until the client goes
/// away.
///
/// Each event is serialised to JSON and written as one WebSocket text frame
/// built by [`websocket::text_frame`]. Only events sent after `rx` was
/// subscribed are seen. If the viewer falls behind and the channel drops
/// events, the missed ones are skipped and streaming carries on with the
/// next. An event that cannot be serialised is skipped.
///
/// Returns `Ok(())` when the channel closes. In the server that never
/// happens while the connection holds a sender, so in practice the loop
/// only ends with a write error.
///
/// # Errors
///
/// Returns the error from the first failed write, which usually means the
/// client has disconnected.
pub async fn stream_events<W>(
  writer: &mut W,
  mut rx: Receiver<Event>,
) -> io::Result<()>
where
  W: AsyncWrite + Unpin,
{
  loop {
    let event = match rx.recv().await {
      Ok(v) => v,
      Err(RecvError::Lagged(_)) => continue,
      Err(RecvError::Closed) => break,
    };

    let Ok(event_s) = serde_json::to_string(&event) else {
      continue;
    };
    writer
      .write_all(&websocket::text_frame(event_s.as_bytes()))
      .await?;
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
  use std::time::Duration;

  use serde_json::Value;
  use tokio::sync::broadcast;
  use tokio::time::timeout;

  type TestResult = Result<(), Box<dyn Error>>;

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

  #[tokio::test]
  async fn writes_nothing_when_the_channel_closes_without_events() -> TestResult
  {
    let (tx, rx) = broadcast::channel::<Event>(16);
    drop(tx);

    let mut written = Vec::new();
    stream_events(&mut written, rx).await?;

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

    let mut written = Vec::new();
    stream_events(&mut written, rx).await?;

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

    let mut written = Vec::new();
    stream_events(&mut written, rx).await?;

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

    let mut written = Vec::new();
    stream_events(&mut written, rx).await?;

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

    let mut written = Vec::new();
    stream_events(&mut written, rx).await?;

    let payloads = payloads(&written).ok_or("not a run of text frames")?;
    assert_eq!(ids(&payloads)?, [Some(2)]);
    Ok(())
  }

  #[tokio::test]
  async fn stops_with_an_error_when_the_client_has_gone() -> TestResult {
    // `tx` stays alive, so only the failed write can end the loop.
    let (tx, rx) = broadcast::channel::<Event>(16);
    let (mut server, client) = tokio::io::duplex(64);
    drop(client);
    let _ = tx.send(event(1));

    let outcome =
      timeout(Duration::from_secs(5), stream_events(&mut server, rx)).await?;

    assert!(outcome.is_err());
    Ok(())
  }

  #[tokio::test]
  async fn waits_for_more_events_while_the_channel_is_open() -> TestResult {
    let (tx, rx) = broadcast::channel::<Event>(16);
    let _ = tx.send(event(1));

    let mut written = Vec::new();
    let outcome =
      timeout(Duration::from_millis(100), stream_events(&mut written, rx))
        .await;

    // Timing out is the pass: the loop was still waiting on the channel.
    assert!(outcome.is_err());
    assert_eq!(payloads(&written).map(|found| found.len()), Some(1));
    Ok(())
  }
}
