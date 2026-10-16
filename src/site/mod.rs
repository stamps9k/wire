//! What the server serves: routing and page bodies.

mod html;
mod pages;
mod render_error;
mod router;

pub use pages::{debug, error, index, validate_templates};
pub use router::error_response;
pub use router::route;
