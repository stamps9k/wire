//! wire: a hand-written HTTP/1.1 server on tokio that shows live TCP
//! connection data (`TCP_INFO`) in the browser.

use std::io;

use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::event::Event;
use crate::site::validate_templates;

mod event;
mod http;
mod server;
mod site;
mod websocket;

/// Checks the page templates, then binds to port 8080 on all interfaces and
/// serves each accepted connection in its own task.
///
/// Each connection is given a number, counting from 1, and a sender for the
/// event channel. A separate task subscribes to that channel and prints
/// every event to standard output as one line of JSON.
///
/// A failed `accept` is logged and skipped, so the server keeps running.
/// Errors from a connection are logged together with the client's address.
///
/// # Errors
///
/// Returns an error only if the listener cannot bind, for example because
/// the port is already in use.
///
/// # Panics
///
/// Panics before binding if a page template is invalid. See
/// [`site::validate_templates`].
#[tokio::main]
async fn main() -> io::Result<()> {
  // Validate all templates before starting server
  validate_templates();

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
    let (stream, peer) = match listener.accept().await {
      Ok(c) => c,
      Err(e) => {
        eprintln!("accept: {e}");
        continue;
      }
    };

    let tx = tx.clone();
    connection_count = connection_count.wrapping_add(1);
    tokio::spawn(async move {
      if let Err(e) = server::handle(stream, peer, tx, connection_count).await {
        eprintln!("{peer}: {e}");
      }
    });
  }
}
