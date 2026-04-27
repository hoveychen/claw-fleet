//! `/webhook/feishu` handler — verifies `X-Lark-Signature`, dispatches
//! `card.action.trigger` events to the matching `respond_to_*` Backend
//! method.
//!
//! Skeleton: function signatures only.  The real wiring happens in
//! `fleet-cli/src/main.rs::cmd_serve` once the env-driven bridge boots.

use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct CardActionEvent {
    pub decision_id: String,
    /// Discriminator: which Backend method to dispatch to.
    /// One of: "guard", "elicitation", "plan_approval".
    pub kind: String,
    /// Free-form action payload — `allow`/`block`, `decision`,
    /// `answers`, `edited_plan`, `feedback`, etc.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Verify signature + parse + dispatch.  Returns the JSON body to send
/// back to Feishu (200 OK with empty body for `card.action.trigger`).
///
/// # Skeleton TODOs
/// - `X-Lark-Signature` HMAC-SHA256 verification against `FEISHU_ENCRYPT_KEY`.
/// - Branch on `event_type`; only handle `card.action.trigger`.
/// - Lookup `decision_id` against the in-flight Decision-Panel store and
///   call `respond_to_guard` / `respond_to_elicitation` /
///   `respond_to_plan_approval` from `claw_fleet_core::backend::Backend`.
/// - Render the resolved card via `client::update_card`.
#[allow(dead_code)]
pub fn handle_webhook(_signature: &str, _body: &[u8]) -> Result<Vec<u8>, String> {
    Err("feishu webhook handler not implemented yet".into())
}
