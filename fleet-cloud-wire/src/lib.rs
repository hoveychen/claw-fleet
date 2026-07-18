//! Stable wire types shared by the Fleet Cloud control plane and customer Runner.
//!
//! This crate deliberately has no dependency on Axum, SQLx, the desktop app, or
//! `claw-fleet-core`. Transport and persistence code belong at the edges; JSON
//! compatibility belongs here.

pub mod event;
pub mod runner;
pub mod task;

/// First version of the private control-plane ↔ Runner stream protocol.
pub const RUNNER_PROTOCOL_VERSION: u16 = 1;
