//! Routing: decides the response for a valid request, and builds error
//! responses with the error page as their body.

use crate::http::Header;
use crate::http::Method;
use crate::http::Request;
use crate::http::Response;
use crate::http::Status;
use crate::http::Target;
use crate::site;
use crate::websocket;

/// The origins whose pages may open the event stream at `/ws`.
///
/// A browser sends the origin of the page that opens a WebSocket in the
/// `Origin` header and does not let the page change it, so this list decides
/// which websites can use their visitors' browsers as viewers. Without it,
/// any site could embed a hidden connection, and each visitor would arrive
/// from a different address, getting past the per-address viewer limit.
/// Clients other than browsers can send any `Origin`, so this does not stop
/// scripts; the viewer limits and lifetime constrain those.
///
/// Each entry must match the header exactly, as scheme, host and any
/// non-default port, with no trailing slash. The entries are the debug page
/// on wire over HTTP and, once wire supports TLS, over HTTPS; the particle
/// page; and the debug page served by a local development container.
const ALLOWED_ORIGINS: &[&str] = &[
  "http://wire.stampatron.com",
  "https://wire.stampatron.com",
  "https://webgpu-particles.stampatron.com",
  "http://localhost:8080",
];

/// Chooses the response for `request`.
///
/// The method is checked first, then the path. Any method other than `GET`
/// gets 405. A `GET` for `/` gets the index page and a `GET` for `/debug`
/// the debug page, with or without a query. A `GET` for `/ws` with no
/// `Origin` header, or one that does not exactly match an entry in
/// [`ALLOWED_ORIGINS`], gets 403 Forbidden; any other is answered by
/// [`websocket::upgrade`]. Everything else gets 404, including targets that
/// are not in origin form.
///
/// The request must already have passed [`Request::validate`]. This function
/// reads nothing from the connection and cannot fail.
pub fn route(request: &Request) -> Response {
  if !matches!(request.method, Method::Get) {
    let mut response = error_response(Status::MethodNotAllowed);
    response.headers.push(Header {
      name: "Allow".to_string(),
      value: b"GET".to_vec(),
    });
    return response;
  }

  let Target::Origin(target) = &request.target else {
    return error_response(Status::NotFound);
  };

  let resource = target.split('?').next();

  match resource {
    Some("/") => Response::ok(site::index(request)),
    Some("/debug") => Response::ok(site::debug()),
    Some("/ws") => {
      let Some(origin) = request.header("origin") else {
        return error_response(Status::Forbidden);
      };

      if ALLOWED_ORIGINS.iter().any(|s| s.as_bytes() == origin.value) {
        return websocket::upgrade(request).unwrap_or_else(error_response);
      }

      error_response(Status::Forbidden)
    }
    Some(_) | None => error_response(Status::NotFound),
  }
}

/// Builds the response for `status` with the error page as its body.
pub fn error_response(status: Status) -> Response {
  Response::new(status, site::error(status))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::http::Version;

  fn request(method: Method, target: Target) -> Request {
    Request {
      method,
      target,
      version: Version { major: 1, minor: 1 },
      headers: Vec::new(),
    }
  }

  fn origin(path: &str) -> Target {
    Target::Origin(path.to_owned())
  }

  fn code(method: Method, target: Target) -> u16 {
    route(&request(method, target)).status.code()
  }

  fn header(name: &str, value: &[u8]) -> Header {
    Header {
      name: name.to_owned(),
      value: value.to_vec(),
    }
  }

  /// A `GET` for `target` with an `Origin` header holding `value`.
  fn from_origin(target: Target, value: &[u8]) -> Request {
    let mut request = request(Method::Get, target);
    request.headers.push(header("origin", value));
    request
  }

  /// A correct WebSocket upgrade request for `/ws`, with an `Origin` header
  /// holding `value` if one is given.
  fn upgrade_request(value: Option<&[u8]>) -> Request {
    let mut request = request(Method::Get, origin("/ws"));
    request.headers = vec![
      header("host", b"wire.stampatron.com"),
      header("upgrade", b"websocket"),
      header("connection", b"Upgrade"),
      header("sec-websocket-key", b"dGhlIHNhbXBsZSBub25jZQ=="),
      header("sec-websocket-version", b"13"),
    ];
    if let Some(value) = value {
      request.headers.push(header("origin", value));
    }
    request
  }

  fn upgrade_code(value: Option<&[u8]>) -> u16 {
    route(&upgrade_request(value)).status.code()
  }

  #[test]
  fn get_root_is_ok() {
    assert_eq!(code(Method::Get, origin("/")), 200);
  }

  #[test]
  fn get_root_ignores_the_query() {
    assert_eq!(code(Method::Get, origin("/?a=b")), 200);
    assert_eq!(code(Method::Get, origin("/?")), 200);
    assert_eq!(code(Method::Get, origin("/?a=b?c=d")), 200);
  }

  #[test]
  fn get_root_serves_the_index_page() {
    let response = route(&request(Method::Get, origin("/")));
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.contains("<p>Method: GET</p>"));
  }

  #[test]
  fn get_unknown_path_is_not_found() {
    assert_eq!(code(Method::Get, origin("/nope")), 404);
    assert_eq!(code(Method::Get, origin("/index.html")), 404);
    assert_eq!(code(Method::Get, origin("//")), 404);
  }

  #[test]
  fn query_does_not_rescue_an_unknown_path() {
    assert_eq!(code(Method::Get, origin("/nope?/")), 404);
  }

  #[test]
  fn get_absolute_form_is_not_found() {
    let target = Target::Absolute("http://example.com/".to_owned());
    assert_eq!(code(Method::Get, target), 404);
  }

  #[test]
  fn other_methods_are_not_allowed() {
    assert_eq!(code(Method::Post, origin("/")), 405);
    assert_eq!(code(Method::Head, origin("/")), 405);
    assert_eq!(code(Method::Other("PURGE".to_owned()), origin("/")), 405);
  }

  #[test]
  fn method_is_checked_before_the_path() {
    assert_eq!(code(Method::Post, origin("/nope")), 405);
  }

  #[test]
  fn connect_and_options_asterisk_are_not_allowed() {
    let authority = Target::Authority("example.com:443".to_owned());
    assert_eq!(code(Method::Connect, authority), 405);
    assert_eq!(code(Method::Options, Target::Asterisk), 405);
  }

  #[test]
  fn error_pages_name_their_status() {
    let response = route(&request(Method::Get, origin("/nope")));
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.contains("<p>Error Code: 404</p>"));
  }

  #[test]
  fn not_allowed_lists_get_in_allow() {
    let response = route(&request(Method::Post, origin("/")));
    assert_eq!(response.headers.len(), 1);
    let header = response.headers.first();
    assert!(header.is_some_and(|h| h.name == "Allow" && h.value == b"GET"));
  }

  #[test]
  fn other_responses_have_no_extra_headers() {
    let found = route(&request(Method::Get, origin("/")));
    let missing = route(&request(Method::Get, origin("/nope")));
    assert!(found.headers.is_empty());
    assert!(missing.headers.is_empty());
  }

  #[test]
  fn get_ws_is_handed_to_the_websocket_handshake() {
    // An allowed origin but no upgrade headers, so the handshake refuses it.
    // A 404 here would mean the path was never routed.
    let site = b"http://wire.stampatron.com";
    let status = |target| route(&from_origin(target, site)).status.code();
    assert_eq!(status(origin("/ws")), 400);
    assert_eq!(status(origin("/ws?a=b")), 400);
    assert_eq!(code(Method::Post, origin("/ws")), 405);
  }

  #[test]
  fn every_allowed_origin_can_upgrade() {
    for allowed in ALLOWED_ORIGINS {
      assert_eq!(upgrade_code(Some(allowed.as_bytes())), 101, "{allowed}");
    }
  }

  #[test]
  fn an_upgrade_without_an_origin_is_forbidden() {
    assert_eq!(upgrade_code(None), 403);
  }

  #[test]
  fn an_upgrade_from_another_site_is_forbidden() {
    assert_eq!(upgrade_code(Some(b"https://example.com")), 403);
    assert_eq!(upgrade_code(Some(b"https://stampatron.com")), 403);
    assert_eq!(upgrade_code(Some(b"null")), 403);
    assert_eq!(upgrade_code(Some(b"")), 403);
  }

  #[test]
  fn the_origin_must_match_exactly() {
    let near_misses: [&[u8]; 6] = [
      b"http://webgpu-particles.stampatron.com",
      b"https://webgpu-particles.stampatron.com/",
      b"https://webgpu-particles.stampatron.com:8443",
      b"http://localhost:8081",
      b"https://wire.stampatron.com.example.net",
      b"HTTPS://WIRE.STAMPATRON.COM",
    ];
    for value in near_misses {
      assert_eq!(upgrade_code(Some(value)), 403);
    }
  }

  #[test]
  fn an_origin_that_is_not_utf8_is_forbidden() {
    assert_eq!(upgrade_code(Some(b"https://wire.\xffstampatron.com")), 403);
  }

  #[test]
  fn the_origin_is_checked_before_the_handshake() {
    // No upgrade headers either, but the missing origin is what is refused.
    assert_eq!(code(Method::Get, origin("/ws")), 403);
  }

  #[test]
  fn other_paths_do_not_need_an_origin() {
    let site = b"https://example.com";
    assert_eq!(route(&from_origin(origin("/"), site)).status.code(), 200);
    assert_eq!(code(Method::Get, origin("/debug")), 200);
  }

  #[test]
  fn get_debug_serves_the_debug_page() {
    let response = route(&request(Method::Get, origin("/debug")));
    assert_eq!(response.status.code(), 200);
    assert!(response.headers.is_empty());
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.contains("<title>wire debug</title>"));
  }

  #[test]
  fn get_debug_ignores_the_query() {
    assert_eq!(code(Method::Get, origin("/debug?a=b")), 200);
    assert_eq!(code(Method::Get, origin("/debug?")), 200);
  }

  #[test]
  fn debug_path_must_match_exactly() {
    assert_eq!(code(Method::Get, origin("/debug/")), 404);
    assert_eq!(code(Method::Get, origin("/debugx")), 404);
    assert_eq!(code(Method::Get, origin("/Debug")), 404);
  }

  #[test]
  fn other_methods_on_debug_are_not_allowed() {
    assert_eq!(code(Method::Post, origin("/debug")), 405);
  }
}
