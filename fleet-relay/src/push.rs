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
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use serde_json::Value;
use web_push::{
    ContentEncoding, HyperWebPushClient, SubscriptionInfo, VapidSignatureBuilder,
    WebPushClient, WebPushError, WebPushMessageBuilder,
};

use crate::frames::PushPayload;
use crate::harmony_push::HarmonyPush;

pub struct Push {
    /// Raw 32-byte private scalar, base64url no-pad (what VapidSignatureBuilder eats).
    private_b64: String,
    /// Uncompressed P-256 public point, base64url no-pad (what pushManager.subscribe eats).
    pub public_b64: String,
    subject: String,
    subs_dir: PathBuf,
    /// Serializes read-modify-write cycles on the subscription files.
    file_lock: Mutex<()>,
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
            file_lock: Mutex::new(()),
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
        let Ok(raw) = fs::read_to_string(self.subs_path(channel)) else {
            return Vec::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save_subs(&self, channel: &str, subs: &[Value]) {
        let path = self.subs_path(channel);
        if subs.is_empty() {
            let _ = fs::remove_file(&path);
            return;
        }
        if let Ok(raw) = serde_json::to_string(subs) {
            let _ = fs::write(&path, raw);
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
        if subscription.get("platform").and_then(Value::as_str) == Some("harmony") {
            let open_id = subscription
                .get("openId")
                .and_then(Value::as_str)
                .ok_or("harmony subscription has no openId")?
                .to_string();
            let _g = self.file_lock.lock().unwrap();
            let mut subs = self.load_subs(channel);
            subs.retain(|s| {
                !(s.get("platform").and_then(Value::as_str) == Some("harmony")
                    && s.get("openId").and_then(Value::as_str) == Some(open_id.as_str()))
            });
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
        if subscription.get("platform").and_then(Value::as_str) == Some("harmony") {
            let open_id = subscription
                .get("openId")
                .and_then(Value::as_str)
                .ok_or("harmony unsubscribe has no openId")?
                .to_string();
            let _g = self.file_lock.lock().unwrap();
            let mut subs = self.load_subs(channel);
            let before = subs.len();
            subs.retain(|s| {
                !(s.get("platform").and_then(Value::as_str) == Some("harmony")
                    && s.get("openId").and_then(Value::as_str) == Some(open_id.as_str()))
            });
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
        let subs = {
            let _g = self.file_lock.lock().unwrap();
            self.load_subs(channel)
        };
        if subs.is_empty() {
            return;
        }
        // Web Push body is JSON; harmony uses the typed payload directly. A
        // failed encode only disables the web leg, not the harmony one.
        let web_body = serde_json::to_vec(payload).ok();
        let client = HyperWebPushClient::new();
        let mut dead: Vec<String> = Vec::new();
        for sub in &subs {
            if Self::is_harmony(sub) {
                // 元服务 account service-notification path. No-op when the
                // channel is disabled (creds absent → harmony is None).
                // Dead-OpenID pruning needs Push Kit's specific failure codes,
                // only observable with real creds — deferred; this just logs.
                let (Some(hp), Some(open_id)) =
                    (harmony, sub.get("openId").and_then(Value::as_str))
                else {
                    continue;
                };
                if let Err(e) = hp.send(open_id, payload).await {
                    log::warn!("harmony push failed on channel {channel}: {e}");
                }
                continue;
            }
            let Some(body) = web_body.as_deref() else { continue };
            let info: SubscriptionInfo = match serde_json::from_value(sub.clone()) {
                Ok(i) => i,
                Err(_) => continue,
            };
            match self.send_one(&client, &info, body).await {
                Ok(()) => {}
                Err(WebPushError::EndpointNotValid(_) | WebPushError::EndpointNotFound(_)) => {
                    dead.push(info.endpoint.clone());
                }
                Err(e) => {
                    log::warn!("web push to {} failed: {e}", info.endpoint);
                }
            }
        }
        if !dead.is_empty() {
            let _g = self.file_lock.lock().unwrap();
            let mut subs = self.load_subs(channel);
            subs.retain(|s| {
                // Never drop harmony subs here — they have no endpoint and the
                // `unwrap_or(false)` below would otherwise evict them all the
                // moment any web endpoint dies.
                if Self::is_harmony(s) {
                    return true;
                }
                s.get("endpoint")
                    .and_then(Value::as_str)
                    .map(|e| !dead.iter().any(|d| d == e))
                    .unwrap_or(false)
            });
            self.save_subs(channel, &subs);
            log::info!("pruned {} dead push subscription(s) on channel {channel}", dead.len());
        }
    }

    async fn send_one(
        &self,
        client: &HyperWebPushClient,
        info: &SubscriptionInfo,
        body: &[u8],
    ) -> Result<(), WebPushError> {
        let mut sig = VapidSignatureBuilder::from_base64(&self.private_b64, info)?;
        sig.add_claim("sub", self.subject.as_str());
        let mut msg = WebPushMessageBuilder::new(info);
        msg.set_payload(ContentEncoding::Aes128Gcm, body);
        msg.set_vapid_signature(sig.build()?);
        client.send(msg.build()?).await
    }
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
}
