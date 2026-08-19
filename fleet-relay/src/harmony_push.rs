//! Huawei Push Kit delivery channel for HarmonyOS 元服务 (atomic service) clients.
//!
//! A HarmonyOS 元服务 cannot use Web Push, and its notifications are delivered
//! through Push Kit's **account-based service-notification** channel, which is a
//! different beast from the app "push token" channel:
//!   * the recipient is identified by their Huawei-account **OpenID** (obtained
//!     client-side via Account Kit), NOT a device push token;
//!   * the message is a pre-approved **subscription template** (`templateId` +
//!     `templateParams`), NOT a free-form title/body notification;
//!   * the request authenticates with a self-signed **PS256 JWT** built from an
//!     AGC service-account key, NOT an OAuth2 client-credentials Bearer token;
//!   * the endpoint is `v1/{projectId}/service_notification/send`, NOT the app
//!     channel's `v3/{projectId}/messages:send`.
//!
//! This whole shape was verified end-to-end against a real device on 2026-07-14
//! (send returns `code 80000000` and the phone shows the notification); see
//! memory `project_harmony_push_kit` for the trail.
//!
//! `from_env()` returns `None` unless every `RELAY_HARMONY_*` credential is set
//! — a missing/invalid config degrades to a no-op so existing Web Push
//! deployments are unaffected. The credentials come from AppGallery Connect /
//! the Huawei API Console; see wiki: mobile/harmony-push-kit-setup.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::Value;

use crate::frames::PushPayload;

/// Account service-notification send endpoint base (元服务 channel).
const SVC_API_BASE: &str = "https://push-api.cloud.huawei.com/v1";
/// Device-token send endpoint base (普通应用 channel). Same host, same JWT auth,
/// different major version and a completely different body shape — see
/// [`build_app_notification`].
const APP_API_BASE: &str = "https://push-api.cloud.huawei.com/v3";
/// The `aud` every Push Kit service-account JWT must carry — the OAuth token
/// endpoint. Omitting it makes the send 401 even though we never call this URL
/// directly (the self-signed JWT is used as the Bearer verbatim).
const JWT_AUD: &str = "https://oauth-login.cloud.huawei.com/oauth2/v3/token";
/// Push Kit accepts a service-account JWT for up to 3600s; we mint one that
/// long and reuse it until the skew window.
const JWT_TTL: Duration = Duration::from_secs(3600);
/// Refresh ahead of expiry so we never race the boundary.
const JWT_SKEW: Duration = Duration::from_secs(300);
/// The single success code the send endpoint returns (HTTP 200 with this in the
/// JSON body); anything else is a business error (e.g. `82500006` bad OpenID).
const SUCCESS_CODE: &str = "80000000";

/// Push Kit business codes that mean the recipient OpenID will never receive
/// again, so the caller should drop the subscription:
///   * `82500006` — invalid OpenID (from this module's original comment).
///   * `80300007` — `atomicUnableSendUnsubscribedMsg`, i.e. the user revoked
///     the service-notification subscription (from the setup wiki).
///
/// CAVEAT: neither code has been confirmed on a real device to be *permanent*
/// (the success path `80000000` and the OAuth-param errors were verified, these
/// two were only read off docs/comments). They are treated as the ONLY
/// prune-worthy codes; every other failure — transport, auth, unknown business
/// code — is transient and keeps the subscription, so a temporary Push Kit
/// hiccup never evicts a live sub. If an on-device test later shows one of
/// these is actually retryable (or surfaces another permanent code), adjust
/// this list. Under-pruning here is harmless (a stale entry that keeps getting
/// skipped); over-pruning silently drops a working subscriber, so we err toward
/// keeping.
const DEAD_OPENID_CODES: &[&str] = &["82500006", "80300007"];

fn is_dead_openid_code(code: &str) -> bool {
    DEAD_OPENID_CODES.contains(&code)
}

/// Codes that mean a device push token will never deliver again.
///
/// Only `80300007` (invalid token). Deliberately narrower than it could be: we
/// send exactly one token per request, so the multi-token "partial success"
/// code `80100000` — whose invalid entries live in a separate `illegal_tokens`
/// field — never applies here. Same bias as [`DEAD_OPENID_CODES`]: under-pruning
/// leaves a dead entry that keeps getting skipped, over-pruning silently drops
/// a working device.
const DEAD_TOKEN_CODES: &[&str] = &["80300007"];

fn is_dead_token_code(code: &str) -> bool {
    DEAD_TOKEN_CODES.contains(&code)
}

/// Notification category for the app channel.
///
/// `WORK` matches what the 元服务 template (「工作事项提醒」) was approved for and
/// is the right classification for a decision card — it is a work item awaiting
/// the user, not marketing.
///
/// CAVEAT (unverified on device): Huawei gates every category except `MARKETING`
/// behind 自分类权益 approval, applied for per app in AGC. Without it the send is
/// expected to fail — loudly, as a `Transient` error in the logs, which is why
/// this defaults to the value we actually want rather than silently degrading.
/// `MARKETING` does get accepted without approval but is rate-limited to a
/// handful of messages per device per day, which would drop decision cards on
/// the floor and look like a bug. Override with `RELAY_HARMONY_CATEGORY` if the
/// approval is still pending.
const DEFAULT_CATEGORY: &str = "WORK";

/// Why a [`HarmonyPush::send`] failed, so the caller can decide whether to
/// prune the OpenID. The inner `String` is a human-readable detail for logs.
#[derive(Debug)]
pub enum SendError {
    /// Push Kit reported this recipient as permanently invalid / unsubscribed —
    /// the caller should remove the subscription. Which codes count depends on
    /// the channel: [`DEAD_OPENID_CODES`] for 元服务, [`DEAD_TOKEN_CODES`] for the
    /// app channel.
    DeadRecipient(String),
    /// Transport / auth / unknown-code failure — keep the subscription, retry
    /// on the next notify.
    Transient(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::DeadRecipient(d) => write!(f, "dead recipient: {d}"),
            SendError::Transient(d) => write!(f, "{d}"),
        }
    }
}

struct CachedJwt {
    token: String,
    minted_at: Instant,
    ttl: Duration,
}

impl CachedJwt {
    /// Still usable if it won't expire within the skew window.
    fn is_valid(&self) -> bool {
        self.minted_at.elapsed() + JWT_SKEW < self.ttl
    }
}

/// JWT claims for the Push Kit service account. `iss` is the service account's
/// `sub_account`; `aud` is fixed (see [`JWT_AUD`]).
#[derive(Serialize)]
struct JwtClaims {
    aud: String,
    iss: String,
    iat: u64,
    exp: u64,
}

pub struct HarmonyPush {
    /// Only in the send URL path.
    project_id: String,
    /// Application-side App ID — goes into the request body's `appId`.
    app_id: String,
    /// Subscription template id claimed in AGC — the body's `templateId`.
    /// 元服务 channel only; the app channel has no templates.
    template_id: String,
    /// Notification category for the app channel — see [`DEFAULT_CATEGORY`].
    category: String,
    /// Service-account key id — the JWT header `kid`.
    key_id: String,
    /// Service-account `sub_account` — the JWT `iss`.
    sub_account: String,
    /// PS256 signing key parsed from the service-account RSA private key.
    encoding_key: EncodingKey,
    http: reqwest::Client,
    jwt: Mutex<Option<CachedJwt>>,
}

impl HarmonyPush {
    /// Construct from env, or `None` if any credential is absent/blank or the
    /// private key doesn't parse as an RSA PEM.
    pub fn from_env() -> Option<Self> {
        let nonblank = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let project_id = nonblank("RELAY_HARMONY_PROJECT_ID")?;
        let app_id = nonblank("RELAY_HARMONY_APP_ID")?;
        let template_id = nonblank("RELAY_HARMONY_TEMPLATE_ID")?;
        let category = nonblank("RELAY_HARMONY_CATEGORY")
            .unwrap_or_else(|| DEFAULT_CATEGORY.to_string());
        let key_id = nonblank("RELAY_HARMONY_KEY_ID")?;
        let sub_account = nonblank("RELAY_HARMONY_SUB_ACCOUNT")?;
        // The PEM is multi-line; docker/compose commonly delivers it with the
        // newlines escaped as literal `\n`, so accept that shape too.
        let private_key = std::env::var("RELAY_HARMONY_PRIVATE_KEY")
            .ok()
            .map(|s| s.replace("\\n", "\n"))
            .filter(|s| !s.trim().is_empty())?;
        Self::new(project_id, app_id, template_id, category, key_id, sub_account, &private_key)
            .map_err(|e| log::warn!("HarmonyOS Push Kit disabled — bad private key: {e}"))
            .ok()
    }

    /// Build from explicit values (env parsing lives in [`from_env`]; tests use
    /// this directly). Fails only if the RSA private key PEM is unparseable.
    fn new(
        project_id: String,
        app_id: String,
        template_id: String,
        category: String,
        key_id: String,
        sub_account: String,
        private_key_pem: &str,
    ) -> Result<Self, String> {
        let encoding_key =
            EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).map_err(|e| format!("{e}"))?;
        Ok(Self {
            project_id,
            app_id,
            template_id,
            category,
            key_id,
            sub_account,
            encoding_key,
            http: reqwest::Client::new(),
            jwt: Mutex::new(None),
        })
    }

    /// Cached-or-fresh PS256 JWT used directly as the `Authorization: Bearer`.
    fn jwt(&self) -> Result<String, String> {
        if let Some(c) = self.jwt.lock().unwrap().as_ref() {
            if c.is_valid() {
                return Ok(c.token.clone());
            }
        }
        let iat = unix_secs();
        let exp = iat + JWT_TTL.as_secs();
        let mut header = Header::new(Algorithm::PS256);
        header.kid = Some(self.key_id.clone());
        let claims = JwtClaims {
            aud: JWT_AUD.to_string(),
            iss: self.sub_account.clone(),
            iat,
            exp,
        };
        let token = jsonwebtoken::encode(&header, &claims, &self.encoding_key)
            .map_err(|e| format!("sign PS256 jwt: {e}"))?;
        *self.jwt.lock().unwrap() = Some(CachedJwt {
            token: token.clone(),
            minted_at: Instant::now(),
            ttl: JWT_TTL,
        });
        Ok(token)
    }

    /// POST one Push Kit request and classify the outcome.
    ///
    /// Push Kit answers HTTP 200 with a business `code` on both success and
    /// failure, so the code — not the HTTP status — is authoritative. The two
    /// channels differ only in which codes mean "this recipient is gone", hence
    /// `is_dead` as a parameter rather than a fixed list.
    async fn post(
        &self,
        what: &str,
        url: String,
        body: Value,
        is_dead: fn(&str) -> bool,
    ) -> Result<(), SendError> {
        let jwt = self.jwt().map_err(SendError::Transient)?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(jwt)
            .json(&body)
            .send()
            .await
            .map_err(|e| SendError::Transient(format!("{what} request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| SendError::Transient(format!("{what} response body: {e}")))?;
        let code = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("code").and_then(Value::as_str).map(str::to_string));
        match code.as_deref() {
            Some(SUCCESS_CODE) => Ok(()),
            Some(c) if is_dead(c) => {
                Err(SendError::DeadRecipient(format!("code {c}: {text} (http {status})")))
            }
            Some(c) => Err(SendError::Transient(format!(
                "{what} code {c}: {text} (http {status})"
            ))),
            None => Err(SendError::Transient(format!(
                "{what} unexpected response (http {status}): {text}"
            ))),
        }
    }

    /// Send one service notification to a Huawei-account OpenID (元服务 channel).
    pub async fn send(&self, open_id: &str, payload: &PushPayload<'_>) -> Result<(), SendError> {
        let url = format!("{SVC_API_BASE}/{}/service_notification/send", self.project_id);
        let body = build_service_notification(
            &gen_msg_id(),
            &self.app_id,
            &self.template_id,
            open_id,
            payload,
        );
        self.post("service_notification", url, body, is_dead_openid_code).await
    }

    /// Send one notification to a device push token (普通应用 channel).
    ///
    /// This is the channel the HarmonyOS **app** build and the Android shell
    /// both use — same endpoint, same body, only the token differs — which is
    /// why there is one implementation rather than one per platform.
    pub async fn send_token(&self, token: &str, payload: &PushPayload<'_>) -> Result<(), SendError> {
        let url = format!("{APP_API_BASE}/{}/messages:send", self.project_id);
        let body = build_app_notification(&self.category, token, payload);
        self.post("messages:send", url, body, is_dead_token_code).await
    }
}

/// Seconds since the Unix epoch (JWT `iat`/`exp`).
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A unique message id in `[1,64]` chars (Push Kit requires the sender to mint
/// it). 32 hex chars of 128-bit entropy is unique in practice and well-formed.
fn gen_msg_id() -> String {
    format!("{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>())
}

/// Build the `service_notification/send` request body from a relay payload.
///
/// The recipient is the Huawei-account `toOpenId` (account subscription), and
/// the visible content comes from the claimed template's params: `thing_0` is
/// the reminder content (the decision preview) and `thing_4` the publishing
/// unit (the workspace title). This matches the AGC template `1BAAD76B2A818700`
/// (「工作事项提醒」) verified on-device.
fn build_service_notification(
    msg_id: &str,
    app_id: &str,
    template_id: &str,
    open_id: &str,
    payload: &PushPayload<'_>,
) -> Value {
    serde_json::json!({
        "msgId": msg_id,
        "toOpenId": open_id,
        "appId": app_id,
        "templateId": template_id,
        "templateParams": {
            "thing_0": payload.body,
            "thing_4": payload.title,
        }
    })
}

/// App-channel notification body.
///
/// Nothing is shared with the 元服务 shape: no `msgId`, no `appId`, no template —
/// the title/body are free-form, and the recipient is a device token array.
///
/// `clickAction.actionType: 0` opens the app's home page. Deep-linking straight
/// to the decision card would need a custom action plus an on-device check that
/// the parameters survive the launch, so that is left to a follow-up rather than
/// asserted here — `payload.url` is deliberately not wired up yet.
fn build_app_notification(category: &str, token: &str, payload: &PushPayload<'_>) -> Value {
    serde_json::json!({
        "payload": {
            "notification": {
                "category": category,
                "title": payload.title,
                "body": payload.body,
                "clickAction": { "actionType": 0 }
            }
        },
        "target": { "token": [token] }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    // Throwaway 2048-bit RSA key, generated offline for these tests only — it
    // signs nothing real (no live service account uses it).
    const TEST_RSA_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCq5ZDVEV9TG/sL
IjaTd7eWhHgflQSTh+fqZ5Tl8NkE+aOTJA/sCiD1INohJ3AcK+03E3KpCfy472rP
4lavr95x+X9EUW5yNUiwkH9aOQmKdguZMYT4eC8geBMIj9TM7l77vN6nA8qi6gG3
ELYmGXEBOqi5ffXrdMs0vvGFS8QnjkN4ECPxnbNPkNoXYUPWnPbuuuDXvepFaCgk
+nfQL04IAr4EBKlr2VYc+OgT8KfpIIKhCkVmtyDZJYpN8XtFqCsXzWLzI7aFbuzX
nmCIliWCLz3YBRIcce0+kMYalpHz7Zg7M4LcjI/pGLg0KVkVJlUwH8g4uUDmWvBV
0AvvIxOLAgMBAAECggEAHbJz1xrEOmB/4QDqg/jHTeAqMa7HE04IJosmbEjMpZkt
7ClVnPprnL0/OoisajDV8X6vK6HBKW/tWz+lObVxjdPB4mDWNQF+ZrRNdSO5PTbj
MBVTWowkAFvtTs0TuSLpHYzGEjbN9T592s6MIJXGNeT4Ife6DtcGYt2VgcjXRtog
H+gRQpb7kBckH5UuPJxEsSS9OaMADqbVEsZeQ6VslhQ76m3TeOo0HxLFt3+Eszgp
pGU0d0GE/VNCfoNjD8ke9TH9M3lY3PN+WCVCeu9MxcMH0wYzD0K5zQBWftP6ue0v
MDiiRqw8C6QU4pb/d2BLIULbJRWTP+5tVHDQT69O/QKBgQDuHDI2Mtnlh9IVNYdC
v/69iFpFsN99z6jpznt7kYNgEZMqU9ugf219l/ISV3fcNzVLEEDFole28XRt1SMy
WvIHY+sHLxUntk5RMNGixDRXOcmHHryFmMwtdgs87lMe9i8jEkvsjsne9szG5Iad
oOAW/3q2m0y1fJXHSCTw1/2SjwKBgQC3vJbErD+DN2SUSguYuVHSutqz5H68/KUY
eEtUAHUNs+FPAByseVZGy2NFw7orvduv8xck3/c9yFnL21JnAxjOBH6Ty41ulDmQ
aLYASe8DV7iXWnS6pyV2DxRa5YMQIafoqHb1Aufjg+8doqzQDxGkceavxBgHKK3D
6bXVc3m9RQKBgQDeWWguInhFlfqBIbZIh9+K/9xEffGFm7hBdTbsYirLOD4z5ZEP
JE+LW6uKozFjbA6RJQFHTN0aEgnGUqUGbdTbP9wGnlnj9qLVwH/SveOenHDrg7FK
FDB+N2AxKuBl5kCIQJqzsXcjhVYeWOK4KbV45GOkSAtu4oM/T8hnO1soUwKBgQCx
ywr1q1wWJC7uk6wfEAzOZsOk2dGOHMfBIv55favHI649XPViLFPBU6RvaNOo6iJA
Y3Gc3CCKJ2pFKqjVR5jkGlNFvu3P+Byv0bN0Ghbv3B2iUASubXmBgVwIDRlDLd4l
84aQ1kv/y7ZBrh2dg0dmIlXA9Xbuzn0/G9M5hnFiJQKBgBCkvW49yb2ViHAk6xHv
8ky7wHK5tGtIq76/JPNJEn17gQ7phvXy+nLqxC0tlgYK7YYe2fkAwIgEBaV6ds++
e5wOAQKKoZmhAVBhHFC9sgloQIw+FjbzFWxJ3hh8Ejuw4mYBaMTVutPU5wJmQImV
cHMuOFehtqcSyMaY3z552xNj
-----END PRIVATE KEY-----";

    fn mk_push() -> HarmonyPush {
        HarmonyPush::new(
            "PROJ".into(),
            "APP123".into(),
            "TPL456".into(),
            "WORK".into(),
            "KEYID789".into(),
            "sub-acct-1".into(),
            TEST_RSA_PEM,
        )
        .expect("test RSA PEM parses")
    }

    #[test]
    fn build_body_shapes_service_notification() {
        let payload = PushPayload {
            title: "netferry",
            body: "有一个 AI 任务待确认",
            tag: Some("fleet-ask:42"),
            url: Some("/"),
        };
        let body = build_service_notification("MSG-1", "APP123", "TPL456", "OPENID-XYZ", &payload);
        assert_eq!(body["msgId"], "MSG-1");
        assert_eq!(body["toOpenId"], "OPENID-XYZ");
        assert_eq!(body["appId"], "APP123");
        assert_eq!(body["templateId"], "TPL456");
        // thing_0 = reminder content (body); thing_4 = publishing unit (title).
        assert_eq!(body["templateParams"]["thing_0"], "有一个 AI 任务待确认");
        assert_eq!(body["templateParams"]["thing_4"], "netferry");
    }

    #[test]
    fn gen_msg_id_is_hex_and_within_len_bound() {
        let id = gen_msg_id();
        assert_eq!(id.len(), 32);
        assert!(id.len() <= 64 && !id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // Two draws don't collide (probabilistically certain).
        assert_ne!(gen_msg_id(), gen_msg_id());
    }

    #[test]
    fn jwt_carries_ps256_kid_and_required_claims() {
        let hp = mk_push();
        let token = hp.jwt().expect("sign jwt");

        // Header: alg must be PS256, kid must be the service-account key id.
        let header = jsonwebtoken::decode_header(&token).expect("decode header");
        assert_eq!(header.alg, Algorithm::PS256);
        assert_eq!(header.kid.as_deref(), Some("KEYID789"));

        // Payload: decode the middle segment ourselves (no verification needed —
        // we're asserting the claim shape, not the signature).
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "a JWT has three dot-separated segments");
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("payload is base64url");
        let claims: Value = serde_json::from_slice(&raw).expect("payload is json");
        // aud is fixed and load-bearing (its absence 401s the send).
        assert_eq!(claims["aud"], JWT_AUD);
        assert_eq!(claims["iss"], "sub-acct-1");
        let iat = claims["iat"].as_u64().unwrap();
        let exp = claims["exp"].as_u64().unwrap();
        assert_eq!(exp - iat, JWT_TTL.as_secs(), "exp is iat + 1h");
    }

    #[test]
    fn jwt_is_cached_across_calls() {
        let hp = mk_push();
        let a = hp.jwt().unwrap();
        let b = hp.jwt().unwrap();
        assert_eq!(a, b, "a still-valid JWT is reused, not re-minted");
    }

    #[test]
    fn cached_jwt_expires_within_skew() {
        let stale = CachedJwt {
            token: "x".into(),
            minted_at: Instant::now(),
            ttl: Duration::from_secs(100), // < JWT_SKEW (300)
        };
        assert!(!stale.is_valid(), "ttl inside skew window counts as expired");
        let fresh = CachedJwt {
            token: "x".into(),
            minted_at: Instant::now(),
            ttl: JWT_TTL,
        };
        assert!(fresh.is_valid(), "a full-hour JWT just minted is valid");
    }

    #[test]
    fn from_env_none_without_all_creds() {
        for v in [
            "RELAY_HARMONY_PROJECT_ID",
            "RELAY_HARMONY_APP_ID",
            "RELAY_HARMONY_TEMPLATE_ID",
            "RELAY_HARMONY_KEY_ID",
            "RELAY_HARMONY_SUB_ACCOUNT",
            "RELAY_HARMONY_PRIVATE_KEY",
        ] {
            std::env::remove_var(v);
        }
        assert!(HarmonyPush::from_env().is_none());
    }

    #[test]
    fn new_rejects_bad_private_key() {
        let r = HarmonyPush::new(
            "P".into(),
            "A".into(),
            "T".into(),
            "C".into(),
            "K".into(),
            "S".into(),
            "not a pem",
        );
        assert!(r.is_err());
    }

    #[test]
    fn only_allowlisted_codes_are_dead() {
        // The two allowlisted permanent codes prune.
        assert!(is_dead_openid_code("82500006"));
        assert!(is_dead_openid_code("80300007"));
        // Everything else — success, unknown business codes, OAuth errors — is
        // transient and must NOT prune (guards against over-pruning live subs).
        assert!(!is_dead_openid_code(SUCCESS_CODE));
        assert!(!is_dead_openid_code("80300010"));
        assert!(!is_dead_openid_code("1101"));
        assert!(!is_dead_openid_code(""));
    }
}
