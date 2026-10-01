//! Per-connection handling: reads the request head and writes the response.

use crate::io::Result;

use std::io::Error;
use std::net::SocketAddr;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio::time::timeout;

use super::method::Method;
use super::request::Request;
use super::target::Target;

/// Serves one client connection from start to finish.
///
/// Reads the request head with a 10-second deadline, logs it, and sends the
/// response. The connection is closed when this function returns, whichever
/// path it takes.
///
/// A client that disconnects early, sends an over-long head, or times out is
/// dropped without a response, and that is not treated as an error.
///
/// # Errors
///
/// Returns an error if reading from or writing to the socket fails, for
/// example because the client reset the connection.
pub async fn handle(mut stream: TcpStream, addr: SocketAddr) -> Result<()> {
  println!("new client: {addr:?}");

  let header: Vec<u8> =
    match timeout(Duration::from_secs(10), read_head(&mut stream)).await {
      Ok(Ok(Some(h))) => h, // got a complete head in time
      Ok(Ok(None)) => {
        println!("{addr}: closed before head complete");
        return Ok(());
      }
      Ok(Err(e)) => return Err(e), // I/O error
      Err(_) => {
        println!("{addr}: header timeout");
        return Ok(()); // timed out
      }
    };

  println!("Header is {:?}", String::from_utf8_lossy(&header));

  let request = Request::parse(&String::from_utf8_lossy(&header))?;

  //TMP log to suppress unused error
  match request.target {
    Target::Origin(path) => println!("path is {path}"),
    Target::Absolute(uri) => println!("absolute: {uri}"),
    Target::Authority(host_port) => println!("connect to {host_port}"),
    Target::Asterisk => println!("server-wide"),
  }
  if let Method::Other(s) = request.method {
    println!("Non standard method: {s:?}");
  }

  let major: u8 = request.version.major;
  let minor: u8 = request.version.minor;
  println!("Major - {major :?}");
  println!("Minor - {minor :?}");

  // Send response
  handle_request(&mut stream, header).await?;

  Ok(())
}

/// Writes the HTTP response for a request to `stream`.
///
/// Currently always sends a fixed `200 OK` HTML page with `Connection: close`,
/// whatever the request says. `_header` is the raw request head, which the
/// parser and router will use later.
///
/// # Errors
///
/// Returns an error if writing to the socket fails.
async fn handle_request(
  stream: &mut TcpStream,
  _header: Vec<u8>,
) -> Result<()> {
  let body = "<!doctype html><title>wire</title><p>hello</p>";
  let head = format!(
    "HTTP/1.1 200 OK\r\n\
     Content-Type: text/html; charset=utf-8\r\n\
     Content-Length: {}\r\n\
     Connection: close\r\n\
     \r\n",
    body.len()
  );
  stream.write_all(head.as_bytes()).await?;
  stream.write_all(body.as_bytes()).await?;

  Ok(())
}

/// Reads the HTTP request head from `stream` until the blank line that ends it.
///
/// Reads in 1 KiB chunks and accumulates them in a buffer until `\r\n\r\n`
/// is found. The returned buffer may contain bytes past the terminator.
///
/// Returns `Ok(None)` if the client closes the connection before the head is
/// complete, or if the head exceeds 8 KiB.
///
/// # Errors
///
/// Returns an error if reading from the socket fails, for example on a
/// connection reset.
async fn read_head(stream: &mut TcpStream) -> Result<Option<Vec<u8>>> {
  let mut data: Vec<u8> = Vec::new();

  let mut chunk = [0u8; 1024];
  loop {
    let n = stream.read(&mut chunk).await?;
    if n == 0 {
      return Ok(None); // client closed before finishing the head
    }

    let Some(bytes) = chunk.get(..n) else {
      return Err(Error::other("read returned more bytes than the buffer"));
    };
    data.extend_from_slice(bytes);

    if data.len() > 8 * 1024 {
      return Ok(None);
    }

    if let Some(_pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
      break;
    }
  }

  Ok(Some(data))
}
