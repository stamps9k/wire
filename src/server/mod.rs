//! Connection handling on tokio: reading, serving, publishing events and
//! streaming them to WebSocket viewers.

mod connection;
mod viewer;

pub use connection::handle;
