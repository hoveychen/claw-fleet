//! Universal Link (iOS) / App Link (Android) association files.
//!
//! The native shell boots from bundled assets, so it never sees the pairing URL
//! the desktop QR encodes (`https://<relay>/#k=<secret>`). Associating this
//! domain with the app makes the OS hand that scanned URL to the app instead of
//! a browser, which is the shell's only route to a pairing secret. The secret
//! itself stays in the fragment and is never sent here — the relay remains a
//! blind forwarder.
//!
//! Two reasons these are routes rather than files under `RELAY_STATIC_DIR`:
//!
//!   * The static service falls back to `index.html` for unknown paths (SPA
//!     routing), so a *missing* association file would be served as HTML with
//!     200 OK. Apple's CDN and Android's verifier would both consume that
//!     garbage instead of failing cleanly. A route can honestly 404.
//!   * `apple-app-site-association` has no file extension, so `ServeDir`'s
//!     mime guess yields `application/octet-stream`; Apple requires
//!     `application/json`.
//!
//! Both files are rendered from env vars and are absent (404) unless configured,
//! so an unconfigured deployment advertises no association at all rather than a
//! broken one.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

/// Rendered association documents, or `None` when not configured.
#[derive(Clone, Default)]
pub struct AppLinks {
    aasa: Option<String>,
    assetlinks: Option<String>,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl AppLinks {
    /// * `RELAY_IOS_APP_ID` — `<TeamID>.<bundleId>`, e.g. `ABCDE12345.com.hoveychen.clawfleet`.
    /// * `RELAY_ANDROID_PACKAGE` + `RELAY_ANDROID_SHA256` — both required for
    ///   the Android side; the fingerprint is the colon-separated uppercase hex
    ///   of the *signing* certificate (`keytool -list -v`), not the debug one.
    pub fn from_env() -> Self {
        Self {
            aasa: env_nonempty("RELAY_IOS_APP_ID").map(|app_id| render_aasa(&app_id)),
            assetlinks: match (
                env_nonempty("RELAY_ANDROID_PACKAGE"),
                env_nonempty("RELAY_ANDROID_SHA256"),
            ) {
                (Some(pkg), Some(sha)) => Some(render_assetlinks(&pkg, &sha)),
                _ => None,
            },
        }
    }

    pub fn ios_configured(&self) -> bool {
        self.aasa.is_some()
    }

    pub fn android_configured(&self) -> bool {
        self.assetlinks.is_some()
    }

    pub fn aasa_response(&self) -> Response {
        json_or_404(self.aasa.as_deref())
    }

    pub fn assetlinks_response(&self) -> Response {
        json_or_404(self.assetlinks.as_deref())
    }
}

fn json_or_404(body: Option<&str>) -> Response {
    match body {
        Some(json) => {
            ([(header::CONTENT_TYPE, "application/json")], json.to_owned()).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `components` (iOS 13+) matches every path; `apps: []` is the legacy field
/// Apple still expects to be present and empty.
fn render_aasa(app_id: &str) -> String {
    serde_json::json!({
        "applinks": {
            "apps": [],
            "details": [{
                "appIDs": [app_id],
                "components": [{ "/": "*" }]
            }]
        }
    })
    .to_string()
}

fn render_assetlinks(package: &str, sha256: &str) -> String {
    serde_json::json!([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {
            "namespace": "android_app",
            "package_name": package,
            "sha256_cert_fingerprints": [sha256]
        }
    }])
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn aasa_carries_the_app_id_and_matches_every_path() {
        let v: Value = serde_json::from_str(&render_aasa("ABCDE12345.com.hoveychen.clawfleet"))
            .expect("valid json");
        let detail = &v["applinks"]["details"][0];
        assert_eq!(detail["appIDs"][0], "ABCDE12345.com.hoveychen.clawfleet");
        assert_eq!(detail["components"][0]["/"], "*");
        // Apple still wants the legacy key present and empty.
        assert_eq!(v["applinks"]["apps"], serde_json::json!([]));
    }

    #[test]
    fn assetlinks_is_a_list_with_the_url_handling_relation() {
        let v: Value = serde_json::from_str(&render_assetlinks(
            "com.hoveychen.clawfleet",
            "71:B9:7B:E8",
        ))
        .expect("valid json");
        assert!(v.is_array(), "assetlinks.json must be a JSON array");
        assert_eq!(v[0]["relation"][0], "delegate_permission/common.handle_all_urls");
        assert_eq!(v[0]["target"]["namespace"], "android_app");
        assert_eq!(v[0]["target"]["package_name"], "com.hoveychen.clawfleet");
        assert_eq!(v[0]["target"]["sha256_cert_fingerprints"][0], "71:B9:7B:E8");
    }

    /// An unconfigured deployment must 404 rather than serve a half-built
    /// document that a verifier would cache as authoritative.
    #[test]
    fn unconfigured_links_404_instead_of_serving_junk() {
        let links = AppLinks::default();
        assert!(!links.ios_configured());
        assert!(!links.android_configured());
        assert_eq!(links.aasa_response().status(), StatusCode::NOT_FOUND);
        assert_eq!(links.assetlinks_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn configured_links_are_served_as_application_json() {
        let links = AppLinks {
            aasa: Some(render_aasa("T.app")),
            assetlinks: Some(render_assetlinks("p", "AA")),
        };
        for resp in [links.aasa_response(), links.assetlinks_response()] {
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(
                resp.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/json",
                "Apple rejects an AASA that is not application/json"
            );
        }
    }
}
