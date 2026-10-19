//! Connection handling on tokio: limiting connections per client address,
//! reading, serving, publishing events and streaming them to WebSocket
//! viewers.

mod connection;
mod ip_limit;
mod viewer;

pub use connection::handle;
pub use connection::shed;
pub use ip_limit::IpLimit;
