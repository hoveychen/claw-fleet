//! Cross-origin access: only enabled for the two mobile-direct paths, and
//! only when a token gate is active.
//!
//! Why it's needed: the page's origin on the phone is a relay domain (or a
//! fake origin in a native shell). Querying a `fleet serve` host from there
//! is a cross-origin request. Without `Access-Control-Allow-Origin`, the browser
//! blocks it before the page sees the response — the backend returns 200 but
//! the frontend only sees a network error.
//!
//! Why only two paths: the mobile data plane has `POST /mobile_rpc` and
//! `GET /events` only. The other hundreds of routes (proc exec, settings,
//! credentials, file browser) have no cross-origin users; enabling CORS for
//! them just expands the attack surface unnecessarily.
//!
//! Why gate it on a token: the `fleet webui` port **has no auth of its own**
//! (the startup log says so: "this port launches agent sessions; you must put
//! a gateway in front"). Returning `Allow-Origin: *` on an unauth port means
//! **any webpage in the user's browser** can drive their Fleet. So: CORS headers
//! go out only when auth is on (admin/scoped tokens are checked); deployments
//! with auth off must handle cross-origin at their own gateway — which is
//! already their responsibility anyway.
//!
//! We use `*` instead of echoing the request Origin because **no cookies are
//! in play here**: credentials go in `Authorization: Bearer` (or `?token=` for
//! SSE), and browsers don't send cookies with responses carrying `*`. So `*`
//! here poses no "browser identity spoofing" risk, while echoing Origin would
//! require maintaining an allowlist.

use tiny_http::Header;

/// Paths that allow cross-origin access — the entire mobile data plane.
const CORS_PATHS: [&str; 2] = [crate::routes::MOBILE_RPC, "/events"];

pub fn is_cors_path(path: &str) -> bool {
    CORS_PATHS.contains(&path)
}

/// Whether this deployment enables cross-origin access externally. When
/// `auth_disabled` is true (there is a gateway in front), it stays disabled —
/// see the module header for the reasoning.
pub fn cors_enabled(auth_disabled: bool) -> bool {
    !auth_disabled
}

/// The CORS headers to add to the response. Empty when disabled — callers still
/// iterate with `with_header` as usual, no branching needed.
pub fn headers(auth_disabled: bool, path: &str) -> Vec<Header> {
    if !cors_enabled(auth_disabled) || !is_cors_path(path) {
        return Vec::new();
    }
    header_set()
}

/// The set of headers to return for a preflight. Both `Authorization` and
/// `Content-Type` must be in the allow list: the former is the token, the
/// latter is `application/json` (which makes POST a "non-simple request", so
/// the browser issues OPTIONS first).
fn header_set() -> Vec<Header> {
    [
        "Access-Control-Allow-Origin: *",
        "Access-Control-Allow-Methods: GET, POST, OPTIONS",
        "Access-Control-Allow-Headers: authorization, content-type",
        // Preflight results cached for 10 minutes: every mobile request adds an
        // extra round trip, real latency on a mobile link.
        "Access-Control-Max-Age: 600",
    ]
    .iter()
    .map(|h| h.parse::<Header>().expect("static CORS header parses"))
    .collect()
}

/// Is this a preflight request we should answer directly?
///
/// **Preflight MUST be answered before auth**: when the browser sends OPTIONS,
/// it does NOT include the `Authorization` header (that's what it's asking
/// about). If we put this after auth, the preflight gets 401, and the real
/// request never goes out — but the symptom just looks like "cross-origin
/// request failed", hiding that preflight died at the gate.
pub fn is_preflight(method: &tiny_http::Method, path: &str, auth_disabled: bool) -> bool {
    cors_enabled(auth_disabled) && method == &tiny_http::Method::Options && is_cors_path(path)
}

/// Return a 204 preflight response.
pub fn preflight_response() -> tiny_http::Response<std::io::Empty> {
    let mut res = tiny_http::Response::empty(204);
    for h in header_set() {
        res.add_header(h);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_the_two_mobile_paths() {
        assert!(is_cors_path("/mobile_rpc"));
        assert!(is_cors_path("/events"));
        // Other routes have no cross-origin users — enabling CORS just expands
        // the attack surface.
        assert!(!is_cors_path("/settings"));
        assert!(!is_cors_path("/proc/exec"));
        assert!(!is_cors_path("/v1/sessions"));
        assert!(!is_cors_path("/"));
    }

    /// Port with no auth does not send CORS headers: otherwise any webpage can
    /// drive this Fleet.
    #[test]
    fn stays_shut_when_auth_is_disabled() {
        assert!(!cors_enabled(true));
        assert!(headers(true, "/mobile_rpc").is_empty());
        assert!(!is_preflight(&tiny_http::Method::Options, "/mobile_rpc", true));
    }

    #[test]
    fn opens_when_a_token_gate_is_active() {
        assert!(cors_enabled(false));
        let hs = headers(false, "/mobile_rpc");
        assert!(!hs.is_empty());
        let rendered: Vec<String> = hs
            .iter()
            .map(|h| format!("{}: {}", h.field.as_str().as_str(), h.value.as_str()))
            .collect();
        assert!(rendered.iter().any(|h| h == "Access-Control-Allow-Origin: *"));
        // Token and JSON content-type must be in the allow list, or preflight
        // will reject the request at the gate.
        assert!(rendered
            .iter()
            .any(|h| h.to_ascii_lowercase().contains("authorization")
                && h.to_ascii_lowercase().contains("content-type")));
    }

    #[test]
    fn non_cors_paths_get_no_headers_even_with_auth_on() {
        assert!(headers(false, "/settings").is_empty());
    }

    #[test]
    fn preflight_is_only_options_on_a_cors_path() {
        assert!(is_preflight(&tiny_http::Method::Options, "/events", false));
        assert!(!is_preflight(&tiny_http::Method::Post, "/mobile_rpc", false));
        assert!(!is_preflight(&tiny_http::Method::Options, "/settings", false));
    }

    #[test]
    fn preflight_response_carries_the_headers() {
        let res = preflight_response();
        assert_eq!(res.status_code().0, 204);
        assert!(res
            .headers()
            .iter()
            .any(|h| h.field.equiv("access-control-allow-origin")));
    }
}
