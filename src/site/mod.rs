//! What the server serves: routing and page bodies.

mod pages;
mod router;

pub use pages::{debug, error, index};
pub use router::error_response;
pub use router::route;
