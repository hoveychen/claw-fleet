//! IPC protocol between the `fleet mcp` server (a Claude Code child process)
//! and the local `fleet` process for the `fleet__render_a2ui` tool.
//!
//! Wire format: file-based IPC at `~/.fleet/fleet-render-a2ui/<id>.json`
//! (request) and `~/.fleet/fleet-render-a2ui/<id>.response.json` (response).
//! Direct mirror of [`crate::mcp_ipc`] (fleet__ask) so the desktop's watcher,
//! orphan filter and cleanup can stay parallel structures.
//!
//! These types are the single source of truth for the `fleet__render_a2ui`
//! MCP schema; [`a2ui_render_input_schema`] mirrors the request shape in
//! JSONSchema form for the MCP `tools/list` response. `message_tree` is left
//! as a passthrough `serde_json::Value` — the A2UI v0.9 schema is large and
//! evolving upstream, so we defer validation to the renderer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct A2uiRenderRequest {
    pub id: String,
    /// Originating Claude Code session id (same role as `FleetAskRequest::session_id`).
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    /// The task this card belongs to — stamped by `pending_decisions::decorate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<crate::task_model::DecisionTaskRef>,
    #[serde(default)]
    pub timestamp: String,
    /// Timed out and parked — see [`crate::mcp_ipc::FleetAskRequest::parked`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
    /// The A2UI v0.9 agent-to-client message tree, passed through verbatim
    /// to `@a2ui/web_core`'s `MessageProcessor` on the desktop side.
    #[cfg_attr(feature = "ts-export", ts(type = "unknown"))]
    pub message_tree: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2uiRenderResponse {
    pub id: String,
    /// `userAction.name` from the rendered surface — `None` when the user
    /// closed the card without firing an Action component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_name: Option<String>,
    /// Flattened `userAction.context` after BoundValue resolution. Non-string
    /// values are stringified by the frontend so the wire stays as a
    /// `BTreeMap<String, String>` — same shape `FleetAskResponse::answers`
    /// uses, so agents that learned one tool's wire can read both.
    #[serde(default)]
    pub action_context: BTreeMap<String, String>,
    #[serde(default)]
    pub cancelled: bool,
}

// ── File-based IPC (mirror of `crate::mcp_ipc`) ──────────────────────────────

fn fleet_a2ui_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-render-a2ui"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    fleet_a2ui_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    fleet_a2ui_dir().map(|d| d.join(format!("{id}.response.json")))
}

/// Write a fleet__render_a2ui request file. Called by the MCP server when it
/// handles a `tools/call` for `fleet__render_a2ui`.
pub fn write_request(req: &A2uiRenderRequest) -> Result<(), String> {
    let dir = fleet_a2ui_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create fleet-render-a2ui dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-render-a2ui request: {e}"))
}

/// Read a pending request file. Called by the desktop watcher when it
/// notices a new id in the directory listing.
pub fn read_request(id: &str) -> Option<A2uiRenderRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Non-blocking response read.
pub fn try_read_response(id: &str) -> Option<A2uiRenderResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<A2uiRenderResponse>(&content).ok()
}

/// Write a response file. Called by the desktop / fleet serve after the user
/// resolves the rendered A2UI surface.
///
/// Same contract as [`crate::mcp_ipc::write_response`]: the existence check and
/// the write share [`withdraw`]'s lock, and an answer whose producer is gone
/// resumes the Fleet-owned session ([`recover_orphaned_response`]). When the
/// session is still running but the card stepped aside for a message, nobody
/// polls this file, so the live turn is told to fetch it.
pub fn write_response(resp: &A2uiRenderResponse) -> Result<(), String> {
    let req_path = request_path(&resp.id).ok_or("cannot determine home dir")?;
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    // Only a live pending request may be answered — see guard::write_response.
    crate::atomic_json::with_file_lock(&req_path, || {
        if !req_path.exists() {
            return Err(format!("no pending request for id {}", resp.id));
        }
        fs::write(&path, json).map_err(|e| format!("write fleet-render-a2ui response: {e}"))
    })?;
    if !recover_orphaned_response(&resp.id)? {
        crate::ask_preempt::notify_resolved_if_detached(&resp.id);
    }
    Ok(())
}

/// How long ago the response for `id` was written, or `None` when there is
/// none. `ask_preempt::sweep_idle` waits this out before it takes over.
pub fn response_age_ms(id: &str) -> Option<u64> {
    let modified = fs::metadata(response_path(id)?).ok()?.modified().ok()?;
    Some(
        std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_millis() as u64,
    )
}

/// Write an answered response for `id` (action `submit`, context `k=v`),
/// back-dated by `age_ms`.
#[cfg(test)]
pub(crate) fn write_response_file_for_test(id: &str, age_ms: u64) {
    let path = response_path(id).unwrap();
    fs::write(&path, r#"{"id":"x","actionName":"submit","actionContext":{"k":"v"}}"#).unwrap();
    let f = fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_millis(age_ms))
        .unwrap();
}

/// Recover an answered request whose MCP producer no longer exists — the
/// `fleet__render_a2ui` twin of [`crate::mcp_ipc::recover_orphaned_response`].
///
/// `Ok(false)`: the live producer (or the live turn of a detached card) still
/// owns the response, or the session is not Fleet-owned. `Ok(true)`: consumed
/// here — a cancellation is cleaned up, an action resumes the session through
/// the parked path.
pub fn recover_orphaned_response(id: &str) -> Result<bool, String> {
    // See `mcp_ipc::recover_orphaned_response`: a detached card's session may be
    // in the first seconds of the resume that woke it.
    let detached = crate::ask_preempt::was_detached(id);
    recover_orphaned_response_with(
        id,
        |session_id| {
            crate::parked::session_alive(session_id)
                || (detached && crate::session_driver::turn_in_flight(session_id))
        },
        crate::parked::resume_session,
    )
}

fn recover_orphaned_response_with<A, S>(id: &str, alive: A, spawn: S) -> Result<bool, String>
where
    A: FnOnce(&str) -> bool,
    S: FnOnce(&str, &str, &str, Option<&str>, Option<&str>, Option<&str>) -> Result<(), String>,
{
    let Some(req) = read_request(id) else {
        return Ok(false);
    };
    let Some(resp) = try_read_response(id) else {
        return Ok(false);
    };
    if alive(&req.session_id) {
        return Ok(false);
    }
    let Some(workspace) = crate::parked::parkable_workspace(&req.session_id) else {
        return Ok(false);
    };

    if resp.cancelled {
        cleanup(id);
    } else {
        let stepped_aside = crate::ask_preempt::was_detached(id);
        let model = crate::session::resolve_session_model_spec(&req.session_id);
        let effort = crate::launch_spec::effort_of(&req.session_id);
        crate::parked::park_with(
            id,
            crate::parked::ParkedKind::A2uiRender,
            &req.session_id,
            &workspace,
            &req,
            model.as_deref(),
            effort.as_deref(),
        )?;
        if stepped_aside {
            crate::parked::mark_stepped_aside(id)?;
        }
        cleanup(id);
        let payload = serde_json::to_value(&resp)
            .map_err(|e| format!("serialize orphaned fleet-render-a2ui response: {e}"))?;
        crate::parked::answer_with(id, &payload, spawn)?;
    }
    crate::log_debug(&format!(
        "fleet-render-a2ui orphan recovery: consumed {id} for session {} (cancelled={})",
        req.session_id, resp.cancelled
    ));
    Ok(true)
}

/// Remove the request + response pair (and any detached-card record). Called
/// by the MCP server once it has consumed the response.
pub fn cleanup(id: &str) {
    if let Some(p) = request_path(id) {
        let _ = fs::remove_file(lock_path(&p));
        let _ = fs::remove_file(p);
    }
    if let Some(p) = response_path(id) {
        let _ = fs::remove_file(p);
    }
    crate::ask_preempt::clear_detached(id);
}

/// The sibling file [`crate::atomic_json::with_file_lock`] locks for `path`.
fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// What [`withdraw`] found.
#[derive(Debug)]
pub enum WithdrawOutcome {
    /// The card is closed: request and any detached record are gone.
    Withdrawn,
    /// The boss acted first. The card is left open with its response, for the
    /// caller to consume like any answer.
    Answered(A2uiRenderResponse),
    /// No open card with this id.
    Gone,
}

/// Close a card unless a response is already in, atomically with respect to
/// [`write_response`] — same protocol as [`crate::mcp_ipc::withdraw`].
pub fn withdraw(id: &str) -> WithdrawOutcome {
    let Some(req_path) = request_path(id) else {
        return WithdrawOutcome::Gone;
    };
    let claimed = req_path.with_extension("withdrawing");
    let outcome = crate::atomic_json::with_file_lock(&req_path, || {
        if fs::rename(&req_path, &claimed).is_err() {
            return WithdrawOutcome::Gone;
        }
        if let Some(resp) = try_read_response(id) {
            let _ = fs::rename(&claimed, &req_path);
            return WithdrawOutcome::Answered(resp);
        }
        let _ = fs::remove_file(&claimed);
        WithdrawOutcome::Withdrawn
    });
    if matches!(outcome, WithdrawOutcome::Withdrawn) {
        cleanup(id);
    }
    outcome
}

/// Soft list of pending request ids; returns empty on any failure.
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict list — same Ok/Err contract as
/// [`crate::mcp_ipc::list_pending_requests_checked`].
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = fleet_a2ui_dir() else {
        return Ok(Vec::new());
    };
    list_pending_in_dir(&dir)
}

fn list_pending_in_dir(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut request_ids: Vec<String> = Vec::new();
    let mut response_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".response.json") {
            response_ids.insert(id.to_string());
        } else if let Some(id) = name.strip_suffix(".json") {
            request_ids.push(id.to_string());
        }
    }
    Ok(request_ids
        .into_iter()
        .filter(|id| !response_ids.contains(id))
        .collect())
}

/// JSONSchema describing `fleet__render_a2ui` arguments. The MCP `tools/list`
/// response embeds this verbatim so the agent sees the same shape the Rust
/// types deserialize. `messageTree` is checked in code rather than listed as
/// required, so `wait` / `withdraw` can omit it (same as `fleet__ask`'s
/// `questions`).
pub fn a2ui_render_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["render", "wait", "withdraw"],
                "description": "Default `render` (raise a card; `messageTree` required). A card steps aside when a new message reaches your turn and comes back with its request id: `wait` resumes waiting on it (returns at once if the boss already acted), `withdraw` takes it off the boss's screen when the message made it obsolete. Both need `request_id`."
            },
            "request_id": {
                "type": "string",
                "description": "For action `wait` / `withdraw`: the id of the card that stepped aside."
            },
            "messageTree": {
                "type": "object",
                "description": "A2UI v0.9 agent-to-client message tree (passed through to @a2ui/web_core MessageProcessor). See https://github.com/google/A2UI/tree/main/specification/v0_9 for the full schema. Required for action `render`."
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fresh_tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-render-a2ui-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn request_round_trips_camel_case_with_message_tree() {
        let req = A2uiRenderRequest {
            parked: false,
            id: "abc".into(),
            session_id: "sess-1".into(),
            workspace_name: "claude-fleet".into(),
            ai_title: Some("Implement P3".into()),
            task: None,
            timestamp: "2026-05-26T12:00:00Z".into(),
            message_tree: json!({
                "surfaceUpdate": {
                    "surfaceId": "demo",
                    "root": { "Card": { "id": "root", "children": [] } }
                }
            }),
        };
        let s = serde_json::to_string(&req).unwrap();
        // camelCase on the wire — matches fleet__ask
        assert!(s.contains("\"sessionId\""));
        assert!(s.contains("\"workspaceName\""));
        assert!(s.contains("\"aiTitle\""));
        assert!(s.contains("\"messageTree\""));
        let back: A2uiRenderRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.session_id, "sess-1");
        assert_eq!(back.ai_title.as_deref(), Some("Implement P3"));
        // message_tree survives round-trip as a Value, no schema coercion
        assert!(back.message_tree["surfaceUpdate"]["surfaceId"] == "demo");
    }

    #[test]
    fn response_round_trips_action_payload() {
        let mut ctx = BTreeMap::new();
        ctx.insert("level".into(), "75".into());
        ctx.insert("date".into(), "2026-05-26".into());
        let resp = A2uiRenderResponse {
            id: "abc".into(),
            action_name: Some("submit".into()),
            action_context: ctx,
            cancelled: false,
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"actionName\""));
        assert!(s.contains("\"actionContext\""));
        let back: A2uiRenderResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.action_name.as_deref(), Some("submit"));
        assert_eq!(back.action_context.get("level").unwrap(), "75");
        assert!(!back.cancelled);
    }

    #[test]
    fn response_round_trips_cancelled() {
        let resp = A2uiRenderResponse {
            id: "k".into(),
            action_name: None,
            action_context: BTreeMap::new(),
            cancelled: true,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: A2uiRenderResponse = serde_json::from_str(&s).unwrap();
        assert!(back.cancelled);
        assert!(back.action_name.is_none());
        // action_name omitted when None thanks to skip_serializing_if
        assert!(!s.contains("\"actionName\""));
    }

    #[test]
    fn parses_request_with_nested_message_tree() {
        // Verifies the messageTree passthrough survives complex nesting —
        // anything @a2ui/web_core's schema accepts must round-trip without
        // schema knowledge baked into Fleet core.
        let raw = json!({
            "id": "r1",
            "messageTree": {
                "surfaceUpdate": {
                    "surfaceId": "form",
                    "root": {
                        "Card": {
                            "id": "card",
                            "children": [
                                { "TextField": { "id": "name", "label": "Name" } },
                                { "Slider": { "id": "n", "min": 0, "max": 100, "value": 50 } },
                                { "Button": { "id": "ok", "label": "OK", "action": { "name": "submit" } } }
                            ]
                        }
                    }
                }
            }
        });
        let req: A2uiRenderRequest = serde_json::from_value(raw).unwrap();
        let children = &req.message_tree["surfaceUpdate"]["root"]["Card"]["children"];
        assert_eq!(children.as_array().map(|a| a.len()), Some(3));
        assert_eq!(children[2]["Button"]["action"]["name"], "submit");
    }

    #[test]
    fn list_pending_in_dir_missing_returns_ok_empty() {
        let dir = fresh_tmp_dir("missing");
        assert!(!dir.exists());
        let result = list_pending_in_dir(&dir);
        assert!(matches!(&result, Ok(v) if v.is_empty()), "got {result:?}");
    }

    #[test]
    fn list_pending_in_dir_filters_responses() {
        let dir = fresh_tmp_dir("filter");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("req-a.json"), "{}").unwrap();
        std::fs::write(dir.join("req-b.json"), "{}").unwrap();
        std::fs::write(dir.join("req-a.response.json"), "{}").unwrap();
        std::fs::write(dir.join("ignored.txt"), "ignore").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert_eq!(ids, vec!["req-b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_pending_in_dir_excludes_already_answered() {
        let dir = fresh_tmp_dir("answered");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("answered.json"), "{}").unwrap();
        std::fs::write(dir.join("answered.response.json"), "{}").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert!(ids.is_empty(), "answered orphan leaked: {ids:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Private `FLEET_HOME` / `CODEX_HOME`, restored on drop. A Codex rollout
    /// is the cheapest way to make a session Fleet-owned for the recovery gate.
    struct TmpHome {
        dir: std::path::PathBuf,
        previous_fleet: Option<std::ffi::OsString>,
        previous_codex: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = fresh_tmp_dir(tag);
            std::fs::create_dir_all(&dir).unwrap();
            let previous_fleet = std::env::var_os("FLEET_HOME");
            let previous_codex = std::env::var_os("CODEX_HOME");
            unsafe {
                std::env::set_var("FLEET_HOME", &dir);
                std::env::set_var("CODEX_HOME", dir.join(".codex"));
            }
            Self { dir, previous_fleet, previous_codex, _lock: lock }
        }

        fn plant_codex_session(&self, id: &str, workspace: &std::path::Path) {
            let sessions = self.dir.join(".codex/sessions/2026/11/04");
            std::fs::create_dir_all(&sessions).unwrap();
            let record = json!({
                "type": "session_meta",
                "payload": { "id": id, "session_id": id, "originator": "fleet", "cwd": workspace, "source": "exec" }
            });
            std::fs::write(
                sessions.join(format!("rollout-2026-11-04T00-00-00-{id}.jsonl")),
                format!("{record}\n"),
            )
            .unwrap();
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.previous_fleet.take() {
                    Some(v) => std::env::set_var("FLEET_HOME", v),
                    None => std::env::remove_var("FLEET_HOME"),
                }
                match self.previous_codex.take() {
                    Some(v) => std::env::set_var("CODEX_HOME", v),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn open_request(id: &str, session_id: &str) {
        write_request(&A2uiRenderRequest {
            id: id.into(),
            session_id: session_id.into(),
            workspace_name: String::new(),
            ai_title: None,
            task: None,
            timestamp: "2026-11-04T10:00:00Z".into(),
            parked: false,
            message_tree: json!({}),
        })
        .unwrap();
    }

    fn submitted(id: &str) -> A2uiRenderResponse {
        A2uiRenderResponse {
            id: id.into(),
            action_name: Some("submit".into()),
            action_context: BTreeMap::from([("k".into(), "v".into())]),
            cancelled: false,
        }
    }

    /// Recover `id` (already planted with a response) and return the resume prompt.
    fn recover_prompt(id: &str) -> String {
        let mut prompt = None;
        assert!(recover_orphaned_response_with(
            id,
            |_| false,
            |_, _, p, _, _, _| {
                prompt = Some(p.to_string());
                Ok(())
            },
        )
        .unwrap());
        prompt.expect("orphan must resume")
    }

    #[test]
    fn detached_orphan_resumes_with_the_stepped_aside_header_and_the_action() {
        let home = TmpHome::new("orphan-detached");
        let workspace = home.dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session_id = "019f6aba-6dd8-7c90-a70e-c969ea896be1";
        home.plant_codex_session(session_id, &workspace);
        let plant = |id: &str| {
            open_request(id, session_id);
            std::fs::write(response_path(id).unwrap(), serde_json::to_vec(&submitted(id)).unwrap())
                .unwrap();
        };

        // A card that timed out keeps the timeout wording.
        plant("plain-1");
        let prompt = recover_prompt("plain-1");
        assert!(prompt.starts_with(crate::parked::RESUME_PROMPT_HEADER), "{prompt}");

        // Stepped aside, turn ended on its own: answered after that.
        plant("aside-1");
        crate::ask_preempt::mark_detached(crate::ask_preempt::CardTool::A2uiRender, "aside-1", session_id);
        let prompt = recover_prompt("aside-1");
        assert!(prompt.starts_with(crate::parked::STEPPED_ASIDE_RESUME_HEADER), "{prompt}");
        assert!(prompt.contains("【A2UI 操作】submit"), "{prompt}");
        assert!(prompt.contains("【k】v"), "{prompt}");
        assert!(!crate::ask_preempt::is_detached("aside-1"));
        assert!(read_request("aside-1").is_none());
        assert!(try_read_response("aside-1").is_none());
        assert!(!crate::parked::is_parked("aside-1"), "answered, so the parked copy is gone too");

        // Same through the sweep, which holds the record under its claim name.
        plant("aside-2");
        crate::ask_preempt::mark_detached(crate::ask_preempt::CardTool::A2uiRender, "aside-2", session_id);
        let dir = home.dir.join(".fleet/fleet-ask-detached");
        std::fs::rename(dir.join("aside-2.json"), dir.join("aside-2.sweeping")).unwrap();
        let prompt = recover_prompt("aside-2");
        assert!(prompt.starts_with(crate::parked::STEPPED_ASIDE_RESUME_HEADER), "{prompt}");
    }

    #[test]
    fn live_session_keeps_the_response_and_cancellation_resumes_nothing() {
        let home = TmpHome::new("orphan-live");
        let workspace = home.dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session_id = "019f6aba-6dd8-7c90-a70e-c969ea896be2";
        home.plant_codex_session(session_id, &workspace);

        open_request("live-1", session_id);
        std::fs::write(response_path("live-1").unwrap(), serde_json::to_vec(&submitted("live-1")).unwrap())
            .unwrap();
        let recovered =
            recover_orphaned_response_with("live-1", |_| true, |_, _, _, _, _, _| panic!("live turn owns it"))
                .unwrap();
        assert!(!recovered);
        assert!(try_read_response("live-1").is_some());

        open_request("cancel-1", session_id);
        let mut cancel = submitted("cancel-1");
        cancel.cancelled = true;
        std::fs::write(response_path("cancel-1").unwrap(), serde_json::to_vec(&cancel).unwrap()).unwrap();
        assert!(recover_orphaned_response_with(
            "cancel-1",
            |_| false,
            |_, _, _, _, _, _| panic!("a dismissal wakes nobody")
        )
        .unwrap());
        assert!(read_request("cancel-1").is_none());
    }

    #[test]
    fn withdraw_returns_a_response_that_is_already_in() {
        let _home = TmpHome::new("withdraw-answered");
        open_request("w-ans", "s1");
        write_response_file_for_test("w-ans", 0);
        match withdraw("w-ans") {
            WithdrawOutcome::Answered(resp) => assert_eq!(resp.action_name.as_deref(), Some("submit")),
            other => panic!("expected the response, got {other:?}"),
        }
        assert!(read_request("w-ans").is_some(), "the card stays open for the caller to consume");
        assert!(try_read_response("w-ans").is_some());
        assert!(matches!(withdraw("w-missing"), WithdrawOutcome::Gone));
    }

    #[test]
    fn response_after_withdraw_is_refused_not_dropped() {
        let home = TmpHome::new("withdraw-then-answer");
        open_request("w-1", "s1");
        crate::ask_preempt::mark_detached(crate::ask_preempt::CardTool::A2uiRender, "w-1", "s1");
        assert!(matches!(withdraw("w-1"), WithdrawOutcome::Withdrawn));
        assert!(read_request("w-1").is_none());
        assert!(!crate::ask_preempt::is_detached("w-1"));
        let err = write_response(&submitted("w-1")).unwrap_err();
        assert!(err.contains("no pending request"), "{err}");
        assert!(try_read_response("w-1").is_none());
        // No claim file or lock is left that a listing could mistake for a card.
        let names: Vec<String> = std::fs::read_dir(home.dir.join(".fleet/fleet-render-a2ui"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("w-1") && !n.ends_with(".lock"))
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn write_response_to_a_detached_card_of_a_dead_unowned_session_keeps_it() {
        // Not Fleet-owned, so neither recovery nor the notice can act: the
        // response stays for a later `wait`.
        let _home = TmpHome::new("write-detached");
        open_request("d-1", "s-unowned");
        crate::ask_preempt::mark_detached(crate::ask_preempt::CardTool::A2uiRender, "d-1", "s-unowned");
        write_response(&submitted("d-1")).unwrap();
        assert_eq!(try_read_response("d-1").unwrap().action_name.as_deref(), Some("submit"));
        assert!(crate::ask_preempt::is_detached("d-1"));
    }

    #[test]
    fn input_schema_advertises_message_tree_and_wait_withdraw() {
        let s = a2ui_render_input_schema();
        assert_eq!(s["type"], "object");
        assert!(s["properties"]["messageTree"].get("type").is_some());
        assert_eq!(s["properties"]["action"]["enum"], json!(["render", "wait", "withdraw"]));
        assert!(s["properties"]["request_id"].is_object());
        assert!(
            s.get("required").is_none(),
            "`messageTree` is checked in code so wait/withdraw can omit it"
        );
    }
}
