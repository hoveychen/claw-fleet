//! Queued follow-up messages for a session that is still mid-turn.
//!
//! Fleet sessions are one-shot headless `claude -p` processes: each user turn is
//! a fresh `claude --resume <id> -p "<text>"` that runs to completion and exits
//! (see [`crate::session_launch`]). While that process is alive there is no live
//! stdin to type into, and firing a *second* `claude --resume` on the same
//! transcript is how you corrupt a session (see [`crate::parked::answer_with`]).
//! So a follow-up typed while the turn is running cannot be delivered
//! immediately.
//!
//! This module is the "queue it, deliver it when the turn ends" layer. A message
//! enqueued for a running session is written to
//! `~/.fleet/pending-messages/<session_id>.json`; the desktop's session-refresh
//! tick calls [`drain_if_idle`] for every session, and the moment a session's
//! turn is over (`proc_alive == false`, no live `claude` on the transcript, not
//! rate-limited) the whole queue is combined into a single resume prompt and
//! fired via `claude --resume`. The mechanism is deliberately the same shape as
//! [`crate::parked`], which already resumes a Fleet-owned session when a parked
//! decision card is answered — the only difference is the trigger ("process went
//! idle" instead of "user answered a card").
//!
//! **Combine, don't queue N turns** (老板 decision 2026-07-14): several messages
//! typed while the turn runs are joined into one resume prompt, so a burst of
//! quick follow-ups becomes one coherent turn rather than N sequential
//! `claude --resume` spawns.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Request body for the `/enqueue_message` endpoint and the relay
/// `enqueue_message` method. Crosses the HTTP/relay boundary, so it needs both
/// `Serialize` and `Deserialize`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueMessageRequest {
    pub session_id: String,
    pub workspace_path: String,
    pub text: String,
}

/// The queued follow-ups for one session, oldest first.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PendingQueue {
    pub session_id: String,
    /// Where the session was launched — needed to `claude --resume` in the right
    /// cwd. Kept as the most recent value seen at enqueue time.
    pub workspace_path: String,
    pub messages: Vec<String>,
}

fn pending_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("pending-messages"))
}

/// Path of a session's queue file, or `None` for an id that could escape the
/// store directory. Mirrors `launch_spec::spec_path`'s sanitisation.
fn queue_path(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return None;
    }
    pending_dir().map(|d| d.join(format!("{session_id}.json")))
}

fn write_queue(path: &PathBuf, q: &PendingQueue) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create pending dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(q).map_err(|e| format!("serialize queue: {e}"))?;
    fs::write(path, json).map_err(|e| format!("write pending queue: {e}"))
}

/// The queue for `session_id`, or `None` when nothing is queued.
pub fn get(session_id: &str) -> Option<PendingQueue> {
    let path = queue_path(session_id)?;
    serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()
}

/// Drop a session's queue file. Idempotent — a missing file is success.
pub fn clear(session_id: &str) -> Result<(), String> {
    let Some(path) = queue_path(session_id) else {
        return Err("bad session id".into());
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove pending queue: {e}")),
    }
}

/// Append a follow-up message to a session's queue.
///
/// Only Fleet-owned sessions can be resumed headlessly, so this rejects sessions
/// Fleet didn't launch — the same gate [`crate::parked::parkable_workspace`]
/// uses, so a message never gets queued for a session that can never drain it.
pub fn enqueue(session_id: &str, workspace_path: &str, text: &str) -> Result<(), String> {
    let session_id = session_id.trim();
    let text = text.trim();
    if text.is_empty() {
        return Err("empty message".into());
    }
    if crate::parked::parkable_workspace(session_id).is_none() {
        return Err("session is not Fleet-owned; cannot queue a follow-up".into());
    }
    let path = queue_path(session_id).ok_or("bad session id")?;
    let mut q = get(session_id).unwrap_or_else(|| PendingQueue {
        session_id: session_id.to_string(),
        workspace_path: workspace_path.to_string(),
        messages: Vec::new(),
    });
    // Keep the freshest workspace in case the session moved (rare, but cheap).
    if !workspace_path.trim().is_empty() {
        q.workspace_path = workspace_path.to_string();
    }
    q.messages.push(text.to_string());
    write_queue(&path, &q)
}

/// True when `session_id` has at least one queued follow-up. Cheap re-entry
/// guard for the UI ("show the pending chips").
pub fn has_pending(session_id: &str) -> bool {
    get(session_id).map(|q| !q.messages.is_empty()).unwrap_or(false)
}

/// Every session's queued follow-ups, `session_id → messages`. One directory
/// read; the store is normally empty, so this is cheap on the scan path.
pub fn all_pending() -> std::collections::HashMap<String, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    let Some(dir) = pending_dir() else {
        return map;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(q) = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PendingQueue>(&raw).ok())
        {
            if !q.messages.is_empty() {
                map.insert(q.session_id, q.messages);
            }
        }
    }
    map
}

/// Attach each session's queued follow-ups to its [`SessionInfo`], so the
/// desktop/mobile UIs can render "queued" chips without a separate fetch. Rides
/// the existing sessions snapshot, so it works over both the local and remote
/// backends. Sessions with no queue keep the empty default.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let map = all_pending();
    if map.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        if let Some(msgs) = map.get(&s.id) {
            s.pending_messages = msgs.clone();
        }
    }
}

/// Whether a session's turn is over and its queue is safe to fire.
///
/// The definitive signal is `proc_alive`: for a Fleet-spawned session it is true
/// exactly while a `claude` process carries this session id in its argv (see
/// [`crate::session::SessionInfo::proc_alive`]). `false` means "turn ended,
/// process gone" — resumable. We additionally skip `RateLimited` (auto-resume
/// owns those; a resume would just bounce off the limit) and sessions with a
/// parked decision card (resuming would race the answer path).
fn is_drainable(session: &crate::session::SessionInfo) -> bool {
    if session.proc_alive {
        return false;
    }
    // Auto-resume owns both RateLimited and ServerErrored recovery; draining a
    // pending message would race the retry path, so skip both.
    if matches!(
        session.status,
        crate::session::SessionStatus::RateLimited
            | crate::session::SessionStatus::ServerErrored
    ) {
        return false;
    }
    if crate::parked::has_parked_for_session(&session.id) {
        return false;
    }
    true
}

/// If `session` has queued follow-ups and its turn is over, combine them into one
/// resume prompt and hand it to `spawn`. No-op otherwise.
///
/// `spawn` matches [`crate::auto_resume::spawn_resume_prompt`]'s signature
/// `(session_id, workspace, prompt, model, effort, permission_mode)`; injected so
/// the drain gate is unit-testable without spawning a real `claude`.
///
/// The queue is cleared **before** `spawn` runs, not after: the desktop tick can
/// fire again within a second, before the freshly-spawned `claude` shows up as
/// `proc_alive`, so a clear-after would risk a *second* resume on the same
/// transcript — the one corruption this whole module exists to avoid. The
/// trade-off is that a `spawn` failure loses the queued text (logged); a message
/// the user can retype is the cheaper loss.
pub fn drain_if_idle<S>(session: &crate::session::SessionInfo, spawn: S)
where
    S: FnOnce(&str, &str, &str, Option<&str>, Option<&str>, Option<&str>) -> Result<(), String>,
{
    let Some(q) = get(&session.id) else {
        return;
    };
    if q.messages.is_empty() {
        let _ = clear(&session.id);
        return;
    }
    if !is_drainable(session) {
        return;
    }
    // Belt-and-braces against the proc_alive scan lagging a still-live turn:
    // never put a second process on the transcript. Re-check liveness against the
    // *current* process table by source — a Codex session's identity is a live
    // `codex` process, not a `claude` one, so the Claude-only `session_pid` check
    // would miss a still-running Codex turn and fire a corrupting second resume.
    let still_live = if session.agent_source == crate::codex_launch::FLEET_AGENT_SOURCE_CODEX {
        crate::codex_source::codex_session_alive(&session.id)
    } else {
        crate::parked::session_pid(&session.id).is_some()
    };
    if still_live {
        return;
    }

    let prompt = q.messages.join("\n\n");
    let workspace = if q.workspace_path.trim().is_empty() {
        session.workspace_path.clone()
    } else {
        q.workspace_path.clone()
    };
    // Same model/effort the session was launched with, so a follow-up never
    // silently downgrades an `opus[1m]` turn to plain `opus` (parked's lesson).
    // `permission_mode` is deliberately not carried — a follow-up must not
    // re-grant an elevated mode.
    let model = crate::session::resolve_session_model_spec(&session.id)
        .or_else(|| crate::launch_spec::model_of(&session.id));
    let effort = crate::launch_spec::effort_of(&session.id);

    // Clear first (see doc): removing the file is the double-fire guard.
    if let Err(e) = clear(&session.id) {
        crate::log_debug(&format!("pending_message: clear {} failed: {e}", session.id));
        return;
    }
    crate::log_debug(&format!(
        "pending_message: draining {} queued msg(s) for {} ({} chars)",
        q.messages.len(),
        session.id,
        prompt.len()
    ));
    if let Err(e) = spawn(
        &session.id,
        &workspace,
        &prompt,
        model.as_deref(),
        effort.as_deref(),
        None,
    ) {
        crate::log_debug(&format!(
            "pending_message: resume {} failed: {e} (queued text lost)",
            session.id
        ));
    }
}

/// [`drain_if_idle`] with the real resume spawn wired in. This is what the
/// desktop session-refresh tick calls per session.
pub fn maybe_drain(session: &crate::session::SessionInfo) {
    // Dispatch the drain resume by the session's source, so a queued follow-up
    // on a Codex session is delivered via `codex exec resume`, not
    // `claude --resume`. Untracked → no-op on_exit box.
    let source = session.agent_source.clone();
    drain_if_idle(session, |sid, ws, prompt, model, effort, perm| {
        crate::agent_source::resume_session(
            &source,
            &crate::agent_source::ResumeSpec {
                session_id: sid.to_string(),
                workspace_path: ws.to_string(),
                prompt: prompt.to_string(),
                model: model.map(str::to_string),
                effort: effort.map(str::to_string),
                permission_mode: perm.map(str::to_string),
            },
            Box::new(|_| {}),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{fleet_home_lock, SessionInfo, SessionStatus};

    fn base_session(id: &str, status: SessionStatus, proc_alive: bool) -> SessionInfo {
        let mut s = SessionInfo::default();
        s.id = id.to_string();
        s.workspace_path = "/ws".to_string();
        s.status = status;
        s.proc_alive = proc_alive;
        s
    }

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!("fleet-pending-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);
        f();
        std::env::remove_var("HOME");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Records every spawn the drain fired, so the gate can be asserted without a
    /// real `claude`.
    fn recording_spawn(
        log: &std::cell::RefCell<Vec<(String, String)>>,
    ) -> impl FnOnce(&str, &str, &str, Option<&str>, Option<&str>, Option<&str>) -> Result<(), String> + '_
    {
        move |sid, _ws, prompt, _m, _e, _p| {
            log.borrow_mut().push((sid.to_string(), prompt.to_string()));
            Ok(())
        }
    }

    #[test]
    fn queue_path_rejects_traversal() {
        assert!(queue_path("../etc/passwd").is_none());
        assert!(queue_path("a/b").is_none());
        assert!(queue_path("").is_none());
        assert!(queue_path("normal-id").is_some());
    }

    #[test]
    fn empty_queue_never_fires() {
        with_temp_home(|| {
            let log = std::cell::RefCell::new(Vec::new());
            let s = base_session("sess-empty", SessionStatus::WaitingInput, false);
            drain_if_idle(&s, recording_spawn(&log));
            assert!(log.borrow().is_empty(), "no queue file => no spawn");
        });
    }

    #[test]
    fn does_not_fire_while_process_alive() {
        with_temp_home(|| {
            // Enqueue by writing the file directly (enqueue() gates on Fleet
            // ownership, which needs a real transcript; the drain gate is what
            // this test isolates).
            let path = queue_path("sess-live").unwrap();
            write_queue(
                &path,
                &PendingQueue {
                    session_id: "sess-live".into(),
                    workspace_path: "/ws".into(),
                    messages: vec!["hi".into()],
                },
            )
            .unwrap();
            let log = std::cell::RefCell::new(Vec::new());
            // proc_alive = true => turn still running => must not resume.
            let s = base_session("sess-live", SessionStatus::Processing, true);
            drain_if_idle(&s, recording_spawn(&log));
            assert!(log.borrow().is_empty(), "must not resume a live turn");
            assert!(get("sess-live").is_some(), "queue must survive for later");
        });
    }

    #[test]
    fn does_not_fire_when_rate_limited() {
        with_temp_home(|| {
            let path = queue_path("sess-rl").unwrap();
            write_queue(
                &path,
                &PendingQueue {
                    session_id: "sess-rl".into(),
                    workspace_path: "/ws".into(),
                    messages: vec!["hi".into()],
                },
            )
            .unwrap();
            let log = std::cell::RefCell::new(Vec::new());
            let s = base_session("sess-rl", SessionStatus::RateLimited, false);
            drain_if_idle(&s, recording_spawn(&log));
            assert!(log.borrow().is_empty(), "rate-limited => auto_resume owns it");
            assert!(get("sess-rl").is_some(), "queue must survive rate-limit");
        });
    }

    /// A Codex session whose turn is over must still drain: the belt-and-braces
    /// liveness recheck is source-routed, so a Codex session takes the
    /// `codex_session_alive` path (no live `codex` process for this fake id) and
    /// is NOT blocked by the Claude-only `session_pid` check (M5 P15).
    #[test]
    fn codex_session_drains_when_idle() {
        with_temp_home(|| {
            let path = queue_path("sess-codex-idle").unwrap();
            write_queue(
                &path,
                &PendingQueue {
                    session_id: "sess-codex-idle".into(),
                    workspace_path: "/ws".into(),
                    messages: vec!["follow up".into()],
                },
            )
            .unwrap();
            let log = std::cell::RefCell::new(Vec::new());
            let mut s = base_session("sess-codex-idle", SessionStatus::WaitingInput, false);
            s.agent_source = "codex".into();
            drain_if_idle(&s, recording_spawn(&log));
            let fired = log.borrow();
            assert_eq!(fired.len(), 1, "idle codex session must drain its queue");
            assert_eq!(fired[0].0, "sess-codex-idle");
            assert!(get("sess-codex-idle").is_none(), "queue cleared after firing");
        });
    }

    #[test]
    fn combines_messages_and_clears_when_idle() {
        with_temp_home(|| {
            let path = queue_path("sess-idle").unwrap();
            write_queue(
                &path,
                &PendingQueue {
                    session_id: "sess-idle".into(),
                    workspace_path: "/ws".into(),
                    messages: vec!["first".into(), "second".into()],
                },
            )
            .unwrap();
            let log = std::cell::RefCell::new(Vec::new());
            let s = base_session("sess-idle", SessionStatus::WaitingInput, false);
            drain_if_idle(&s, recording_spawn(&log));
            let fired = log.borrow();
            assert_eq!(fired.len(), 1, "burst combines into ONE resume");
            assert_eq!(fired[0].0, "sess-idle");
            assert_eq!(fired[0].1, "first\n\nsecond", "messages joined in order");
            assert!(get("sess-idle").is_none(), "queue cleared after firing");
        });
    }
}
