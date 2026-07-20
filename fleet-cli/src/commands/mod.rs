//! CLI subcommand implementations, split out of `main.rs` by domain.
//!
//! `main.rs` keeps the clap definitions and the top-level dispatch `match`; each
//! module here holds the `cmd_*` handler(s) for one command domain plus their
//! private helpers.

pub(crate) mod account;
pub(crate) mod agents;
pub(crate) mod audit;
pub(crate) mod bootstrap;
pub(crate) mod guard;
pub(crate) mod handoff;
pub(crate) mod loop_cmd;
pub(crate) mod memory;
pub(crate) mod plan;
pub(crate) mod prd;
pub(crate) mod remote;
pub(crate) mod report;
pub(crate) mod schedule;
pub(crate) mod search;
pub(crate) mod serve;
pub(crate) mod session;
pub(crate) mod skill;
pub(crate) mod watch;
pub(crate) mod wiki;
