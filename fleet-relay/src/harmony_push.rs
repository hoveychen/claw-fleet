//! Huawei Push Kit delivery channel for HarmonyOS clients.
//!
//! HarmonyOS NEXT's built-in browser (ArkWeb) has no Web Push backend, so a
//! harmony client registers a Push Kit token instead of a browser
//! PushSubscription. This module turns the same `{title,body,tag,url}` payload
//! (see [`PushPayload`]) into a Push Kit REST call, authenticated with an
//! OAuth2 client-credentials access token that is cached and refreshed on
//! expiry.
//!
//! `from_env()` returns `None` unless `RELAY_HARMONY_CLIENT_ID`,
//! `RELAY_HARMONY_CLIENT_SECRET` and `RELAY_HARMONY_PROJECT_ID` are all set —
//! a missing config degrades to a no-op so existing Web Push deployments are
//! unaffected. The credentials come from AppGallery Connect; see the setup
//! handbook (wiki: mobile/harmony-push-kit-setup) for how to obtain them.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::frames::PushPayload;

const OAUTH_URL: &str = "https://oauth-login.cloud.huawei.com/oauth2/v3/token";
const PUSH_API_BASE: &str = "https://push-api.cloud.huawei.com/v3";
/// Refresh ahead of the ~3600s expiry so we never race the boundary.
const TOKEN_SKEW: Duration = Duration::from_secs(300);

struct CachedToken {
    access_token: String,
    fetched_at: Instant,
    ttl: Duration,
}

impl CachedToken {
    /// Still usable if it won't expire within the skew window.
    fn is_valid(&self) -> bool {
        self.fetched_at.elapsed() + TOKEN_SKEW < self.ttl
    }
}

pub struct HarmonyPush {
    client_id: String,
    client_secret: String,
    project_id: String,
    http: reqwest::Client,
    token: Mutex<Option<CachedToken>>,
}

impl HarmonyPush {
    /// Construct from env, or `None` if any credential is absent/blank.
    pub fn from_env() -> Option<Self> {
        let nonblank = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let client_id = nonblank("RELAY_HARMONY_CLIENT_ID")?;
        let client_secret = nonblank("RELAY_HARMONY_CLIENT_SECRET")?;
        let project_id = nonblank("RELAY_HARMONY_PROJECT_ID")?;
        Some(Self {
            client_id,
            client_secret,
            project_id,
            http: reqwest::Client::new(),
            token: Mutex::new(None),
        })
    }

    /// Cached-or-fresh OAuth2 access token (client-credentials grant).
    async fn access_token(&self) -> Result<String, String> {
        if let Some(t) = self.token.lock().unwrap().as_ref() {
            if t.is_valid() {
                return Ok(t.access_token.clone());
            }
        }
        let resp = self
            .http
            .post(OAUTH_URL)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("oauth request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("oauth returned HTTP {}", resp.status()));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("oauth response not json: {e}"))?;
        let access_token = body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or("oauth response has no access_token")?
            .to_string();
        let ttl = Duration::from_secs(
            body.get("expires_in").and_then(Value::as_u64).unwrap_or(3600),
        );
        *self.token.lock().unwrap() = Some(CachedToken {
            access_token: access_token.clone(),
            fetched_at: Instant::now(),
            ttl,
        });
        Ok(access_token)
    }

    /// Send one notification to a Push Kit token. Returns Err on transport /
    /// auth / API failure; the caller logs and (later) prunes dead tokens.
    pub async fn send(&self, token: &str, payload: &PushPayload<'_>) -> Result<(), String> {
        let access = self.access_token().await?;
        let url = format!("{PUSH_API_BASE}/{}/messages:send", self.project_id);
        let msg = build_message(token, payload);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(access)
            .header("push-type", "0") // 0 = alert / notification-bar message
            .json(&msg)
            .send()
            .await
            .map_err(|e| format!("push-api request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("push-api returned HTTP {}", resp.status()));
        }
        Ok(())
    }
}

/// Build the Push Kit `messages:send` request body from a relay payload.
///
/// `category: IM` keeps decision cards on the service/instant-message
/// frequency policy (NOT `MARKETING`, whose 2-5/day cap would drop them).
/// `clickAction.actionType 3` = open the app's home page — a safe skeleton
/// default; P5 (the ArkTS atomic service) will refine this into a deeplink
/// built from `payload.tag` / `payload.url`.
fn build_message(token: &str, payload: &PushPayload<'_>) -> Value {
    serde_json::json!({
        "payload": {
            "notification": {
                "category": "IM",
                "title": payload.title,
                "body": payload.body,
                "clickAction": { "actionType": 3 }
            }
        },
        "target": { "token": [token] }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_message_shapes_push_kit_body() {
        let payload = PushPayload {
            title: "Fleet",
            body: "新决策卡",
            tag: Some("decision:42"),
            url: Some("/"),
        };
        let msg = build_message("TKN123", &payload);
        assert_eq!(msg["payload"]["notification"]["title"], "Fleet");
        assert_eq!(msg["payload"]["notification"]["body"], "新决策卡");
        // category must be a service/IM class, never MARKETING (frequency cap).
        assert_eq!(msg["payload"]["notification"]["category"], "IM");
        // token goes into target.token as a single-element array.
        assert_eq!(msg["target"]["token"][0], "TKN123");
        assert_eq!(msg["target"]["token"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn cached_token_expires_within_skew() {
        // A token whose whole ttl is inside the skew window is never valid.
        let t = CachedToken {
            access_token: "x".into(),
            fetched_at: Instant::now(),
            ttl: Duration::from_secs(100), // < TOKEN_SKEW (300)
        };
        assert!(!t.is_valid(), "ttl inside skew window must be treated as expired");

        let fresh = CachedToken {
            access_token: "x".into(),
            fetched_at: Instant::now(),
            ttl: Duration::from_secs(3600),
        };
        assert!(fresh.is_valid(), "a full-hour token just fetched is valid");
    }

    #[test]
    fn from_env_none_without_all_creds() {
        // With no RELAY_HARMONY_* set in this test process, config is absent.
        // (Set-and-restore would race other tests; absence is the default.)
        std::env::remove_var("RELAY_HARMONY_CLIENT_ID");
        std::env::remove_var("RELAY_HARMONY_CLIENT_SECRET");
        std::env::remove_var("RELAY_HARMONY_PROJECT_ID");
        assert!(HarmonyPush::from_env().is_none());
    }
}
