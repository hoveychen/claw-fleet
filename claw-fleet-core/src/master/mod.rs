//! Master agent runtime — see PRD §5.7 / TASKS P19.
//!
//! The master is a long-running Claude Code session that owns one task's
//! plan: dispatches workers, runs acceptance audits, calls AskUserQuestion
//! when stuck, and is the only entity authorized to call
//! `fleet task mark-done / mark-failed / update-plan`.
//!
//! This module is **pure data + prompt composition**. Actual subprocess
//! lifecycle (spawn, SIGSTOP, destroy) lives in `crate::supervisor` and
//! `claw-fleet-desktop::local_backend` — they consume `MasterSpawnSpec`.

pub mod event_router;
pub mod runtime;
pub mod system_template;

pub use event_router::{
    dispatchable_changed, format_event, format_user, EventDebouncer, MasterEvent, UserMessage,
};
pub use runtime::{spawn_spec_for_task, MasterSpawnSpec};
pub use system_template::{compose_system_prompt, MASTER_SYSTEM_TEMPLATE};
