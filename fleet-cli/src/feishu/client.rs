//! Feishu Open Platform HTTP client — thin re-export.
//!
//! The real `tenant_access_token` cache and the `send_card` /
//! `update_card` / `urgent_app` calls live in [`claw_fleet_core::feishu`]
//! so both `LocalBackend` and `fleet serve` share the same in-process
//! state.  This module exists to keep the file layout aligned with
//! `design/feishu-integration.md`.

#[allow(unused_imports)]
pub use claw_fleet_core::feishu::{send_card, update_card, urgent_app};
