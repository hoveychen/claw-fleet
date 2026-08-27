//! Agent Client Protocol (ACP) — Fleet's public agent surface.
//!
//! ACP is the open standard for "any client drives any coding agent"
//! (<https://agentclientprotocol.com>, Zed Industries, JSON-RPC 2.0). Fleet
//! implements the **agent** side, so existing ACP clients — Zed, JetBrains, VS
//! Code, Neovim, Emacs, Obsidian, acp-ui's desktop/web/Android builds — can
//! drive a Fleet workspace without anyone writing a Fleet-specific client.
//!
//! # Why this replaces the OpenAI-Responses surface
//!
//! `hooks_server::responses` maps Fleet onto OpenAI's Responses API. That shape
//! is a *stateful LLM* contract, not an *agent harness* one, and two things
//! Fleet does natively have nowhere to live in it:
//!
//! - **Tool traces.** The Responses projection drops every `tool_use` block and
//!   emits only assistant text, because OpenAI's output items have no vocabulary
//!   for "the agent read this file, then ran this command". ACP has
//!   `tool_call` / `tool_call_update` with `kind`, `status`, `locations` and
//!   `diff` content — a first-class trace.
//! - **Waiting for a human.** Responses has no interrupted state, so Fleet's six
//!   decision cards had to borrow the `function_call` slot, which properly means
//!   "client, run this function for me". ACP has two purpose-built channels:
//!   `session/request_permission` (approve a tool) and `elicitation/create` (ask
//!   a question, with a form schema).
//!
//! # Layout
//!
//! - [`jsonrpc`] — full-duplex JSON-RPC 2.0. Transport-agnostic on purpose: the
//!   same protocol code serves the `fleet acp` stdio subcommand (what a desktop
//!   editor spawns) and the `/acp` WebSocket endpoint (what mobile/web clients
//!   dial). ACP v1's `transports.mdx` blesses this explicitly under "Custom
//!   Transports" — any bidirectional channel is conformant so long as the
//!   JSON-RPC format and lifecycle are preserved.

//! - [`types`] — the v1 wire types, spelled from the published schema.
//! - [`agent`] — method dispatch onto Fleet's spawn/resume machinery.

pub mod agent;
pub mod attachments;
pub mod conn;
pub mod decisions;
pub mod jsonrpc;
pub mod stdio;
pub mod tools;
pub mod types;
pub mod watcher;
pub mod ws;
