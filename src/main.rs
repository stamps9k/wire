//! wire: a hand-written HTTP/1.1 server on tokio that shows live TCP
//! connection data (`TCP_INFO`) in the browser.

use std::io;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Duration;

use crate::event::Event;
use crate::site::validate_templates;

mod event;
mod http;
mod server;
mod site;
mod websocket;

/// The most connections served at once.
///
/// Each connection holds a permit from a semaphore of this size for as long
/// as its task runs. When none are free, the accept loop waits, and new
/// clients queue in the kernel's listen backlog until a permit is released.
///
/// WebSocket viewers stay connected and count against this limit like any
/// other connection, so enough of them can block ordinary requests. Keep
/// this well below the process's open file limit (`ulimit -n`).
const MAX_CONNECTIONS: usize = 512;

/// Checks the page templates, then binds to port 8080 on all interfaces and
/// serves each accepted connection in its own task.
///
/// At most [`MAX_CONNECTIONS`] connections are served at once. When that many
/// are open, accepting pauses until one finishes.
///
/// Each connection is given a number, counting from 1, and a sender for the
/// event channel. A separate task subscribes to that channel and prints
/// every event to standard output as one line of JSON.
///
/// A failed `accept` is logged and retried after 100 ms, so running out of
/// file descriptors does not spin the loop. Errors from a connection are
/// logged together with the client's address.
///
/// # Errors
///
/// Returns an error if the listener cannot bind, for example because the
/// port is already in use, or if the connection semaphore is closed, which
/// does not happen in normal running.
///
/// # Panics
///
/// Panics before binding if a page template is invalid. See
/// [`site::validate_templates`].
#[tokio::main]
async fn main() -> io::Result<()> {
  // Validate all templates before starting server
  validate_templates();

  let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
  let listener = TcpListener::bind("0.0.0.0:8080").await?;
  let (tx, mut rx) = broadcast::channel::<Event>(256);
  let mut connection_count: u64 = 0;

  // Task explicitly for logging only
  tokio::spawn(async move {
    loop {
      let event = match rx.recv().await {
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

  loop {
    let permit = match limit.clone().acquire_owned().await {
      Ok(c) => c,
      Err(e) => return Err(io::Error::other(e)),
    };

    let (stream, peer) = match listener.accept().await {
      Ok(c) => c,
      Err(e) => {
        eprintln!("accept: {e}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        continue;
      }
    };

    let tx = tx.clone();
    connection_count = connection_count.wrapping_add(1);
    tokio::spawn(async move {
      let _permit = permit;
      if let Err(e) = server::handle(stream, peer, tx, connection_count).await {
        eprintln!("{peer}: {e}");
      }
    });
  }
}
