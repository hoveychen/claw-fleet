//! Feishu (Lark) bridge — fleet-cli–side glue.
//!
//! The actual OAuth state machine, env probe, and the public entry points
//! (`start_oauth`, `poll_oauth`, `status`, `disconnect`) live in
//! [`claw_fleet_core::feishu`] so that `LocalBackend` (in-process) and the
//! `fleet serve` HTTP daemon drive the same store.
//!
//! This module retains design-intent submodules (`oauth`, `client`, `bot`,
//! `card`) where the listener server, Card 2.0 builders, and webhook
//! handler will land — see `design/feishu-integration.md`.

pub mod bot;
pub mod card;
pub mod client;
pub mod oauth;

pub use claw_fleet_core::feishu::{disconnect, poll_oauth, start_oauth, status};
