//! WebSocket protocol (RFC 6455): the opening handshake and frames.
//! Pure protocol: no tokio, no events.

mod frame;
mod handshake;

pub use frame::{Frame, FrameBuffer, FrameError, OpCode};
pub use handshake::upgrade;
