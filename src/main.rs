//! wire: a hand-written HTTP/1.1 server on tokio that shows live TCP
//! connection data (`TCP_INFO`) in the browser.

use std::io;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Duration;
use tokio::time::Instant;

use crate::event::Event;
use crate::event::ShedReason;
use crate::server::IpLimit;
use crate::server::run_viewer_feed;
use crate::site::validate_templates;

mod event;
mod http;
mod server;
mod site;
mod websocket;

/// The most connections served at once.
///
/// Each connection holds a permit from a semaphore of this size for as long
/// as its task runs. When none are free, a new connection is still
/// accepted, but it is closed at once without reading from it and published
/// as a shed event (see [`server::shed`]). The accept loop never waits for a
/// permit, so every connection that reaches the server produces an event,
/// even during a flood from many addresses.
///
/// WebSocket viewers count against this limit too, but are also limited
/// separately by [`MAX_VIEWERS`], so they cannot take every connection and
/// block ordinary requests. Keep this well below the process's open file
/// limit (`ulimit -n`).
const MAX_CONNECTIONS: usize = 512;

/// The most WebSocket viewers connected at once.
///
/// Viewers stay connected for as long as the client keeps the page open, so
/// without their own limit they could hold every connection permit. A
/// connection takes a viewer slot when its upgrade is accepted and holds it
/// until the viewer disconnects. An upgrade that arrives when no slot is free
/// is answered with 503 Service Unavailable. Each client address may also
/// hold at most [`MAX_VIEWERS_PER_IP`] of these slots.
const MAX_VIEWERS: usize = 32;

/// The most WebSocket viewers allowed from one client address at once.
///
/// Counted separately from [`MAX_IP_COUNT`], and checked before
/// [`MAX_VIEWERS`] when an upgrade is accepted. An upgrade from an address
/// that already has this many viewers is answered with 503 Service
/// Unavailable, the same response as when every viewer slot is taken. A
/// page opens one viewer, so this leaves room for a second tab or a second
/// user behind the same address, while stopping one client from holding
/// every slot in [`MAX_VIEWERS`].
const MAX_VIEWERS_PER_IP: u32 = 2;

/// The most connections allowed from one client address at once.
///
/// A connection from an address that already has this many open is closed
/// as soon as it is accepted, without reading from it, and published as a
/// shed event (see [`server::shed`]). Browsers open about six connections to
/// a host, and many users can share one address behind carrier-grade NAT,
/// so this leaves room for several real users on one address while stopping
/// one client from taking every permit in [`MAX_CONNECTIONS`].
const MAX_IP_COUNT: u32 = 16;

/// The longest a client may take to send its whole request head.
///
/// The deadline covers the head as a whole, not each read, so a client that
/// sends a byte at a time is cut off as surely as one that sends nothing. A
/// real client sends its head at once, well within this even on a slow
/// mobile link. Until the deadline, a silent client holds a connection
/// permit and one of its address's slots, so a short deadline frees them
/// sooner during a flood. When it passes, the client gets 408 Request
/// Timeout and the event is rejected with reason `timeout`.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Checks the page templates, then binds to port 8080 on all interfaces and
/// serves each accepted connection in its own task.
///
/// Each client address may have at most [`MAX_IP_COUNT`] connections open,
/// and at most [`MAX_CONNECTIONS`] are served at once in total. The address
/// limit is checked first. A connection over either limit is given a number
/// like any other, then closed at once and published as a shed event; the
/// loop goes straight on to the next `accept`.
///
/// Each connection is given a number, counting from 1, a sender for the
/// event channel and a sender for the viewer feed. A separate task
/// subscribes to the event channel and prints every event to standard
/// output as one line of JSON. Another runs the viewer feed (see
/// [`server::run_viewer_feed`]), which forwards events to WebSocket viewers
/// within a budget per window and summarises the rest.
///
/// A failed `accept` is logged and retried after 100 ms, so running out of
/// file descriptors does not spin the loop. Errors from a connection are
/// logged together with the client's address.
///
/// # Errors
///
/// Returns an error if the listener cannot bind, for example because the
/// port is already in use. Nothing after that returns an error: if the
/// connection semaphore were ever closed, which does not happen in normal
/// running, every connection would be shed for capacity instead.
///
/// # Panics
///
/// Panics before binding if a page template is invalid. See
/// [`site::validate_templates`].
#[tokio::main]
async fn main() -> io::Result<()> {
  // Validate all templates before starting server
  validate_templates();

  let connection_pool = Arc::new(Semaphore::new(MAX_CONNECTIONS));
  let viewer_pool = Arc::new(Semaphore::new(MAX_VIEWERS));
  let connection_ip_limit = Arc::new(IpLimit::new(MAX_IP_COUNT));
  let viewer_ip_limit = Arc::new(IpLimit::new(MAX_VIEWERS_PER_IP));
  let listener = TcpListener::bind("0.0.0.0:8080").await?;
  let (tx_events, mut rx_events) = broadcast::channel::<Event>(256);
  let rx_viewer_feed = tx_events.subscribe();
  let tx_viewer_feed = broadcast::Sender::<Arc<[u8]>>::new(64);
  let mut connection_count: u64 = 0;

  // Task explicitly for logging only
  tokio::spawn(async move {
    loop {
      let event = match rx_events.recv().await {
        Ok(v) => v,
        Err(RecvError::Lagged(skipped)) => {
          eprintln!("subscriber lagged, skipped {skipped} events");
          continue;
        }
        Err(RecvError::Closed) => break,
      };

      let event_s = match serde_json::to_string(&event) {
        Ok(v) => v,
        Err(e) => {
          eprintln!("skipping: {e}");
          continue;
        }
      };
      println!("{event_s}");
    }
  });

  // Task that limits and frames the events sent to viewers
  tokio::spawn(run_viewer_feed(rx_viewer_feed, tx_viewer_feed.clone()));

  loop {
    let (stream, peer) = match listener.accept().await {
      Ok(c) => c,
      Err(e) => {
        eprintln!("accept: {e}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        continue;
      }
    };

    connection_count = connection_count.wrapping_add(1);

    let Some(ip_permit) = connection_ip_limit.try_acquire(peer.ip()) else {
      server::shed(connection_count, peer, ShedReason::IpLimit, &tx_events);
      continue;
    };

    let Ok(permit) = connection_pool.clone().try_acquire_owned() else {
      server::shed(connection_count, peer, ShedReason::Capacity, &tx_events);
      continue;
    };

    let tx = tx_events.clone();
    let tx_viewer_feed = tx_viewer_feed.clone();
    let viewer_pool = viewer_pool.clone();
    let viewer_ip_limit = viewer_ip_limit.clone();

    tokio::spawn(async move {
      let Some(deadline) = Instant::now().checked_add(CONNECTION_TIMEOUT)
      else {
        return;
      };
      // TODO add TLS handshake

      let _permit = permit;
      let _ip_permit = ip_permit;
      if let Err(e) = server::handle(
        stream,
        peer,
        connection_count,
        deadline,
        tx,
        tx_viewer_feed,
        (viewer_pool, viewer_ip_limit),
      )
      .await
      {
        eprintln!("{peer}: {e}");
      }
    });
  }
}
