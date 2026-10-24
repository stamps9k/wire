//! [`ViewerMessage`]: what the viewer feed sends to WebSocket viewers. Each
//! message is either one connection's [`Event`] or a [`Summary`] of the
//! events the feed dropped in one window.

use serde::Serialize;

use crate::event::Event;
use crate::event::Kind;
use crate::event::RejectReason;
use crate::event::ShedReason;

/// One message sent to WebSocket viewers, serialised as a single flat JSON
/// object.
///
/// The enum is `untagged`, so it adds nothing of its own to the JSON: each
/// variant serialises exactly as its contents would. Both carry a `kind`
/// field, so a client tells them apart by `kind` alone: `request`,
/// `rejected` and `shed` are connection events, and `summary` is a
/// [`Summary`].
#[derive(Serialize)]
#[serde(untagged)]
pub enum ViewerMessage {
  Event(Event),
  Summary(Summary),
}

/// What the viewer feed left out of one window, sent at the end of any
/// window in which events were dropped or missed.
///
/// Serialised with `"kind": "summary"` in front of its fields, so it sits
/// alongside the connection events as a fourth kind.
#[derive(Serialize)]
#[serde(tag = "kind", rename = "summary")]
pub struct Summary {
  /// When the window started, in milliseconds since the Unix epoch.
  pub window_start_ms: u64,
  /// The length of the window in milliseconds.
  pub window_ms: u64,
  /// Events the feed never received because it fell behind the event
  /// channel and they were overwritten. Their kinds are unknown, so they
  /// are not in [`dropped`](Self::dropped).
  pub lagged: u32,
  /// Events the feed received but did not send because the window's budget
  /// was used up, counted by kind.
  pub dropped: Dropped,
}

/// Counts of the events dropped in one window, grouped by kind and then by
/// status class or reason.
///
/// Every count is always present in the JSON, even when it is 0, so clients
/// see a fixed shape. Built with `Default` and updated only through
/// [`count`](Self::count).
#[derive(Serialize, Default)]
pub struct Dropped {
  request: RequestDropped,
  rejected: RejectedDropped,
  shed: ShedDropped,
}

/// Dropped requests, counted by the class of the response wire sent: `s1`
/// for 1xx up to `s5` for 5xx.
#[derive(Default, Serialize)]
pub struct RequestDropped {
  s1: u32,
  s2: u32,
  s3: u32,
  s4: u32,
  s5: u32,
}

/// Dropped rejections, counted by [`RejectReason`].
#[derive(Default, Serialize)]
pub struct RejectedDropped {
  closed: u32,
  io: u32,
  too_large: u32,
  malformed: u32,
  timeout: u32,
}

/// Dropped sheds, counted by [`ShedReason`].
#[derive(Default, Serialize)]
pub struct ShedDropped {
  ip_limit: u32,
  capacity: u32,
}

impl Dropped {
  /// Counts one dropped event of `kind`.
  ///
  /// Requests are counted by the class of their status. Rejections and sheds
  /// are counted by their reason. A status outside 100–599, which wire never
  /// sends, is not counted. Counts saturate at `u32::MAX` instead of
  /// overflowing.
  ///
  /// Every variant is named in the matches, so a new one does not compile
  /// until it has been decided here where it is counted.
  pub fn count(&mut self, kind: &Kind) {
    let counter: &mut u32 = match kind {
      Kind::Rejected { reason, .. } => match reason {
        RejectReason::Closed => &mut self.rejected.closed,
        RejectReason::Io => &mut self.rejected.io,
        RejectReason::Malformed => &mut self.rejected.malformed,
        RejectReason::Timeout => &mut self.rejected.timeout,
        RejectReason::TooLarge => &mut self.rejected.too_large,
      },
      Kind::Shed { reason } => match reason {
        ShedReason::Capacity => &mut self.shed.capacity,
        ShedReason::IpLimit => &mut self.shed.ip_limit,
      },
      Kind::Request { status, .. } => match status {
        0..100 | 600.. => return,
        100..200 => &mut self.request.s1,
        200..300 => &mut self.request.s2,
        300..400 => &mut self.request.s3,
        400..500 => &mut self.request.s4,
        500..600 => &mut self.request.s5,
      },
    };
    *counter = counter.saturating_add(1);
  }
}

#[cfg(test)]
mod tests {
  use std::net::SocketAddr;

  use serde_json::Value;
  use serde_json::json;

  use super::*;
  use crate::event::Kind;
  use crate::event::RejectReason;
  use crate::event::ShedReason;
  use crate::http::HostClass;

  /// A request kind with `status`, for counting tests.
  fn request(status: u16) -> Kind {
    Kind::Request {
      method: "GET".to_string(),
      target: "/".to_string(),
      target_length: 1,
      version: "HTTP/1.1".to_string(),
      status,
      user_agent: None,
      user_agent_length: 0,
      host_class: HostClass::Own,
      header_count: 1,
    }
  }

  /// A rejected kind with `reason`, for counting tests.
  fn rejected(reason: RejectReason) -> Kind {
    Kind::Rejected {
      reason,
      detail: None,
      status: None,
    }
  }

  /// The JSON of a `Dropped` holding these three groups.
  fn dropped_json(request: &Value, rejected: &Value, shed: &Value) -> Value {
    json!({ "request": request, "rejected": rejected, "shed": shed })
  }

  /// The JSON of a request group with nothing counted.
  fn no_requests() -> Value {
    json!({ "s1": 0, "s2": 0, "s3": 0, "s4": 0, "s5": 0 })
  }

  /// The JSON of a rejected group with nothing counted.
  fn no_rejections() -> Value {
    json!({ "closed": 0, "io": 0, "too_large": 0, "malformed": 0, "timeout": 0 })
  }

  /// The JSON of a shed group with nothing counted.
  fn no_sheds() -> Value {
    json!({ "ip_limit": 0, "capacity": 0 })
  }

  #[test]
  fn a_summary_is_a_flat_object_with_kind_summary()
  -> Result<(), serde_json::Error> {
    let summary = ViewerMessage::Summary(Summary {
      window_start_ms: 5000,
      window_ms: 1000,
      lagged: 3,
      dropped: Dropped::default(),
    });

    assert_eq!(
      serde_json::to_value(&summary)?,
      json!({
        "kind": "summary",
        "window_start_ms": 5000,
        "window_ms": 1000,
        "lagged": 3,
        "dropped": dropped_json(&no_requests(), &no_rejections(), &no_sheds()),
      })
    );
    Ok(())
  }

  #[test]
  fn an_event_message_serialises_exactly_like_the_event()
  -> Result<(), serde_json::Error> {
    let event = Event {
      connection_id: 7,
      peer_address: SocketAddr::from(([203, 0, 113, 0], 0)),
      timestamp_ms: 1234,
      duration_ms: 5,
      bytes_received: 40,
      tcp_snapshot: None,
      kind: request(200),
    };
    let expected = serde_json::to_value(&event)?;

    assert_eq!(serde_json::to_value(ViewerMessage::Event(event))?, expected);
    Ok(())
  }

  #[test]
  fn requests_are_counted_by_status_class() -> Result<(), serde_json::Error> {
    let mut dropped = Dropped::default();
    for status in [101, 200, 200, 404, 405, 405, 503] {
      dropped.count(&request(status));
    }

    assert_eq!(
      serde_json::to_value(&dropped)?,
      dropped_json(
        &json!({ "s1": 1, "s2": 2, "s3": 0, "s4": 3, "s5": 1 }),
        &no_rejections(),
        &no_sheds()
      )
    );
    Ok(())
  }

  #[test]
  fn rejections_are_counted_by_reason() -> Result<(), serde_json::Error> {
    let mut dropped = Dropped::default();
    for reason in [
      RejectReason::Closed,
      RejectReason::Io,
      RejectReason::Io,
      RejectReason::TooLarge,
      RejectReason::Malformed,
      RejectReason::Malformed,
      RejectReason::Malformed,
      RejectReason::Timeout,
    ] {
      dropped.count(&rejected(reason));
    }

    assert_eq!(
      serde_json::to_value(&dropped)?,
      dropped_json(
        &no_requests(),
        &json!({
          "closed": 1, "io": 2, "too_large": 1, "malformed": 3, "timeout": 1
        }),
        &no_sheds()
      )
    );
    Ok(())
  }

  #[test]
  fn sheds_are_counted_by_reason() -> Result<(), serde_json::Error> {
    let mut dropped = Dropped::default();
    for reason in [
      ShedReason::IpLimit,
      ShedReason::IpLimit,
      ShedReason::Capacity,
    ] {
      dropped.count(&Kind::Shed { reason });
    }

    assert_eq!(
      serde_json::to_value(&dropped)?,
      dropped_json(
        &no_requests(),
        &no_rejections(),
        &json!({ "ip_limit": 2, "capacity": 1 })
      )
    );
    Ok(())
  }

  #[test]
  fn a_counter_saturates_instead_of_overflowing()
  -> Result<(), serde_json::Error> {
    let mut dropped = Dropped::default();
    dropped.shed.ip_limit = u32::MAX;
    dropped.count(&Kind::Shed {
      reason: ShedReason::IpLimit,
    });

    assert_eq!(
      serde_json::to_value(&dropped)?,
      dropped_json(
        &no_requests(),
        &no_rejections(),
        &json!({ "ip_limit": u32::MAX, "capacity": 0 })
      )
    );
    Ok(())
  }
}
