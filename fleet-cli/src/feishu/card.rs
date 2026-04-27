//! Card 2.0 builders — thin re-export.
//!
//! The real `GuardCard` / `ElicitationCard` / `PlanCard` types live in
//! [`claw_fleet_core::feishu`] so both `LocalBackend` (desktop) and
//! `fleet serve` (the remote probe HTTP daemon) share the same renderers.
//! This module exists to keep the file layout aligned with
//! `design/feishu-integration.md`.

#[allow(unused_imports)]
pub use claw_fleet_core::feishu::{
    ElicitationCard, ElicitationOptionCard as ElicitationOption, GuardCard, PlanCard,
};
