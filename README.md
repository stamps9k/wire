# wire

A web server written from scratch in Rust that shows live TCP connection data
in the browser, including the scanners, probes and junk traffic that reach any
public IP address.

Most web servers hide the transport layer behind HTTP. wire puts it on display:
for every connection it receives, it reads the kernel's own view of that
connection (round-trip time, congestion window, retransmissions, state
changes) and streams it to every connected browser, whether or not the client
ever sends valid HTTP.

## How it works

- **HTTP/1.1 from scratch.** Requests are parsed directly from raw
  `tokio::net::TcpStream` bytes, with no HTTP framework, and with strict limits
  on header size and read time so hostile clients can't use up resources.
- **`TCP_INFO` per connection.** `getsockopt(TCP_INFO)` samples the kernel's
  TCP statistics for each accepted socket.
- **Shared live feed.** A hand-written WebSocket (RFC 6455) endpoint
  broadcasts the same data to every client, so all viewers see the same
  traffic.
- **eBPF connection tracing.** An [Aya](https://aya-rs.dev) program on the
  `sock:inet_sock_set_state` tracepoint records every TCP state change on the
  host, including connections that never send a byte.
- **Exposed directly.** In production, wire runs on its own public IP with no
  CDN or reverse proxy in front of it, so it sees traffic as it arrives.

## Status

Early development.

- [ ] HTTP/1.1 server with per-connection `TCP_INFO` *(in progress)*
- [ ] WebSocket live feed shared between clients
- [ ] eBPF tracing of all TCP state changes
- [ ] Production container deployment

## Requirements

- Linux: `TCP_INFO` and eBPF are Linux-only. On macOS, develop inside a Linux
  container (see below).
- Rust stable, edition 2024.
- For the eBPF stage (not yet implemented): nightly Rust with `rust-src`,
  `bpf-linker`, and a kernel with BTF enabled.

## Building and running

```sh
cargo run
```

The server listens on `0.0.0.0:8080`. To test it:

```sh
curl -v http://localhost:8080/
```

or open <http://localhost:8080> in a browser.

### Development container

Development happens in a privileged Docker container (based on
`rust:1-trixie`) that has the eBPF toolchain installed and mounts `tracefs`,
`debugfs` and `bpffs`. The source is mounted into the container, and port
`8080` is published on `127.0.0.1`.

```sh
docker compose up -d --build
docker compose exec dev bash
```

## Development

All commits must pass:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

These run automatically as a pre-commit hook, which
[cargo-husky](https://github.com/rhysd/cargo-husky) installs from
`.cargo-husky/hooks/` the first time you run `cargo test`.

The lint configuration in `Cargo.toml` is deliberately strict. Because wire
parses untrusted input from the open internet, code that can panic (`unwrap`,
`panic!`, unchecked indexing and slicing) is denied, arithmetic that can
overflow is flagged, and every `unsafe` block must have a documented safety
justification.

## License

GNU General Public License v3.0. See [LICENSE](LICENSE).
