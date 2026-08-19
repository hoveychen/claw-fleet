//! Web Push: VAPID key management, per-channel subscription persistence and
//! notification fan-out.
//!
//! The VAPID private key is the raw 32-byte P-256 scalar, base64url (no pad),
//! provided via `RELAY_VAPID_KEY`. Like rca-relay's RCA_RELAY_KEY, a missing
//! key is generated at startup and logged so the operator can pin it —
//! browser subscriptions are bound to the public key, so an unpinned key
//! invalidates every subscription on restart.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use serde_json::Value;
use web_push::{
    ContentEncoding, HyperWebPushClient, SubscriptionInfo, VapidSignatureBuilder,
    WebPushClient, WebPushError, WebPushMessageBuilder,
};

use crate::frames::PushPayload;
use crate::harmony_push::{HarmonyPush, SendError};

pub struct Push {
    /// Raw 32-byte private scalar, base64url no-pad (what VapidSignatureBuilder eats).
    private_b64: String,
    /// Uncompressed P-256 public point, base64url no-pad (what pushManager.subscribe eats).
    pub public_b64: String,
    subject: String,
    subs_dir: PathBuf,
    /// Serializes read-modify-write cycles on the subscription files. `Arc` so
    /// notify's blocking file work can hold it inside a `spawn_blocking` closure
    /// (which must be `'static`) while the sync subscribe/unsubscribe paths hold
    /// it directly — all four contend on the same lock.
    file_lock: Arc<Mutex<()>>,
    /// Reused across notifies — the hyper client keeps a connection pool, so
    /// building it once (instead of per-notify) lets keep-alive connections to
    /// the push services be reused.
    web_client: HyperWebPushClient,
}

impl Push {
    pub fn new(private_b64: String, subject: String, data_dir: &Path) -> Result<Self, String> {
        let raw = URL_SAFE_NO_PAD
            .decode(private_b64.trim())
            .map_err(|e| format!("RELAY_VAPID_KEY is not base64url: {e}"))?;
        let secret = p256::SecretKey::from_slice(&raw)
            .map_err(|e| format!("RELAY_VAPID_KEY is not a valid P-256 scalar: {e}"))?;
        let public_b64 = URL_SAFE_NO_PAD.encode(
            secret
                .public_key()
                .to_encoded_point(false)
                .as_bytes(),
        );
        let subs_dir = data_dir.join("subs");
        fs::create_dir_all(&subs_dir)
            .map_err(|e| format!("create {}: {e}", subs_dir.display()))?;
        Ok(Self {
            private_b64: private_b64.trim().to_string(),
            public_b64,
            subject,
            subs_dir,
            file_lock: Arc::new(Mutex::new(())),
            web_client: HyperWebPushClient::new(),
        })
    }

    /// Generate a fresh private key (base64url no-pad raw scalar).
    pub fn generate_private_key() -> String {
        let secret = p256::SecretKey::random(&mut rand::rngs::OsRng);
        URL_SAFE_NO_PAD.encode(secret.to_bytes())
    }

    fn subs_path(&self, channel: &str) -> PathBuf {
        // channel is hex(sha256), safe as a file name
        self.subs_dir.join(format!("{channel}.json"))
    }

    fn load_subs(&self, channel: &str) -> Vec<Value> {
        Self::load_subs_at(&self.subs_path(channel))
    }

    fn save_subs(&self, channel: &str, subs: &[Value]) {
        Self::save_subs_at(&self.subs_path(channel), subs);
    }

    /// Path-only file read, so it can run inside a `spawn_blocking` closure that
    /// can't borrow `&self`. Missing/corrupt file → empty list.
    fn load_subs_at(path: &Path) -> Vec<Value> {
        let Ok(raw) = fs::read_to_string(path) else {
            return Vec::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// Path-only file write (companion to [`load_subs_at`]). Empty list removes
    /// the file, matching the instance method's semantics.
    fn save_subs_at(path: &Path, subs: &[Value]) {
        if subs.is_empty() {
            let _ = fs::remove_file(path);
            return;
        }
        if let Ok(raw) = serde_json::to_string(subs) {
            let _ = fs::write(path, raw);
        }
    }

    /// Register a subscription for the channel. Two client kinds share one
    /// store: a browser Web Push subscription (`endpoint` + `keys`, deduped by
    /// endpoint) and a HarmonyOS 元服务 account subscription
    /// (`platform:"harmony"` + `openId`, deduped by openId). The 元服务 channel
    /// delivers by Huawei-account OpenID, not a device push token — see
    /// `harmony_push.rs`. A subscription with no explicit `platform` is treated
    /// as Web Push for backward compat.
    pub fn subscribe(&self, channel: &str, subscription: Value) -> Result<(), String> {
        if Self::is_harmony(&subscription) {
            let key = Self::harmony_key(&subscription)
                .ok_or("harmony subscription has neither token nor openId")?;
            let _g = self.file_lock.lock().unwrap();
            let mut subs = self.load_subs(channel);
            subs.retain(|s| Self::harmony_key(s).as_deref() != Some(key.as_str()));
            subs.push(subscription);
            self.save_subs(channel, &subs);
            return Ok(());
        }

        let endpoint = subscription
            .get("endpoint")
            .and_then(Value::as_str)
            .ok_or("subscription has no endpoint")?
            .to_string();
        subscription
            .pointer("/keys/p256dh")
            .and_then(Value::as_str)
            .ok_or("subscription has no keys.p256dh")?;
        subscription
            .pointer("/keys/auth")
            .and_then(Value::as_str)
            .ok_or("subscription has no keys.auth")?;

        let _g = self.file_lock.lock().unwrap();
        let mut subs = self.load_subs(channel);
        subs.retain(|s| s.get("endpoint").and_then(Value::as_str) != Some(endpoint.as_str()));
        subs.push(subscription);
        self.save_subs(channel, &subs);
        Ok(())
    }

    /// Remove a subscription from the channel — the mirror of [`subscribe`].
    /// A harmony subscription (`platform:"harmony"`) is matched by `openId`; a
    /// web subscription by `endpoint`. Removing a subscription that isn't
    /// present is a no-op (`Ok`). When the last subscription goes, the channel
    /// file is deleted (via `save_subs`).
    pub fn unsubscribe(&self, channel: &str, subscription: &Value) -> Result<(), String> {
        if Self::is_harmony(subscription) {
            let key = Self::harmony_key(subscription)
                .ok_or("harmony unsubscribe has neither token nor openId")?;
            let _g = self.file_lock.lock().unwrap();
            let mut subs = self.load_subs(channel);
            let before = subs.len();
            subs.retain(|s| Self::harmony_key(s).as_deref() != Some(key.as_str()));
            if subs.len() != before {
                self.save_subs(channel, &subs);
            }
            return Ok(());
        }

        let endpoint = subscription
            .get("endpoint")
            .and_then(Value::as_str)
            .ok_or("unsubscribe has no endpoint")?
            .to_string();
        let _g = self.file_lock.lock().unwrap();
        let mut subs = self.load_subs(channel);
        let before = subs.len();
        subs.retain(|s| s.get("endpoint").and_then(Value::as_str) != Some(endpoint.as_str()));
        if subs.len() != before {
            self.save_subs(channel, &subs);
        }
        Ok(())
    }

    /// True if the subscription is a HarmonyOS Push Kit registration.
    fn is_harmony(sub: &Value) -> bool {
        sub.get("platform").and_then(Value::as_str) == Some("harmony")
    }

    /// Identity of a harmony subscription, used for dedup and for pruning.
    ///
    /// Two Push Kit channels land in the same store: the 元服务 account channel
    /// keys on `openId`, the 普通应用 device channel on `token`. Prefixing keeps
    /// them from ever colliding, and the prefix is what `notify` reads back to
    /// decide which endpoint to call — so the wire shape alone determines the
    /// channel and no extra discriminator field has to be kept in sync.
    ///
    /// `token` wins when both are present: a device that has migrated from the
    /// 元服务 build should be reached the new way.
    fn harmony_key(sub: &Value) -> Option<String> {
        if !Self::is_harmony(sub) {
            return None;
        }
        if let Some(t) = sub.get("token").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return Some(format!("token:{t}"));
        }
        sub.get("openId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|o| format!("openid:{o}"))
    }

    pub fn subscription_count(&self, channel: &str) -> usize {
        let _g = self.file_lock.lock().unwrap();
        self.load_subs(channel).len()
    }

    /// Push `payload` to every subscription of the channel. Subscriptions the
    /// push service reports as gone (unsubscribed / expired) are pruned.
    pub async fn notify(
        &self,
        channel: &str,
        payload: &PushPayload<'_>,
        harmony: Option<&HarmonyPush>,
    ) {
        // Read the subscription file off the async worker threads: file I/O is
        // blocking, and notify runs on every decision card.
        let subs = {
            let lock = self.file_lock.clone();
            let path = self.subs_path(channel);
            tokio::task::spawn_blocking(move || {
                let _g = lock.lock().unwrap();
                Self::load_subs_at(&path)
            })
            .await
            .unwrap_or_default()
        };
        if subs.is_empty() {
            return;
        }
        // Web Push body is JSON; harmony uses the typed payload directly. A
        // failed encode only disables the web leg, not the harmony one.
        // `as_deref()` yields an `Option<&[u8]>` (Copy), so every per-sub future
        // can capture it by value while `web_body` stays owned on this stack.
        let web_body = serde_json::to_vec(payload).ok();
        let web_body = web_body.as_deref();

        // Fan out concurrently: one future per subscription, all polled together
        // in THIS task (join_all, no spawn) so they can borrow `&self` / payload
        // / harmony. Wall-clock becomes the slowest single send, not their sum.
        let outcomes = futures_util::future::join_all(subs.iter().map(|sub| async move {
            if Self::is_harmony(sub) {
                // Two Push Kit channels, picked by the subscription's own shape:
                // a device `token` goes to the 普通应用 endpoint
                // (`v3/messages:send`), an `openId` to the 元服务
                // service-notification endpoint. No-op when the channel is
                // disabled (creds absent → harmony is None).
                let (Some(hp), Some(key)) = (harmony, Self::harmony_key(sub)) else {
                    return SendOutcome::Ok;
                };
                let sent = match key.strip_prefix("token:") {
                    Some(token) => hp.send_token(token, payload).await,
                    None => hp.send(key.trim_start_matches("openid:"), payload).await,
                };
                return match sent {
                    Ok(()) => SendOutcome::Ok,
                    Err(SendError::DeadRecipient(detail)) => {
                        log::info!("harmony recipient dead on channel {channel}, pruning: {detail}");
                        SendOutcome::DeadHarmony(key)
                    }
                    Err(SendError::Transient(detail)) => {
                        log::warn!("harmony push failed on channel {channel}: {detail}");
                        SendOutcome::Ok
                    }
                };
            }
            let Some(body) = web_body else { return SendOutcome::Ok };
            let info: SubscriptionInfo = match serde_json::from_value(sub.clone()) {
                Ok(i) => i,
                Err(_) => return SendOutcome::Ok,
            };
            match self.send_one(&info, body).await {
                Ok(()) => SendOutcome::Ok,
                Err(WebPushError::EndpointNotValid(_) | WebPushError::EndpointNotFound(_)) => {
                    SendOutcome::DeadWeb(info.endpoint.clone())
                }
                Err(e) => {
                    log::warn!("web push to {} failed: {e}", info.endpoint);
                    SendOutcome::Ok
                }
            }
        }))
        .await;

        let mut dead: Vec<String> = Vec::new();
        // Harmony OpenIDs Push Kit reported as permanently invalid / unsubscribed
        // (see `harmony_push::DEAD_OPENID_CODES`). Kept separate from web `dead`
        // because harmony subs are matched by openId, not endpoint.
        let mut dead_harmony: Vec<String> = Vec::new();
        for outcome in outcomes {
            match outcome {
                SendOutcome::Ok => {}
                SendOutcome::DeadWeb(endpoint) => dead.push(endpoint),
                SendOutcome::DeadHarmony(open_id) => dead_harmony.push(open_id),
            }
        }
        if !dead.is_empty() || !dead_harmony.is_empty() {
            let web_n = dead.len();
            let harmony_n = dead_harmony.len();
            // Re-read → filter → write as one locked blocking unit off the async
            // workers. Re-reading under the lock (rather than reusing the `subs`
            // snapshot) preserves any subscribe that landed during fan-out.
            let lock = self.file_lock.clone();
            let path = self.subs_path(channel);
            let _ = tokio::task::spawn_blocking(move || {
                let _g = lock.lock().unwrap();
                let subs = Self::retain_live(Self::load_subs_at(&path), &dead, &dead_harmony);
                Self::save_subs_at(&path, &subs);
            })
            .await;
            log::info!(
                "pruned dead push subscription(s) on channel {channel}: web={web_n} harmony={harmony_n}"
            );
        }
    }

    /// Keep only the still-live subscriptions after a notify round: drop web
    /// subs whose endpoint is in `dead_web`, and harmony subs whose openId is
    /// in `dead_harmony`. A web endpoint dying never touches harmony subs (they
    /// have no endpoint); a harmony sub with no openId is kept (can't match a
    /// dead id), a web sub with no endpoint is dropped (malformed).
    fn retain_live(subs: Vec<Value>, dead_web: &[String], dead_harmony: &[String]) -> Vec<Value> {
        subs.into_iter()
            .filter(|s| {
                if Self::is_harmony(s) {
                    // Prefixed key (see `harmony_key`), so a dead device token
                    // can never evict an account subscription that happens to
                    // carry the same string.
                    return Self::harmony_key(s)
                        .map(|k| !dead_harmony.iter().any(|d| *d == k))
                        .unwrap_or(true);
                }
                s.get("endpoint")
                    .and_then(Value::as_str)
                    .map(|e| !dead_web.iter().any(|d| d == e))
                    .unwrap_or(false)
            })
            .collect()
    }

    async fn send_one(
        &self,
        info: &SubscriptionInfo,
        body: &[u8],
    ) -> Result<(), WebPushError> {
        let mut sig = VapidSignatureBuilder::from_base64(&self.private_b64, info)?;
        sig.add_claim("sub", self.subject.as_str());
        let mut msg = WebPushMessageBuilder::new(info);
        msg.set_payload(ContentEncoding::Aes128Gcm, body);
        msg.set_vapid_signature(sig.build()?);
        self.web_client.send(msg.build()?).await
    }
}

/// Per-subscription result of a notify fan-out, collected after all sends
/// complete concurrently. A dead entry drives pruning (web by endpoint, harmony
/// by openId); `Ok` covers both success and kept-transient failures.
enum SendOutcome {
    Ok,
    DeadWeb(String),
    DeadHarmony(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mk_push(dir: &Path) -> Push {
        Push::new(Push::generate_private_key(), "mailto:t@example.com".into(), dir).unwrap()
    }

    fn mk_sub(endpoint: &str) -> Value {
        json!({
            "endpoint": endpoint,
            "keys": { "p256dh": "BPk", "auth": "abc" }
        })
    }

    fn mk_harmony(open_id: &str) -> Value {
        json!({ "platform": "harmony", "openId": open_id })
    }

    /// 普通应用 channel registration (device push token) — see `harmony_key`.
    fn mk_harmony_token(token: &str) -> Value {
        json!({ "platform": "harmony", "token": token })
    }

    #[test]
    fn generated_key_roundtrips_and_derives_public() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        let pub_raw = URL_SAFE_NO_PAD.decode(&push.public_b64).unwrap();
        assert_eq!(pub_raw.len(), 65, "uncompressed P-256 point");
        assert_eq!(pub_raw[0], 0x04);
    }

    #[test]
    fn subscribe_dedups_by_endpoint_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch1", mk_sub("https://push/a")).unwrap();
        push.subscribe("ch1", mk_sub("https://push/b")).unwrap();
        push.subscribe("ch1", mk_sub("https://push/a")).unwrap();
        assert_eq!(push.subscription_count("ch1"), 2);
        assert_eq!(push.subscription_count("ch2"), 0);

        // fresh instance re-reads from disk
        let push2 = Push::new(Push::generate_private_key(), "mailto:t@example.com".into(), dir.path()).unwrap();
        assert_eq!(push2.subscription_count("ch1"), 2);
    }

    #[test]
    fn subscribe_rejects_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        assert!(push.subscribe("ch", json!({"keys": {}})).is_err());
        assert!(push.subscribe("ch", json!({"endpoint": "https://x"})).is_err());
    }

    #[test]
    fn subscribe_accepts_harmony_and_dedups_by_open_id() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch", mk_harmony("OID-A")).unwrap();
        push.subscribe("ch", mk_harmony("OID-B")).unwrap();
        push.subscribe("ch", mk_harmony("OID-A")).unwrap(); // duplicate openId
        assert_eq!(push.subscription_count("ch"), 2);
    }

    #[test]
    fn harmony_and_web_coexist_on_same_channel() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch", mk_sub("https://push/a")).unwrap();
        push.subscribe("ch", mk_harmony("OID-A")).unwrap();
        assert_eq!(push.subscription_count("ch"), 2);
    }

    #[test]
    fn subscribe_rejects_harmony_without_open_id() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        assert!(push.subscribe("ch", json!({ "platform": "harmony" })).is_err());
    }

    #[test]
    fn unsubscribe_removes_web_by_endpoint_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch", mk_sub("https://push/a")).unwrap();
        push.subscribe("ch", mk_sub("https://push/b")).unwrap();
        push.unsubscribe("ch", &mk_sub("https://push/a")).unwrap();
        assert_eq!(push.subscription_count("ch"), 1);

        // survives a fresh instance re-reading from disk
        let push2 =
            Push::new(Push::generate_private_key(), "mailto:t@example.com".into(), dir.path())
                .unwrap();
        assert_eq!(push2.subscription_count("ch"), 1);
    }

    #[test]
    fn unsubscribe_removes_harmony_by_open_id_only() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch", mk_harmony("OID-A")).unwrap();
        push.subscribe("ch", mk_harmony("OID-B")).unwrap();
        push.subscribe("ch", mk_sub("https://push/a")).unwrap();
        push.unsubscribe("ch", &mk_harmony("OID-A")).unwrap();
        assert_eq!(push.subscription_count("ch"), 2); // OID-B + web survive
        // the surviving harmony sub is OID-B, and the web sub is untouched
        let subs = push.load_subs("ch");
        assert!(subs
            .iter()
            .any(|s| s.get("openId").and_then(Value::as_str) == Some("OID-B")));
        assert!(subs
            .iter()
            .any(|s| s.get("endpoint").and_then(Value::as_str) == Some("https://push/a")));
    }

    #[test]
    fn unsubscribe_absent_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch", mk_sub("https://push/a")).unwrap();
        push.unsubscribe("ch", &mk_sub("https://push/gone")).unwrap();
        push.unsubscribe("ch", &mk_harmony("OID-gone")).unwrap();
        assert_eq!(push.subscription_count("ch"), 1);
    }

    #[test]
    fn unsubscribe_last_sub_deletes_channel_file() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("ch", mk_sub("https://push/a")).unwrap();
        push.unsubscribe("ch", &mk_sub("https://push/a")).unwrap();
        assert_eq!(push.subscription_count("ch"), 0);
        assert!(!push.subs_path("ch").exists(), "empty channel file is removed");
    }

    #[test]
    fn unsubscribe_rejects_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        assert!(push.unsubscribe("ch", &json!({})).is_err());
        assert!(push.unsubscribe("ch", &json!({ "platform": "harmony" })).is_err());
    }

    fn open_ids(subs: &[Value]) -> Vec<String> {
        subs.iter()
            .filter_map(|s| s.get("openId").and_then(Value::as_str).map(str::to_string))
            .collect()
    }

    #[test]
    fn retain_live_prunes_only_dead_harmony_open_ids() {
        let subs = vec![mk_harmony("OID-A"), mk_harmony("OID-B"), mk_sub("https://push/a")];
        let kept = Push::retain_live(subs, &[], &["openid:OID-A".to_string()]);
        assert_eq!(kept.len(), 2);
        assert_eq!(open_ids(&kept), vec!["OID-B"]); // OID-B kept
        assert!(kept
            .iter()
            .any(|s| s.get("endpoint").and_then(Value::as_str) == Some("https://push/a")));
    }

    #[test]
    fn retain_live_dead_web_never_evicts_harmony() {
        // Regression guard: a dying web endpoint must not drop harmony subs.
        let subs = vec![mk_harmony("OID-A"), mk_sub("https://push/dead")];
        let kept = Push::retain_live(subs, &["https://push/dead".to_string()], &[]);
        assert_eq!(open_ids(&kept), vec!["OID-A"]);
        assert_eq!(kept.len(), 1, "harmony sub survives web endpoint death");
    }

    #[test]
    fn retain_live_keeps_harmony_without_open_id() {
        // A malformed harmony sub (no openId) can't match a dead id → kept.
        let subs = vec![json!({ "platform": "harmony" })];
        let kept = Push::retain_live(subs, &[], &["openid:OID-A".to_string()]);
        assert_eq!(kept.len(), 1);
    }

    // ── 普通应用 (device token) channel ────────────────────────────────────

    #[test]
    fn harmony_key_prefers_token_over_open_id() {
        // A device that migrated from the 元服务 build can report both; the new
        // channel must win, otherwise it keeps getting the (now dead) account
        // service-notification.
        let both = json!({ "platform": "harmony", "openId": "OID-A", "token": "TOK-A" });
        assert_eq!(Push::harmony_key(&both).as_deref(), Some("token:TOK-A"));
    }

    #[test]
    fn harmony_key_ignores_blank_values() {
        let blank = json!({ "platform": "harmony", "token": "", "openId": "OID-A" });
        assert_eq!(Push::harmony_key(&blank).as_deref(), Some("openid:OID-A"));
        let empty = json!({ "platform": "harmony", "token": "", "openId": "" });
        assert_eq!(Push::harmony_key(&empty), None);
    }

    #[test]
    fn subscribe_dedups_harmony_by_token() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("chan", mk_harmony_token("TOK-A")).unwrap();
        push.subscribe("chan", mk_harmony_token("TOK-A")).unwrap();
        push.subscribe("chan", mk_harmony_token("TOK-B")).unwrap();
        assert_eq!(push.subscription_count("chan"), 2);
    }

    #[test]
    fn unsubscribe_removes_harmony_token() {
        let dir = tempfile::tempdir().unwrap();
        let push = mk_push(dir.path());
        push.subscribe("chan", mk_harmony_token("TOK-A")).unwrap();
        push.subscribe("chan", mk_harmony("OID-A")).unwrap();
        push.unsubscribe("chan", &mk_harmony_token("TOK-A")).unwrap();
        assert_eq!(open_ids(&push.load_subs("chan")), vec!["OID-A"]);
    }

    #[test]
    fn retain_live_prunes_dead_token_without_touching_same_named_open_id() {
        // Exactly what the `token:` / `openid:` prefixes exist for: the two
        // channels share one store, and an unprefixed key would let a dead
        // device token evict an unrelated account subscription (or vice versa)
        // whenever the two strings happened to match.
        let subs = vec![mk_harmony_token("SAME"), mk_harmony("SAME")];
        let kept = Push::retain_live(subs, &[], &["token:SAME".to_string()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(open_ids(&kept), vec!["SAME"], "the openId sub survives");
    }

    #[test]
    fn retain_live_dead_open_id_never_evicts_token_sub() {
        let subs = vec![mk_harmony_token("SAME"), mk_harmony("SAME")];
        let kept = Push::retain_live(subs, &[], &["openid:SAME".to_string()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].get("token").and_then(Value::as_str),
            Some("SAME"),
            "the token sub survives"
        );
    }
}
