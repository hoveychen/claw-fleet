//! [`AgentSource`] for dsh (DeepSeek Harness).
//!
//! Read-only half: list sessions and read their history. Unlike the other two
//! sources, dsh keeps its transcripts in a zstd-compressed private layout under
//! `~/.dsh/sessions` and exposes them only through a running `dsh web` server —
//! so this source talks RPC ([`crate::dsh_client`]) to a server it owns
//! ([`crate::dsh_server`]) rather than reading files.
//!
//! One server covers every workspace: `session.list` returns all sessions the
//! harness home knows about regardless of their cwd, including ones written by
//! a completely different profile (a `dsh --profile headless` run shows up
//! here). The server's own cwd only decides where *new* sessions would root.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::dsh_client::DshClient;
use crate::dsh_events::DshEventWatcher;
use crate::dsh_server::DshServer;
use crate::session::{SessionInfo, SessionStatus};

/// URI scheme for this source's `SessionInfo::jsonl_path`.
pub const DSH_URI_PREFIX: &str = "dsh://";

/// dsh has no filesystem signal Fleet can watch (its logs are compressed and
/// written by a server that owns them), so the session list is polled.
///
/// The `events.mux` / `events.host` downlinks do not replace this poll: they
/// carry a session's *phase*, not its roster or its token totals, and the
/// registry's watch loop is a shared thread no single source may block. So the
/// poll stays the roster refresh and [`DshEventWatcher`] sharpens the status of
/// what it returns.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

pub struct DshSource {
    /// The server is started on first use and reused. `None` until then, or
    /// after a start failure — a machine without dsh installed never pays for
    /// a spawn attempt because [`is_available`] gates the whole source.
    ///
    /// [`is_available`]: AgentSource::is_available
    server: Mutex<Option<DshServer>>,
    /// Follower of the server's two downlinks, rebuilt whenever the server
    /// lands on a new port.
    watcher: Mutex<Option<DshEventWatcher>>,
}

impl Default for DshSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DshSource {
    pub fn new() -> Self {
        Self {
            server: Mutex::new(None),
            watcher: Mutex::new(None),
        }
    }

    /// Run `f` against a live server, starting or restarting it as needed.
    ///
    /// The client is rebuilt per call on purpose: a restart lands on a fresh
    /// OS-assigned port, so a memoized client would silently point at a dead
    /// listener.
    fn with_client<T>(&self, f: impl FnOnce(&DshClient) -> Result<T, String>) -> Result<T, String> {
        let mut guard = self
            .server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match guard.as_mut() {
            Some(server) => server.ensure_alive()?,
            None => {
                let binary = crate::dsh_server::discover().ok_or_else(|| {
                    "dsh is not installed (npm i -g @deepseek-ai/dsh)".to_string()
                })?;
                // Root the server at the harness home rather than a project:
                // observation spans every workspace, and a server rooted in a
                // directory that later disappears would fail to restart.
                let cwd = crate::session::real_home_dir()
                    .ok_or_else(|| "cannot determine home dir".to_string())?;
                *guard = Some(DshServer::start(&binary, &cwd)?);
            }
        }

        let server = guard.as_ref().expect("server started above");
        self.ensure_watcher(server.port());
        let client = server.client()?;
        f(&client)
    }

    /// Point the downlink follower at `port`, replacing one that is following a
    /// stale port.
    ///
    /// Called from inside [`with_client`], so the lock order is always
    /// server-then-watcher; nothing takes them the other way round.
    ///
    /// [`with_client`]: Self::with_client
    fn ensure_watcher(&self, port: u16) {
        let mut guard = self
            .watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_ref().is_some_and(|w| w.port() == port) {
            return;
        }
        // Assigning drops the old watcher, which stops its follower thread. Its
        // live view goes with it: those phases belong to sessions as seen by a
        // server that no longer exists.
        *guard = Some(DshEventWatcher::start(port));
    }

    /// The port Fleet's `dsh web` instance is listening on, or `None` before
    /// the first RPC starts it. Diagnostics, and the handle tests need to drive
    /// a turn through the same server this source observes.
    pub fn server_port(&self) -> Option<u16> {
        self.server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(DshServer::port)
    }

    /// The phase the downlinks report for `session_id`, if any is fresh.
    fn live_phase(&self, session_id: &str) -> Option<SessionStatus> {
        self.watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|w| w.phase_of(session_id))
    }

    /// Strip the `dsh://` scheme off a session URI.
    fn session_id_of(path: &str) -> Option<&str> {
        path.strip_prefix(DSH_URI_PREFIX).filter(|s| !s.is_empty())
    }
}

/// Read a token count out of a `session.list` projections block.
fn projection_u64(projections: &Value, key: &str, field: &str) -> u64 {
    projections
        .get("values")
        .and_then(|v| v.get(key))
        .and_then(|v| v.get(field))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// Map one `session.list` item onto Fleet's [`SessionInfo`].
///
/// dsh hands over its own projections (title, token usage, context breakdown)
/// already folded, so this is a field rename rather than a transcript parse.
/// Everything Fleet derives from a transcript it does not have here (entrypoint,
/// pid liveness, todos, skills) stays at its default.
fn session_info_from_list_item(item: &Value) -> Option<SessionInfo> {
    let id = item.get("sessionId").and_then(Value::as_str)?.to_string();
    let workspace_path = item
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let updated_at = item.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
    let running = item
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let projections = item
        .get("projections")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let title = projections
        .get("values")
        .and_then(|v| v.get("title"))
        .and_then(Value::as_str)
        .map(str::to_string);

    // dsh reports the four usage buckets separately; Fleet's "input" total is
    // everything sent to the API, cache re-reads included — the same口径 as the
    // other two sources.
    let output = projection_u64(&projections, "tokenUsage", "outputTokens");
    let input = projection_u64(&projections, "tokenUsage", "uncachedInputTokens")
        + projection_u64(&projections, "tokenUsage", "cacheReadTokens")
        + projection_u64(&projections, "tokenUsage", "cacheWriteTokens");

    Some(SessionInfo {
        id: id.clone(),
        workspace_name: crate::session::workspace_name(&workspace_path),
        workspace_path,
        ai_title: title,
        // `running` is the only liveness bit `session.list` carries. Finer
        // phases (Thinking / Streaming / Executing / Processing) come off the
        // mux downlink and are overlaid by `scan_sessions`.
        status: if running {
            SessionStatus::Active
        } else {
            SessionStatus::Idle
        },
        total_output_tokens: output,
        total_input_tokens: input,
        last_activity_ms: updated_at,
        agent_last_activity_ms: updated_at,
        created_at_ms: updated_at,
        jsonl_path: format!("{DSH_URI_PREFIX}{id}"),
        agent_source: "dsh".to_string(),
        ..Default::default()
    })
}

impl AgentSource for DshSource {
    fn name(&self) -> &'static str {
        "dsh"
    }

    fn uri_prefix(&self) -> &'static str {
        DSH_URI_PREFIX
    }

    fn is_available(&self) -> bool {
        crate::dsh_server::is_available()
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        let listed =
            self.with_client(|client| client.call("session.list", json!({})).map_err(Into::into));

        match listed {
            Ok(value) => value
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(session_info_from_list_item)
                        .map(|mut info| {
                            // The poll only knows running/not-running; the
                            // downlinks know which phase of the turn it is in.
                            if let Some(phase) = self.live_phase(&info.id) {
                                info.status = phase;
                            }
                            info
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(e) => {
                crate::log_debug(&format!("dsh scan_sessions: {e}"));
                Vec::new()
            }
        }
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let id = Self::session_id_of(path).ok_or_else(|| format!("invalid dsh URI: {path}"))?;
        // `maxMessages` is absent, so this is the tail page: dsh pages by whole
        // append-origin messages and the tail additionally carries the
        // in-flight partial.
        let value = self.with_client(|client| {
            client
                .call("session.history", json!({ "sessionId": id }))
                .map_err(Into::into)
        })?;

        Ok(history_events(&value))
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let id = Self::session_id_of(path).ok_or_else(|| format!("invalid dsh URI: {path}"))?;
        let value = self.with_client(|client| {
            client
                .call(
                    "session.history",
                    json!({ "sessionId": id, "maxMessages": n }),
                )
                .map_err(Into::into)
        })?;

        Ok(history_events(&value))
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Poll(POLL_INTERVAL)
    }

    fn owns_path(&self, path: &str) -> bool {
        path.starts_with(DSH_URI_PREFIX)
    }

    /// dsh sessions have no Fleet-readable file: the transcript is a
    /// zstd-framed log the server owns. Nothing may hand out a path for it.
    fn resolve_file_path(&self, _path: &str) -> Option<PathBuf> {
        None
    }
}

/// Pull the raw `SessionEvent`s out of a `session.history` answer.
///
/// Each entry pairs the durable event with an optional host-computed tool view;
/// only the event is durable, so that is what Fleet keeps.
fn history_events(value: &Value) -> Vec<Value> {
    value
        .get("events")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("event").cloned())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape of one `session.list` item observed live.
    fn live_list_item() -> Value {
        json!({
            "sessionId": "session-f8d1103c-db06-45c9-9902-6dc522f0f0ee",
            "updatedAt": 1786739335270u64,
            "running": false,
            "blank": false,
            "cwd": "/tmp",
            "agentPreset": "standard",
            "projections": {
                "asOfSeq": 15,
                "values": {
                    "title": "say hi",
                    "sessionStats": { "turns": 1, "steps": 1 },
                    "tokenUsage": {
                        "uncachedInputTokens": 100,
                        "outputTokens": 20,
                        "cacheReadTokens": 5,
                        "cacheWriteTokens": 2
                    },
                    "contextPressure": { "contextWindow": 1000000 }
                }
            }
        })
    }

    #[test]
    fn maps_a_live_list_item() {
        let info = session_info_from_list_item(&live_list_item()).expect("mapped");
        assert_eq!(info.id, "session-f8d1103c-db06-45c9-9902-6dc522f0f0ee");
        assert_eq!(info.workspace_path, "/tmp");
        assert_eq!(info.ai_title.as_deref(), Some("say hi"));
        assert_eq!(info.agent_source, "dsh");
        assert_eq!(
            info.jsonl_path,
            "dsh://session-f8d1103c-db06-45c9-9902-6dc522f0f0ee"
        );
        assert_eq!(info.last_activity_ms, 1786739335270);
    }

    #[test]
    fn sums_every_input_bucket_into_total_input() {
        let info = session_info_from_list_item(&live_list_item()).expect("mapped");
        // uncached 100 + cacheRead 5 + cacheWrite 2 — cache re-reads included,
        // matching what the other sources report as "tokens sent".
        assert_eq!(info.total_input_tokens, 107);
        assert_eq!(info.total_output_tokens, 20);
    }

    #[test]
    fn running_flag_maps_to_active() {
        let mut item = live_list_item();
        item["running"] = json!(true);
        let info = session_info_from_list_item(&item).expect("mapped");
        assert!(matches!(info.status, SessionStatus::Active));
    }

    #[test]
    fn tolerates_a_session_with_no_projections() {
        // A blank session that never ran carries no projections block.
        let item = json!({
            "sessionId": "session-blank",
            "updatedAt": 1u64,
            "running": false,
            "cwd": "/tmp"
        });
        let info = session_info_from_list_item(&item).expect("mapped");
        assert_eq!(info.ai_title, None);
        assert_eq!(info.total_input_tokens, 0);
    }

    #[test]
    fn rejects_an_item_without_a_session_id() {
        assert!(session_info_from_list_item(&json!({ "cwd": "/tmp" })).is_none());
    }

    #[test]
    fn history_events_keeps_the_durable_event_only() {
        // Entries pair the durable event with a transient host-computed view.
        let value = json!({
            "events": [
                { "event": { "type": "user/message", "seq": 7 }, "view": { "for": "call" } },
                { "event": { "type": "turn/end", "seq": 8 } }
            ],
            "hasMore": false
        });
        let events = history_events(&value);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "user/message");
        assert!(events[0].get("view").is_none(), "the view is not durable");
    }

    #[test]
    fn history_events_on_an_empty_answer() {
        assert!(history_events(&json!({})).is_empty());
    }

    #[test]
    fn owns_only_its_own_scheme() {
        let source = DshSource::new();
        assert!(source.owns_path("dsh://session-abc"));
        assert!(!source.owns_path("codex://rollout-abc"));
        assert!(!source.owns_path("/Users/me/.claude/projects/x/y.jsonl"));
    }

    #[test]
    fn session_id_needs_a_non_empty_tail() {
        assert_eq!(
            DshSource::session_id_of("dsh://session-abc"),
            Some("session-abc")
        );
        assert_eq!(DshSource::session_id_of("dsh://"), None);
        assert_eq!(DshSource::session_id_of("session-abc"), None);
    }

    #[test]
    fn dsh_sessions_expose_no_file_path() {
        // The transcript is a zstd-framed log the server owns; handing out a
        // path would invite a reader that cannot parse it.
        assert_eq!(
            DshSource::new().resolve_file_path("dsh://session-abc"),
            None
        );
    }
}
