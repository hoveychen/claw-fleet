//! Static file serving: the mobile web bundle at `/` and the desktop web
//! bundle at `/desktop/`.
//!
//! Layout of `RELAY_STATIC_DIR` (see fleet-relay/Dockerfile):
//!
//! ```text
//! static/
//!   index.html, assets/, sw.js, ...      mobile-web
//!   desktop/
//!     index.html, assets/, sw.js, ...    claw-fleet-desktop
//! ```
//!
//! Fallback rules differ per bundle:
//!
//! * Under `/` an unknown path answers mobile-web's `index.html` (the mobile
//!   PWA is a single page that also handles deep links such as pairing URLs).
//! * Under `/desktop/` an unknown path is a plain 404. The desktop UI has no
//!   path-based routing — it only ever lives at `/desktop/` (plus a query) —
//!   and answering a missing hash-named chunk with HTML would let the desktop
//!   service worker cache an HTML page as a script forever.
//! * `/desktop/h/*` is the tunnel prefix: the desktop page and its service
//!   worker carry those requests to the paired Mac over the relay socket, so
//!   one reaching the relay over the network means the worker was not in
//!   control (first load, hard refresh, a sandboxed iframe). It gets a plain
//!   503, never an HTML page that a `<img>` or `<iframe>` would render.

use std::path::Path;

use axum::handler::HandlerWithoutStateExt;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Mount point of the desktop web bundle. Must match the prefix the desktop
/// front end detects (`relayMountPath` in claw-fleet-desktop/app/main.tsx).
pub const DESKTOP_MOUNT: &str = "/desktop";

fn plain(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

async fn tunnel_miss() -> Response {
    plain(
        StatusCode::SERVICE_UNAVAILABLE,
        "desktop tunnel request reached the relay: the page's service worker is not in control\n",
    )
}

async fn desktop_not_found() -> Response {
    plain(StatusCode::NOT_FOUND, "not found\n")
}

/// Router for every static path. Merged after the API routes, so `/ws`,
/// `/healthz` and friends win over files of the same name.
pub fn router(static_dir: &Path) -> Router {
    // Rooted at static_dir (not static_dir/desktop) because the route keeps
    // the `/desktop/` prefix in the request path; the routes below make sure
    // only `/desktop/...` paths ever reach it.
    let desktop = ServeDir::new(static_dir).not_found_service(desktop_not_found.into_service());
    let mobile =
        ServeDir::new(static_dir).not_found_service(ServeFile::new(static_dir.join("index.html")));

    Router::new()
        // Without the trailing slash the bundle's relative URLs (`./assets/…`)
        // would resolve against `/` and land in mobile-web.
        .route(
            DESKTOP_MOUNT,
            get(|| async { Redirect::permanent("/desktop/") }),
        )
        .route("/desktop/h", any(tunnel_miss))
        .route("/desktop/h/", any(tunnel_miss))
        .route("/desktop/h/{*rest}", any(tunnel_miss))
        .route_service("/desktop/", desktop.clone())
        .route_service("/desktop/{*rest}", desktop)
        .fallback_service(mobile)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        base: String,
    }

    async fn serve() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("index.html"), "<html>mobile</html>").unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/m.js"), "mobile()").unwrap();
        std::fs::create_dir_all(root.join("desktop/assets")).unwrap();
        std::fs::write(root.join("desktop/index.html"), "<html>desktop</html>").unwrap();
        std::fs::write(root.join("desktop/assets/d.js"), "desktop()").unwrap();
        std::fs::write(
            root.join("desktop/manifest.webmanifest"),
            r#"{"start_url":"./"}"#,
        )
        .unwrap();

        let app = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .merge(router(root));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fixture {
            _dir: dir,
            base: format!("http://{addr}"),
        }
    }

    async fn get_(base: &str, path: &str) -> (u16, String, String) {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        let status = resp.status().as_u16();
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_string());
        let body = resp.text().await.unwrap();
        (status, ct, loc.unwrap_or(body))
    }

    #[tokio::test]
    async fn mobile_bundle_and_spa_fallback_at_root() {
        let f = serve().await;
        let (s, _, b) = get_(&f.base, "/").await;
        assert_eq!((s, b.as_str()), (200, "<html>mobile</html>"));
        let (s, _, b) = get_(&f.base, "/assets/m.js").await;
        assert_eq!((s, b.as_str()), (200, "mobile()"));
        // ServeDir's not_found_service keeps the 404 status; the body is
        // still mobile-web's index.html (pre-existing relay behaviour).
        let (_, ct, b) = get_(&f.base, "/pair/abc").await;
        assert_eq!(b, "<html>mobile</html>");
        assert!(ct.starts_with("text/html"), "{ct}");
    }

    #[tokio::test]
    async fn api_routes_win_over_static() {
        let f = serve().await;
        let (s, _, b) = get_(&f.base, "/healthz").await;
        assert_eq!((s, b.as_str()), (200, "ok"));
    }

    #[tokio::test]
    async fn desktop_bundle_served_under_mount() {
        let f = serve().await;
        let (s, ct, b) = get_(&f.base, "/desktop/").await;
        assert_eq!((s, b.as_str()), (200, "<html>desktop</html>"));
        assert!(ct.starts_with("text/html"), "{ct}");
        let (s, _, b) = get_(&f.base, "/desktop/assets/d.js").await;
        assert_eq!((s, b.as_str()), (200, "desktop()"));
        let (s, _, _) = get_(&f.base, "/desktop/manifest.webmanifest").await;
        assert_eq!(s, 200);
    }

    #[tokio::test]
    async fn bare_mount_redirects_to_trailing_slash() {
        let f = serve().await;
        let (s, _, loc) = get_(&f.base, "/desktop").await;
        assert_eq!((s, loc.as_str()), (308, "/desktop/"));
    }

    #[tokio::test]
    async fn desktop_unknown_path_is_404_not_mobile_html() {
        let f = serve().await;
        for path in ["/desktop/some/route", "/desktop/assets/gone-abc123.js"] {
            let (s, ct, b) = get_(&f.base, path).await;
            assert_eq!(s, 404, "{path}");
            assert!(ct.starts_with("text/plain"), "{path}: {ct}");
            assert!(!b.contains("<html>"), "{path}: {b}");
        }
    }

    #[tokio::test]
    async fn tunnel_prefix_is_plain_503() {
        let f = serve().await;
        for path in [
            "/desktop/h",
            "/desktop/h/",
            "/desktop/h/sessions",
            "/desktop/h/decision_asset/a/b.png",
        ] {
            let (s, ct, b) = get_(&f.base, path).await;
            assert_eq!(s, 503, "{path}");
            assert!(ct.starts_with("text/plain"), "{path}: {ct}");
            assert!(!b.contains("<html>"), "{path}: {b}");
        }
    }
}
