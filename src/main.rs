//! wire: a hand-written HTTP/1.1 server on tokio that shows live TCP
//! connection data (`TCP_INFO`) in the browser.

use std::io;

use tokio::net::TcpListener;

mod connection;

/// Binds to port 8080 on all interfaces and serves each accepted connection
/// in its own task.
///
/// A failed `accept` is logged and skipped, so the server keeps running.
/// Errors from a connection are logged together with the client's address.
///
/// # Errors
///
/// Returns an error only if the listener cannot bind, for example because
/// the port is already in use.
#[tokio::main]
async fn main() -> io::Result<()> {
  let listener = TcpListener::bind("0.0.0.0:8080").await?;

  loop {
    let (stream, peer) = match listener.accept().await {
      Ok(c) => c,
      Err(e) => {
        eprintln!("accept: {e}");
        continue;
      }
    };

    tokio::spawn(async move {
      if let Err(e) = connection::handle(stream, peer).await {
        eprintln!("{peer}: {e}");
      }
    });
  }
}
