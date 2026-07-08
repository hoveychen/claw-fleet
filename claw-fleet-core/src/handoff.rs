//! Handoff — session-to-session relay for long-running work.
//!
//! When an agent senses its context is running long mid-plan, it registers a
//! handoff (`fleet handoff --note "..."`) instead of grinding to a halt. The
//! Stop hook (`fleet session idle`) then consumes the pending record the
//! moment the session yields its turn and spawns a fresh successor session
//! whose opening prompt carries the mandatory handoff note (plus plan/P
//! pointers when the work is a TASKS.md plan — the prd-context hook re-injects
//! the macro plan into the successor automatically).
//!
//! Why the Stop hook and not a desktop watcher: the hook runs on the machine
//! the session lives on, so local and remote workspaces behave identically,
//! there is zero polling latency, and the relay works even when the Fleet
//! desktop app is closed.
//!
//! File layout:
//! - `~/.fleet/handoffs/pending/<session_id>.json` — at most one un-consumed
//!   handoff per session; re-registering overwrites.
//! - `~/.fleet/handoffs/chain/<chain_id>.json` — consumed links, append-only.
//!   Chain membership is derived (last link's `to_session_id` == registering
//!   session) rather than threaded through process envs, so a successor that
//!   registers its own handoff continues the same chain without plumbing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A registered-but-not-yet-consumed handoff, keyed by the source session.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingHandoff {
    pub from_session_id: String,
    /// Workspace the successor session must be spawned in.
    pub workspace_path: String,
    /// Mandatory relay note: what's done, what's next, key files, gotchas.
    pub note: String,
    /// TASKS.md plan the relay continues, when the work is plan-bound.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan_id: Option<String>,
    /// Next P-item token (e.g. `P4`), when plan-bound.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_task: Option<String>,
    /// Chain this handoff extends; a fresh uuid when the session starts one.
    pub chain_id: String,
    /// 1-based position of the *source* session in the chain.
    pub hop: u32,
    /// Epoch ms of registration — consumption past `EXPIRY_MS` is refused.
    pub created: u64,
}

/// One consumed relay step: `from` yielded its turn and `to` was spawned.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffLink {
    pub from_session_id: String,
    pub to_session_id: String,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_task: Option<String>,
    /// Epoch ms the successor was spawned.
    pub handed_at: u64,
}

/// A full relay chain. `links.len()` handoffs connect `links.len() + 1`
/// sessions; session N's transcript ends where session N+1's begins.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffChain {
    pub chain_id: String,
    pub workspace_path: String,
    /// Plan of the most recent plan-bound link, for grouping in the UI.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan_id: Option<String>,
    pub links: Vec<HandoffLink>,
}

impl HandoffChain {
    /// Ordered session ids on this chain (from -> ... -> latest to).
    pub fn session_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::with_capacity(self.links.len() + 1);
        for l in &self.links {
            if ids.last().map(|s| s != &l.from_session_id).unwrap_or(true) {
                ids.push(l.from_session_id.clone());
            }
            ids.push(l.to_session_id.clone());
        }
        ids
    }

    /// 1-based hop of `session_id` on this chain, if present.
    pub fn hop_of(&self, session_id: &str) -> Option<u32> {
        self.session_ids()
            .iter()
            .position(|s| s == session_id)
            .map(|i| i as u32 + 1)
    }
}

/// Lightweight per-session relay position, embedded into `SessionInfo` so the
/// session card can render the "接力 n/N" chip without an extra round-trip.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionHandoffInfo {
    pub chain_id: String,
    /// This session's 1-based position on the chain.
    pub hop: u32,
    /// Total sessions currently on the chain.
    pub chain_len: u32,
}

/// A pending handoff older than this is treated as abandoned and discarded
/// instead of spawning a successor (e.g. hook was disabled when the session
/// stopped, and it only fires again much later).
pub const EXPIRY_MS: u64 = 30 * 60 * 1000;

/// Hard ceiling on chain length — a relay loop (successor immediately hands
/// off again without progress) must not spawn sessions forever.
pub const MAX_CHAIN_HOPS: u32 = 10;

fn handoffs_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("handoffs"))
}

fn pending_dir() -> Option<PathBuf> {
    handoffs_dir().map(|d| d.join("pending"))
}

fn chain_dir() -> Option<PathBuf> {
    handoffs_dir().map(|d| d.join("chain"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── registration ──────────────────────────────────────────────────────────────

/// Register a handoff for `session_id`. Overwrites any previous un-consumed
/// registration by the same session. Fails when the chain the session sits on
/// has already reached `MAX_CHAIN_HOPS`.
pub fn register(
    session_id: &str,
    workspace_path: &str,
    note: &str,
    plan_id: Option<&str>,
    next_task: Option<&str>,
) -> Result<PendingHandoff, String> {
    let pdir = pending_dir().ok_or("cannot determine home dir")?;
    let cdir = chain_dir().ok_or("cannot determine home dir")?;
    register_in(&pdir, &cdir, session_id, workspace_path, note, plan_id, next_task, now_ms())
}

#[allow(clippy::too_many_arguments)]
fn register_in(
    pending_dir: &Path,
    chain_dir: &Path,
    session_id: &str,
    workspace_path: &str,
    note: &str,
    plan_id: Option<&str>,
    next_task: Option<&str>,
    now: u64,
) -> Result<PendingHandoff, String> {
    let note = note.trim();
    if note.is_empty() {
        return Err("handoff note is required".to_string());
    }
    // Continue the chain this session was itself handed, if any.
    let (chain_id, hop) = match chain_containing_in(chain_dir, session_id) {
        Some(chain) => {
            let hop = chain.hop_of(session_id).unwrap_or(1);
            if hop >= MAX_CHAIN_HOPS {
                return Err(format!(
                    "handoff chain {} already has {} hops (max {}) — refusing to extend; \
                     finish or surface the work instead",
                    chain.chain_id, hop, MAX_CHAIN_HOPS
                ));
            }
            (chain.chain_id, hop)
        }
        None => (uuid::Uuid::new_v4().to_string(), 1),
    };
    let rec = PendingHandoff {
        from_session_id: session_id.to_string(),
        workspace_path: workspace_path.to_string(),
        note: note.to_string(),
        plan_id: plan_id.map(str::to_string),
        next_task: next_task.map(str::to_string),
        chain_id,
        hop,
        created: now,
    };
    fs::create_dir_all(pending_dir).map_err(|e| format!("create pending dir: {e}"))?;
    let path = pending_dir.join(format!("{session_id}.json"));
    let json = serde_json::to_string_pretty(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write pending handoff: {e}"))?;
    Ok(rec)
}

/// Read a session's pending handoff without consuming it.
pub fn read_pending(session_id: &str) -> Option<PendingHandoff> {
    let dir = pending_dir()?;
    read_pending_in(&dir, session_id)
}

fn read_pending_in(dir: &Path, session_id: &str) -> Option<PendingHandoff> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

/// Cancel a session's pending handoff (e.g. the user prompted the session
/// again, taking over manually). Idempotent.
pub fn cancel_pending(session_id: &str) {
    if let Some(dir) = pending_dir() {
        let _ = fs::remove_file(dir.join(format!("{session_id}.json")));
    }
}

/// Atomically take a session's pending handoff for consumption. Expired
/// records are discarded and `None` is returned.
pub fn take_pending(session_id: &str) -> Option<PendingHandoff> {
    let dir = pending_dir()?;
    take_pending_in(&dir, session_id, now_ms())
}

fn take_pending_in(dir: &Path, session_id: &str, now: u64) -> Option<PendingHandoff> {
    let path = dir.join(format!("{session_id}.json"));
    let rec = read_pending_in(dir, session_id)?;
    // Remove first: even on the expired path the record must not fire twice.
    let _ = fs::remove_file(&path);
    if now.saturating_sub(rec.created) > EXPIRY_MS {
        crate::log_debug(&format!(
            "handoff: pending record for {session_id} expired ({}ms old), discarded",
            now.saturating_sub(rec.created)
        ));
        return None;
    }
    Some(rec)
}

// ── chain archive ─────────────────────────────────────────────────────────────

/// Append the consumed link to its chain file (creating the chain on first
/// link) once the successor session has been spawned.
pub fn record_link(pending: &PendingHandoff, to_session_id: &str) -> Result<(), String> {
    let dir = chain_dir().ok_or("cannot determine home dir")?;
    record_link_in(&dir, pending, to_session_id, now_ms())
}

fn record_link_in(
    dir: &Path,
    pending: &PendingHandoff,
    to_session_id: &str,
    now: u64,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create chain dir: {e}"))?;
    let path = dir.join(format!("{}.json", pending.chain_id));
    let mut chain = read_chain_file(&path).unwrap_or_else(|| HandoffChain {
        chain_id: pending.chain_id.clone(),
        workspace_path: pending.workspace_path.clone(),
        plan_id: None,
        links: Vec::new(),
    });
    if pending.plan_id.is_some() {
        chain.plan_id = pending.plan_id.clone();
    }
    chain.links.push(HandoffLink {
        from_session_id: pending.from_session_id.clone(),
        to_session_id: to_session_id.to_string(),
        note: pending.note.clone(),
        plan_id: pending.plan_id.clone(),
        next_task: pending.next_task.clone(),
        handed_at: now,
    });
    let json = serde_json::to_string_pretty(&chain).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write chain: {e}"))
}

fn read_chain_file(path: &Path) -> Option<HandoffChain> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// All recorded chains, unordered.
pub fn list_chains() -> Vec<HandoffChain> {
    let Some(dir) = chain_dir() else {
        return Vec::new();
    };
    list_chains_in(&dir)
}

fn list_chains_in(dir: &Path) -> Vec<HandoffChain> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| read_chain_file(&e.path()))
        .collect()
}

/// The chain a session appears on (as source or successor of any link).
pub fn chain_containing(session_id: &str) -> Option<HandoffChain> {
    let dir = chain_dir()?;
    chain_containing_in(&dir, session_id)
}

fn chain_containing_in(dir: &Path, session_id: &str) -> Option<HandoffChain> {
    list_chains_in(dir)
        .into_iter()
        .find(|c| c.hop_of(session_id).is_some())
}

/// Session-id → relay position for every session on any chain. Built once per
/// scanner pass so per-session enrichment stays O(1).
pub fn session_handoff_index() -> std::collections::HashMap<String, SessionHandoffInfo> {
    let Some(dir) = chain_dir() else {
        return Default::default();
    };
    session_handoff_index_in(&dir)
}

fn session_handoff_index_in(
    dir: &Path,
) -> std::collections::HashMap<String, SessionHandoffInfo> {
    let mut map = std::collections::HashMap::new();
    for chain in list_chains_in(dir) {
        let ids = chain.session_ids();
        let len = ids.len() as u32;
        for (i, sid) in ids.into_iter().enumerate() {
            map.insert(
                sid,
                SessionHandoffInfo {
                    chain_id: chain.chain_id.clone(),
                    hop: i as u32 + 1,
                    chain_len: len,
                },
            );
        }
    }
    map
}

/// Stamp relay positions onto scanned sessions. Called at scan-aggregation
/// time (not inside the mtime-cached per-session parse) because a
/// predecessor's chain membership changes when its successor spawns, without
/// its own jsonl ever being touched.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let idx = session_handoff_index();
    if idx.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        s.handoff = idx.get(&s.id).cloned();
    }
}

// ── successor prompt + consumption ────────────────────────────────────────────

/// Opening prompt for the successor session. The prd-context hook re-injects
/// the full TASKS.md plan on top of this, so the prompt only needs the relay
/// note and the concrete "resume here" pointer.
pub fn compose_successor_prompt(p: &PendingHandoff) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "你是一次接力开发的第 {} 棒，接替上一个 session（{}）继续未完成的工作。\n\n",
        p.hop + 1,
        p.from_session_id
    ));
    out.push_str("上一棒留下的交接信息：\n\n---\n");
    out.push_str(&p.note);
    out.push_str("\n---\n\n");
    if let Some(plan) = &p.plan_id {
        match &p.next_task {
            Some(t) => out.push_str(&format!(
                "本次接力属于 TASKS.md plan `{plan}`。先运行 `fleet plan start {plan} {t}` 声明接手，然后从 {t} 继续执行，直到整个 plan 完成。\n"
            )),
            None => out.push_str(&format!(
                "本次接力属于 TASKS.md plan `{plan}`。先运行 `fleet plan start {plan}` 声明接手，然后从第一个未完成的 P 继续执行，直到整个 plan 完成。\n"
            )),
        }
        out.push_str("plan 的完整任务清单会由 prd-context hook 自动注入你的上下文，以 TASKS.md 为准。\n");
    } else {
        out.push_str("按交接信息继续完成这项工作。\n");
    }
    out.push_str(
        "\n若你的上下文也接近上限，先用 `fleet handoff --note \"<交接信息>\"` 注册下一棒再结束 turn，不要中途弃工。",
    );
    out
}

/// Stop-hook entrypoint: consume `session_id`'s pending handoff (if any),
/// spawn the successor session, and archive the chain link. Returns the new
/// session id when a relay fired. Errors never propagate to the hook exit
/// code — callers log and move on.
pub fn consume_and_spawn(session_id: &str) -> Result<Option<String>, String> {
    let Some(pending) = take_pending(session_id) else {
        return Ok(None);
    };
    let prompt = compose_successor_prompt(&pending);
    let resp = crate::session_launch::spawn_new_session_with_entrypoint(
        &pending.workspace_path,
        &prompt,
        None,
        None,
        None,
        HANDOFF_ENTRYPOINT,
    )?;
    let to_sid = resp
        .session_id
        .ok_or_else(|| "spawn returned no session id".to_string())?;
    record_link(&pending, &to_sid)?;
    crate::log_debug(&format!(
        "handoff: relayed session {} -> {} (chain {}, hop {} -> {})",
        session_id,
        to_sid,
        pending.chain_id,
        pending.hop,
        pending.hop + 1
    ));
    Ok(Some(to_sid))
}

/// `CLAUDE_CODE_ENTRYPOINT` for relay-spawned sessions, so the scanner (and
/// humans reading transcripts) can tell them apart from user-initiated ones.
pub const HANDOFF_ENTRYPOINT: &str = "claw-fleet-handoff";

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "fleet-handoff-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        let pending = root.join("pending");
        let chain = root.join("chain");
        fs::create_dir_all(&pending).unwrap();
        fs::create_dir_all(&chain).unwrap();
        (root, pending, chain)
    }

    fn register_simple(
        pdir: &Path,
        cdir: &Path,
        sid: &str,
        note: &str,
        now: u64,
    ) -> Result<PendingHandoff, String> {
        register_in(pdir, cdir, sid, "/ws", note, Some("my-plan"), Some("P4"), now)
    }

    #[test]
    fn register_requires_note() {
        let (root, pdir, cdir) = fresh_dirs("note");
        let err = register_simple(&pdir, &cdir, "s1", "   ", 1).unwrap_err();
        assert!(err.contains("note is required"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn register_take_roundtrip() {
        let (root, pdir, cdir) = fresh_dirs("roundtrip");
        let rec = register_simple(&pdir, &cdir, "s1", "did A, next B", 1000).unwrap();
        assert_eq!(rec.hop, 1);
        assert_eq!(rec.plan_id.as_deref(), Some("my-plan"));

        // camelCase on the wire
        let raw = fs::read_to_string(pdir.join("s1.json")).unwrap();
        assert!(raw.contains("\"fromSessionId\""));
        assert!(raw.contains("\"planId\""));

        let taken = take_pending_in(&pdir, "s1", 2000).unwrap();
        assert_eq!(taken, rec);
        // consumed — gone
        assert!(take_pending_in(&pdir, "s1", 2000).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reregister_overwrites() {
        let (root, pdir, cdir) = fresh_dirs("overwrite");
        register_simple(&pdir, &cdir, "s1", "first", 1000).unwrap();
        register_simple(&pdir, &cdir, "s1", "second", 2000).unwrap();
        let taken = take_pending_in(&pdir, "s1", 3000).unwrap();
        assert_eq!(taken.note, "second");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn expired_pending_is_discarded_and_not_refired() {
        let (root, pdir, cdir) = fresh_dirs("expiry");
        register_simple(&pdir, &cdir, "s1", "stale", 1000).unwrap();
        assert!(take_pending_in(&pdir, "s1", 1000 + EXPIRY_MS + 1).is_none());
        // record must be gone, not lingering for a later take
        assert!(read_pending_in(&pdir, "s1").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn chain_continues_across_hops_and_caps_at_max() {
        let (root, pdir, cdir) = fresh_dirs("chain");
        // s1 -> s2
        let p1 = register_simple(&pdir, &cdir, "s1", "hop1", 1000).unwrap();
        assert_eq!(p1.hop, 1);
        let taken = take_pending_in(&pdir, "s1", 1001).unwrap();
        record_link_in(&cdir, &taken, "s2", 1002).unwrap();

        // s2 continues the SAME chain at hop 2
        let p2 = register_simple(&pdir, &cdir, "s2", "hop2", 2000).unwrap();
        assert_eq!(p2.chain_id, p1.chain_id);
        assert_eq!(p2.hop, 2);
        let taken = take_pending_in(&pdir, "s2", 2001).unwrap();
        record_link_in(&cdir, &taken, "s3", 2002).unwrap();

        let chain = chain_containing_in(&cdir, "s2").unwrap();
        assert_eq!(chain.session_ids(), vec!["s1", "s2", "s3"]);
        assert_eq!(chain.hop_of("s1"), Some(1));
        assert_eq!(chain.hop_of("s3"), Some(3));

        // an unrelated session starts a NEW chain
        let px = register_simple(&pdir, &cdir, "other", "solo", 3000).unwrap();
        assert_ne!(px.chain_id, p1.chain_id);
        assert_eq!(px.hop, 1);

        // walk s3..s(MAX) — registration at hop MAX_CHAIN_HOPS must refuse
        let mut sid = "s3".to_string();
        for i in 3..MAX_CHAIN_HOPS {
            let p = register_simple(&pdir, &cdir, &sid, "hopN", 10_000 + i as u64).unwrap();
            assert_eq!(p.hop, i);
            let taken = take_pending_in(&pdir, &sid, 10_001 + i as u64).unwrap();
            let next = format!("s{}", i + 1);
            record_link_in(&cdir, &taken, &next, 10_002 + i as u64).unwrap();
            sid = next;
        }
        let err = register_simple(&pdir, &cdir, &sid, "one too many", 99_999).unwrap_err();
        assert!(err.contains("refusing to extend"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn handoff_index_maps_every_session_on_chain() {
        let (root, pdir, cdir) = fresh_dirs("index");
        let p1 = register_simple(&pdir, &cdir, "a", "n", 1000).unwrap();
        let taken = take_pending_in(&pdir, "a", 1001).unwrap();
        record_link_in(&cdir, &taken, "b", 1002).unwrap();

        let idx = session_handoff_index_in(&cdir);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx["a"].hop, 1);
        assert_eq!(idx["b"].hop, 2);
        assert_eq!(idx["b"].chain_len, 2);
        assert_eq!(idx["a"].chain_id, p1.chain_id);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn successor_prompt_carries_note_plan_and_next_task() {
        let p = PendingHandoff {
            from_session_id: "src-sid".into(),
            workspace_path: "/ws".into(),
            note: "P1-P3 已完成；P4 卡在 X".into(),
            plan_id: Some("auth-refactor".into()),
            next_task: Some("P4".into()),
            chain_id: "c1".into(),
            hop: 2,
            created: 1,
        };
        let prompt = compose_successor_prompt(&p);
        assert!(prompt.contains("第 3 棒"));
        assert!(prompt.contains("src-sid"));
        assert!(prompt.contains("P1-P3 已完成；P4 卡在 X"));
        assert!(prompt.contains("fleet plan start auth-refactor P4"));

        // free-form handoff: no plan pointer, still carries the note
        let free = PendingHandoff {
            plan_id: None,
            next_task: None,
            ..p
        };
        let prompt = compose_successor_prompt(&free);
        assert!(!prompt.contains("fleet plan start"));
        assert!(prompt.contains("按交接信息继续完成这项工作"));
    }
}
