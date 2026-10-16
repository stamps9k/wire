//! The error returned by [`render`](super::html::render).

/// Why a template could not be rendered.
///
/// Templates are fixed at compile time, so either error means the template
/// or the code that calls `render` has a mistake. It never means a request
/// was bad.
#[derive(Debug, PartialEq, Eq)]
pub enum RenderError {
  /// A placeholder named something that has no value. Holds the name.
  UnknownName(String),
  /// A `{{` has no `}}` before the end of the template or the next `{{`.
  Unclosed,
}
