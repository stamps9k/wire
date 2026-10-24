//! The viewer feed: the stream of messages sent to WebSocket viewers.
//!
//! One task subscribes to the event channel and forwards at most
//! [`FEED_RATE`] events per [`FEED_WINDOW`] to every viewer. Anything over
//! that is counted and reported in one [`Summary`] at the end of the
//! window. Each message is trimmed for viewers, serialised and framed once
//! here, and every viewer writes the same shared bytes. Standard output is
//! not affected: it subscribes to the event channel directly and still gets
//! every event in full.

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::Sender;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;

use crate::event::Dropped;
use crate::event::Event;
use crate::event::Kind;
use crate::event::Summary;
use crate::event::ViewerMessage;
use crate::websocket::Frame;

/// The most events sent to viewers in one [`FEED_WINDOW`].
///
/// Every viewer receives every message the feed sends, so this caps what
/// one burst of connections can make the server send: about this many
/// events per window to each viewer, plus one summary, however many
/// connections arrive.
const FEED_RATE: u32 = 50;

/// The length of a window. The budget in [`FEED_RATE`] is reset at the end
/// of each one, when its [`Summary`] is sent, if there is one.
const FEED_WINDOW: Duration = Duration::from_secs(1);

/// Forwards events from `events` to viewers on `viewer_feed`, within a
/// budget of [`FEED_RATE`] per [`FEED_WINDOW`].
///
/// While the window has budget, each event has its peer address reduced
/// with [`truncate_peer_address`] and its rejection detail removed with
/// [`truncate_kind`], and is then sent as a [`ViewerMessage::Event`]. Once
/// the budget is used up, events are counted in a [`Dropped`] by kind and
/// not sent. If this task falls behind the event channel, the events it
/// missed are counted as `lagged`.
///
/// At the end of each window, if anything was dropped or missed, one
/// [`ViewerMessage::Summary`] is sent with the counts. Then the counts and
/// the budget are reset and the next window starts. A window with nothing
/// dropped or missed sends no summary.
///
/// Every message is sent as a finished WebSocket text frame, so viewers
/// write it to their sockets as it is. A failed send only means that no
/// viewer is connected, so it is ignored.
///
/// Returns when the event channel closes. In the server that never happens,
/// because `main` holds a sender for as long as it runs.
pub async fn run_viewer_feed(
  mut events: Receiver<Event>,
  tx_viewer_feed: Sender<Arc<[u8]>>,
) {
  let mut interval = interval(FEED_WINDOW);
  interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
  // Skip the first tick
  interval.tick().await;

  let mut window_start_ms = now_ms();

  let mut event_count: u32 = 0;
  let mut lagged: u32 = 0;

  let mut dropped = None;

  loop {
    tokio::select! {
      result = events.recv() => {
        let mut event = match result {
          Ok(v) => {
            event_count = event_count.saturating_add(1);
            v
          },
          Err(RecvError::Lagged(n)) => {
            lagged = lagged.saturating_add(
              u32::try_from(n).unwrap_or(u32::MAX));
            continue;
          },
          Err(RecvError::Closed) => break,
        };

        if event_count > FEED_RATE {
          dropped.get_or_insert_with(Dropped::default).count(&event.kind);
        } else {
          event.peer_address = truncate_peer_address(event.peer_address);
          event.kind = truncate_kind(event.kind);
          let message = ViewerMessage::Event(event);
          publish_message(&message, &tx_viewer_feed);
        }
      },
      _ = interval.tick() => {
        event_count = 0;

        if dropped.is_some() || lagged > 0 {
          let message = ViewerMessage::Summary(Summary {
            window_start_ms,
            window_ms: u64::try_from(
              FEED_WINDOW.as_millis())
              .unwrap_or(u64::MAX),
            lagged,
            dropped: dropped.take().unwrap_or_default(),
          });

          publish_message(&message, &tx_viewer_feed);
        }
        window_start_ms = now_ms();
        lagged = 0;
      }
    }
  }
}

/// Serialises `message` to JSON, builds a WebSocket text frame around it,
/// and sends the frame's bytes to every viewer subscribed to `tx`.
///
/// A message that cannot be serialised is skipped. A failed send only means
/// that no viewer is connected, so it is ignored.
fn publish_message(message: &ViewerMessage, tx: &Sender<Arc<[u8]>>) {
  let Ok(json) = serde_json::to_string(message) else {
    return;
  };
  let frame = Arc::from(Frame::text(json.as_bytes()).to_bytes());
  let _ = tx.send(frame);
}

/// Removes from `kind` what viewers should not see.
///
/// A rejection's `detail`, which names the parser check that failed, is set
/// to `None`, so watching the stream does not reveal what the generic 400
/// response hides. The field stays in the JSON as `null`, so every event of
/// a kind has the same shape. Every other variant is returned unchanged.
///
/// Every variant and field is named in the match, so a new one does not
/// compile until it has been decided here what viewers may see of it.
fn truncate_kind(kind: Kind) -> Kind {
  match kind {
    Kind::Rejected {
      reason,
      detail: _,
      status,
    } => Kind::Rejected {
      reason,
      detail: None,
      status,
    },
    Kind::Request {
      method: _,
      target: _,
      target_length: _,
      version: _,
      status: _,
      user_agent: _,
      user_agent_length: _,
      host_class: _,
      header_count: _,
    }
    | Kind::Shed { reason: _ } => kind,
  }
}

/// Reduces `address` to its network, for showing to viewers.
///
/// IPv4 addresses keep their first 24 bits and IPv6 addresses their first
/// 48. The port is set to 0. An IPv4-mapped IPv6 address is first turned
/// back into IPv4, so it is cut to a /24 and does not keep the whole IPv4
/// address inside a /48.
///
/// Viewers see a JSON string like `203.0.113.0:0` or `[2001:db8:abcd::]:0`,
/// which should be read as the /24 or /48 network.
fn truncate_peer_address(address: SocketAddr) -> SocketAddr {
  let masked_ip = match address.ip().to_canonical() {
    IpAddr::V4(v4) => {
      IpAddr::V4(Ipv4Addr::from_bits(v4.to_bits() & 0xFFFF_FF00))
    }
    IpAddr::V6(v6) => {
      IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & (!0u128 << 80)))
    }
  };

  SocketAddr::new(masked_ip, 0)
}

/// The current time in milliseconds since the Unix epoch.
///
/// A clock set before the epoch gives 0, and a time too large for a `u64`
/// gives `u64::MAX`, so this never fails.
fn now_ms() -> u64 {
  u64::try_from(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_millis(),
  )
  .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
  use super::*;

  use std::error::Error;

  use serde_json::Value;
  use serde_json::json;
  use tokio::sync::broadcast;
  use tokio::task::JoinHandle;
  use tokio::time::timeout;

  use crate::event::RejectReason;
  use crate::event::ShedReason;
  use crate::http::HostClass;

  type TestResult = Result<(), Box<dyn Error>>;

  /// How long to wait for a message that should arrive. Longer than a
  /// window, so a summary sent at the end of one arrives in time. The tests
  /// run on a paused clock, so waiting costs no real time.
  const WAIT: Duration = Duration::from_secs(5);

  /// Starts [`run_viewer_feed`] on an event channel holding `capacity`
  /// events, and returns the event sender, a viewer subscribed to the feed,
  /// and the feed's task.
  ///
  /// The feed's channel holds plenty, so the test viewer never lags. The
  /// task does not start running until the test first waits, so events sent
  /// straight after this all arrive in the first window.
  fn start(
    capacity: usize,
  ) -> (Sender<Event>, Receiver<Arc<[u8]>>, JoinHandle<()>) {
    let (events, rx_events) = broadcast::channel::<Event>(capacity);
    let feed = Sender::<Arc<[u8]>>::new(1024);
    let viewer = feed.subscribe();
    let task = tokio::spawn(run_viewer_feed(rx_events, feed));
    (events, viewer, task)
  }

  /// A rejected event from a full address, with a rejection detail, as
  /// connection `connection_id`.
  fn event(connection_id: u64) -> Event {
    Event {
      connection_id,
      peer_address: SocketAddr::from(([203, 0, 113, 9], 51_234)),
      timestamp_ms: 1_700_000_000_000,
      duration_ms: 12,
      bytes_received: 78,
      tcp_snapshot: None,
      kind: Kind::Rejected {
        reason: RejectReason::Malformed,
        detail: Some("bad target"),
        status: Some(400),
      },
    }
  }

  /// An event of `kind`, otherwise like [`event`].
  fn event_of(connection_id: u64, kind: Kind) -> Event {
    Event {
      kind,
      ..event(connection_id)
    }
  }

  /// A request kind answered with `status`.
  fn request(status: u16) -> Kind {
    Kind::Request {
      method: "GET".to_owned(),
      target: "/".to_owned(),
      target_length: 1,
      version: "HTTP/1.1".to_owned(),
      status,
      user_agent: None,
      user_agent_length: 0,
      host_class: HostClass::Own,
      header_count: 1,
    }
  }

  /// Checks that `frame` is exactly one unmasked text frame and returns its
  /// payload parsed as JSON. Understands the one-byte and two-byte lengths
  /// only.
  fn parse(frame: &[u8]) -> Result<Value, Box<dyn Error>> {
    let (length, payload) = match frame {
      [0x81, 126, high, low, payload @ ..] => {
        (usize::from(u16::from_be_bytes([*high, *low])), payload)
      }
      [0x81, short, payload @ ..] if *short < 126 => {
        (usize::from(*short), payload)
      }
      _ => return Err("not an unmasked text frame".into()),
    };
    if payload.len() != length {
      return Err("frame length does not match its payload".into());
    }
    Ok(serde_json::from_slice(payload)?)
  }

  /// Waits for the next message on `viewer` and parses it.
  async fn next(
    viewer: &mut Receiver<Arc<[u8]>>,
  ) -> Result<Value, Box<dyn Error>> {
    let frame = timeout(WAIT, viewer.recv()).await??;
    parse(&frame)
  }

  /// The `kind` field of a message.
  fn kind(message: &Value) -> Option<&str> {
    message.get("kind").and_then(Value::as_str)
  }

  /// The `connection_id` field of an event message.
  fn id(message: &Value) -> Option<u64> {
    message.get("connection_id").and_then(Value::as_u64)
  }

  /// Receives `count` messages and checks that they are the events with
  /// ids `first` onwards, in order.
  async fn expect_events(
    viewer: &mut Receiver<Arc<[u8]>>,
    first: u64,
    count: u32,
  ) -> TestResult {
    for offset in 0..u64::from(count) {
      let message = next(viewer).await?;
      assert_eq!(kind(&message), Some("rejected"));
      assert_eq!(id(&message), Some(first.saturating_add(offset)));
    }
    Ok(())
  }

  /// Sends the events with ids `first` up to and including `last`.
  fn send_events(events: &Sender<Event>, first: u64, last: u64) {
    for connection_id in first..=last {
      let _ = events.send(event(connection_id));
    }
  }

  /// The `dropped` JSON of a summary with only `malformed` rejections
  /// counted.
  fn malformed_only(count: u32) -> Value {
    json!({
      "request": { "s1": 0, "s2": 0, "s3": 0, "s4": 0, "s5": 0 },
      "rejected": {
        "closed": 0, "io": 0, "too_large": 0, "malformed": count, "timeout": 0
      },
      "shed": { "ip_limit": 0, "capacity": 0 }
    })
  }

  /// Checks that `message` is a summary of one [`FEED_WINDOW`] with
  /// `lagged` and `dropped`.
  fn assert_summary(message: &Value, lagged: u32, dropped: &Value) {
    assert_eq!(kind(message), Some("summary"));
    assert_eq!(
      message.get("window_ms").and_then(Value::as_u64),
      u64::try_from(FEED_WINDOW.as_millis()).ok()
    );
    assert!(
      message
        .get("window_start_ms")
        .and_then(Value::as_u64)
        .is_some_and(|start| start > 0)
    );
    assert_eq!(
      message.get("lagged").and_then(Value::as_u64),
      Some(u64::from(lagged))
    );
    assert_eq!(message.get("dropped"), Some(dropped));
  }

  // ---- forwarding ----

  #[tokio::test(start_paused = true)]
  async fn forwards_an_event_as_one_text_frame_of_its_trimmed_json()
  -> TestResult {
    let (events, mut viewer, _task) = start(16);
    let original = event(1);
    let mut trimmed = original.clone();
    trimmed.peer_address = SocketAddr::from(([203, 0, 113, 0], 0));
    trimmed.kind = Kind::Rejected {
      reason: RejectReason::Malformed,
      detail: None,
      status: Some(400),
    };
    let _ = events.send(original);

    let message = next(&mut viewer).await?;

    assert_eq!(message, serde_json::to_value(&trimmed)?);
    Ok(())
  }

  #[tokio::test(start_paused = true)]
  async fn never_sends_the_full_peer_address() -> TestResult {
    let (events, mut viewer, _task) = start(16);
    let _ = events.send(event(1));

    let frame = timeout(WAIT, viewer.recv()).await??;

    let text = String::from_utf8_lossy(&frame);
    assert!(text.contains(r#""peer_address":"203.0.113.0:0""#));
    assert!(!text.contains("203.0.113.9"));
    assert!(!text.contains("51234"));
    Ok(())
  }

  #[tokio::test(start_paused = true)]
  async fn never_sends_the_rejection_detail() -> TestResult {
    let (events, mut viewer, _task) = start(16);
    let _ = events.send(event(1));

    let frame = timeout(WAIT, viewer.recv()).await??;

    let text = String::from_utf8_lossy(&frame);
    assert!(text.contains(r#""reason":"malformed","detail":null"#));
    assert!(!text.contains("bad target"));
    Ok(())
  }

  // ---- the budget ----

  #[tokio::test(start_paused = true)]
  async fn sends_at_most_feed_rate_events_then_one_summary() -> TestResult {
    let (events, mut viewer, _task) = start(256);
    let total = u64::from(FEED_RATE).saturating_add(10);
    send_events(&events, 1, total);

    expect_events(&mut viewer, 1, FEED_RATE).await?;
    let summary = next(&mut viewer).await?;

    assert_summary(&summary, 0, &malformed_only(10));
    Ok(())
  }

  #[tokio::test(start_paused = true)]
  async fn a_window_with_nothing_dropped_sends_no_summary() -> TestResult {
    let (events, mut viewer, _task) = start(16);
    send_events(&events, 1, 3);
    expect_events(&mut viewer, 1, 3).await?;

    // Several windows end while nothing more arrives.
    let quiet = timeout(FEED_WINDOW * 3, viewer.recv()).await;

    assert!(quiet.is_err(), "a summary was sent for a quiet window");
    Ok(())
  }

  #[tokio::test(start_paused = true)]
  async fn a_summary_is_sent_once_and_not_repeated() -> TestResult {
    let (events, mut viewer, _task) = start(256);
    let total = u64::from(FEED_RATE).saturating_add(1);
    send_events(&events, 1, total);
    expect_events(&mut viewer, 1, FEED_RATE).await?;
    assert_summary(&next(&mut viewer).await?, 0, &malformed_only(1));

    // The windows after it drop nothing, so they send nothing.
    let quiet = timeout(FEED_WINDOW * 3, viewer.recv()).await;

    assert!(quiet.is_err(), "the summary was sent again");
    Ok(())
  }

  #[tokio::test(start_paused = true)]
  async fn the_budget_resets_in_the_next_window() -> TestResult {
    let (events, mut viewer, _task) = start(256);
    let rate = u64::from(FEED_RATE);
    let first_total = rate.saturating_add(10);
    send_events(&events, 1, first_total);
    expect_events(&mut viewer, 1, FEED_RATE).await?;
    assert_summary(&next(&mut viewer).await?, 0, &malformed_only(10));

    // The summary marks the start of the next window, with a full budget.
    let second_first = first_total.saturating_add(1);
    let second_total = first_total.saturating_add(rate).saturating_add(20);
    send_events(&events, second_first, second_total);

    expect_events(&mut viewer, second_first, FEED_RATE).await?;
    assert_summary(&next(&mut viewer).await?, 0, &malformed_only(20));
    Ok(())
  }

  #[tokio::test(start_paused = true)]
  async fn a_summary_counts_each_kind_separately() -> TestResult {
    let (events, mut viewer, _task) = start(256);
    send_events(&events, 1, u64::from(FEED_RATE));
    let mut connection_id = u64::from(FEED_RATE);
    let mut send = |kind: Kind| {
      connection_id = connection_id.saturating_add(1);
      let _ = events.send(event_of(connection_id, kind));
    };
    send(request(200));
    send(request(404));
    send(request(404));
    send(request(503));
    send(Kind::Rejected {
      reason: RejectReason::Timeout,
      detail: None,
      status: Some(408),
    });
    send(Kind::Shed {
      reason: ShedReason::IpLimit,
    });
    send(Kind::Shed {
      reason: ShedReason::Capacity,
    });
    send(Kind::Shed {
      reason: ShedReason::Capacity,
    });

    expect_events(&mut viewer, 1, FEED_RATE).await?;
    let summary = next(&mut viewer).await?;

    assert_summary(
      &summary,
      0,
      &json!({
        "request": { "s1": 0, "s2": 1, "s3": 0, "s4": 2, "s5": 1 },
        "rejected": {
          "closed": 0, "io": 0, "too_large": 0, "malformed": 0, "timeout": 1
        },
        "shed": { "ip_limit": 1, "capacity": 2 }
      }),
    );
    Ok(())
  }

  // ---- falling behind ----

  #[tokio::test(start_paused = true)]
  async fn events_the_feed_missed_are_reported_as_lagged() -> TestResult {
    // The event channel holds four events, and the feed does not run until
    // the test waits, so the first six of these ten are overwritten before
    // it reads any.
    let (events, mut viewer, _task) = start(4);
    send_events(&events, 1, 10);

    expect_events(&mut viewer, 7, 4).await?;
    let summary = next(&mut viewer).await?;

    // Nothing was dropped for the budget, but the lag alone is reported.
    assert_summary(&summary, 6, &malformed_only(0));
    Ok(())
  }

  // ---- stopping ----

  #[tokio::test(start_paused = true)]
  async fn stops_when_the_event_channel_closes() -> TestResult {
    let (events, mut viewer, task) = start(16);

    drop(events);

    timeout(WAIT, task).await??;
    // The feed's sender went with the task, so viewers see the feed close.
    assert!(matches!(viewer.recv().await, Err(RecvError::Closed)));
    Ok(())
  }

  // ---- trimming ----

  fn address(text: &str) -> Result<SocketAddr, std::net::AddrParseError> {
    text.parse()
  }

  #[test]
  fn truncate_keeps_the_ipv4_slash_24_and_drops_the_port() -> TestResult {
    let truncated = truncate_peer_address(address("203.0.113.9:51234")?);
    assert_eq!(truncated, address("203.0.113.0:0")?);
    Ok(())
  }

  #[test]
  fn truncate_zeroes_the_whole_last_ipv4_octet() -> TestResult {
    let truncated = truncate_peer_address(address("198.51.100.255:1")?);
    assert_eq!(truncated, address("198.51.100.0:0")?);
    Ok(())
  }

  #[test]
  fn truncate_keeps_the_ipv6_slash_48_and_drops_the_port() -> TestResult {
    let original = address("[2001:db8:abcd:12:34:56:78:9a]:443")?;
    assert_eq!(
      truncate_peer_address(original),
      address("[2001:db8:abcd::]:0")?
    );
    Ok(())
  }

  #[test]
  fn truncate_cuts_ipv6_exactly_after_the_third_group() -> TestResult {
    let original = address("[2001:db8:ffff:ffff::1]:1")?;
    assert_eq!(
      truncate_peer_address(original),
      address("[2001:db8:ffff::]:0")?
    );
    Ok(())
  }

  #[test]
  fn truncate_treats_ipv4_mapped_ipv6_as_ipv4() -> TestResult {
    let truncated =
      truncate_peer_address(address("[::ffff:203.0.113.9]:51234")?);
    assert_eq!(truncated, address("203.0.113.0:0")?);
    Ok(())
  }

  #[test]
  fn truncate_is_unchanged_on_an_already_truncated_address() -> TestResult {
    let once = truncate_peer_address(address("203.0.113.9:51234")?);
    assert_eq!(truncate_peer_address(once), once);
    Ok(())
  }

  /// `kind` as JSON, for comparing kinds, which do not implement
  /// `PartialEq`.
  fn kind_json(kind: &Kind) -> Result<String, serde_json::Error> {
    serde_json::to_string(kind)
  }

  #[test]
  fn truncate_kind_removes_the_rejection_detail() -> TestResult {
    let kind = Kind::Rejected {
      reason: RejectReason::Malformed,
      detail: Some("bad target"),
      status: Some(400),
    };
    let expected = Kind::Rejected {
      reason: RejectReason::Malformed,
      detail: None,
      status: Some(400),
    };
    assert_eq!(kind_json(&truncate_kind(kind))?, kind_json(&expected)?);
    Ok(())
  }

  #[test]
  fn truncate_kind_leaves_a_rejection_without_detail_unchanged() -> TestResult {
    let kind = Kind::Rejected {
      reason: RejectReason::Timeout,
      detail: None,
      status: Some(408),
    };
    let before = kind_json(&kind)?;
    assert_eq!(kind_json(&truncate_kind(kind))?, before);
    Ok(())
  }

  #[test]
  fn truncate_kind_leaves_a_request_unchanged() -> TestResult {
    let kind = Kind::Request {
      method: "GET".to_owned(),
      target: "/?q=1".to_owned(),
      target_length: 5,
      version: "HTTP/1.1".to_owned(),
      status: 200,
      user_agent: Some("curl/8.0".to_owned()),
      user_agent_length: 8,
      host_class: HostClass::Own,
      header_count: 3,
    };
    let before = kind_json(&kind)?;
    assert_eq!(kind_json(&truncate_kind(kind))?, before);
    Ok(())
  }

  #[test]
  fn truncate_kind_leaves_a_shed_unchanged() -> TestResult {
    let kind = Kind::Shed {
      reason: ShedReason::IpLimit,
    };
    let before = kind_json(&kind)?;
    assert_eq!(kind_json(&truncate_kind(kind))?, before);
    Ok(())
  }
}
