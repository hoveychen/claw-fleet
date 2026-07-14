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
    /// Workspace the successor session must be spawned in — the *session's* cwd,
    /// not the agent's shell cwd. See `predecessor_cwd`.
    pub workspace_path: String,
    /// Where the predecessor's shell was working when it registered the handoff,
    /// recorded only when it differs from `workspace_path` — under the Rule-3
    /// worktree workflow that is `<repo>/.worktrees/<task>`. The successor is
    /// told about it but is NOT spawned there: the same plan ends by merging and
    /// removing that worktree, which would delete the successor's own cwd and
    /// make the session invisible to the scan (its workspace dir is gone).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub predecessor_cwd: Option<String>,
    /// Mandatory relay note: what's done, what's next, key files, gotchas.
    pub note: String,
    /// TASKS.md plan the relay continues, when the work is plan-bound.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan_id: Option<String>,
    /// Next P-item token (e.g. `P4`), when plan-bound.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_task: Option<String>,
    /// Model spec the source session was running, so the successor continues on
    /// the same model instead of silently falling back to the CLI default.
    /// `None` — including on pending files written before this field existed —
    /// means "let the CLI pick", preserving the old behaviour.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Reasoning effort the source session was running; same rationale as
    /// `model`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<String>,
    /// Chain this handoff extends; a fresh uuid when the session starts one.
    pub chain_id: String,
    /// 1-based position of the *source* session in the chain.
    pub hop: u32,
    /// Epoch ms of registration — consumption past `EXPIRY_MS` is refused.
    pub created: u64,
}

/// One consumed relay step: `from` yielded its turn and `to` was spawned.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
#[allow(clippy::too_many_arguments)]
pub fn register(
    session_id: &str,
    workspace_path: &str,
    predecessor_cwd: Option<&str>,
    note: &str,
    plan_id: Option<&str>,
    next_task: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<PendingHandoff, String> {
    let pdir = pending_dir().ok_or("cannot determine home dir")?;
    let cdir = chain_dir().ok_or("cannot determine home dir")?;
    register_in(
        &pdir,
        &cdir,
        session_id,
        workspace_path,
        predecessor_cwd,
        note,
        plan_id,
        next_task,
        model,
        effort,
        now_ms(),
    )
}

#[allow(clippy::too_many_arguments)]
fn register_in(
    pending_dir: &Path,
    chain_dir: &Path,
    session_id: &str,
    workspace_path: &str,
    predecessor_cwd: Option<&str>,
    note: &str,
    plan_id: Option<&str>,
    next_task: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
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
    let blank = |s: &str| s.trim().is_empty();
    let rec = PendingHandoff {
        from_session_id: session_id.to_string(),
        workspace_path: workspace_path.to_string(),
        predecessor_cwd: predecessor_cwd
            .filter(|c| !c.trim().is_empty() && *c != workspace_path)
            .map(str::to_string),
        note: note.to_string(),
        plan_id: plan_id.map(str::to_string),
        next_task: next_task.map(str::to_string),
        model: model.filter(|m| !blank(m)).map(str::to_string),
        effort: effort.filter(|e| !blank(e)).map(str::to_string),
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
    if let Some(cwd) = &p.predecessor_cwd {
        out.push_str(&format!(
            "上一棒的代码工作在 `{cwd}`（多半是 Rule 3 的 worktree），而你被启动在会话原本的 workspace 里。\
             该目录若仍存在，先 `cd` 进去再继续；若计划已合并、worktree 已清理，就留在当前目录。\n\n"
        ));
    }
    if let Some(plan) = &p.plan_id {
        match &p.next_task {
            Some(t) => out.push_str(&format!(
                "本次接力属于 TASKS.md plan `{plan}`，Fleet 已把你归属到该 plan 的 {t}。直接从 {t} 继续执行，直到整个 plan 完成。\n"
            )),
            None => out.push_str(&format!(
                "本次接力属于 TASKS.md plan `{plan}`，Fleet 已把你归属到该 plan。从第一个未完成的 P 继续执行，直到整个 plan 完成。\n"
            )),
        }
        out.push_str("plan 的完整任务清单会由 prd-context hook 自动注入你的上下文，以 TASKS.md 为准。\n");
        out.push_str(
            "若该 plan 是子 plan（sentinel 带 `parent=`），做完最后一个 P 后 Fleet 会把你的焦点\
             切回父 plan 并指示下一个 P——按提示继续，别因为子 plan 完成就收工。\n",
        );
    } else {
        out.push_str("按交接信息继续完成这项工作。\n");
    }
    out.push_str(
        "\n接手第一件事：交接信息里若含「测试已过 / build 绿 / 接口返回 X」这类**声称的**验证结论，\
         别直接信任——先自己重跑对应的验证命令，确认它当下仍然成立，再据此往下做。\
         上一棒的声称可能是伪造的，也可能在交接后已经失效；不核实就继续，会把未经证实的结论沿接力链一路传下去。\n",
    );
    out.push_str(
        "\n若你的上下文也接近上限，先用 `fleet handoff --note \"<交接信息>\"` 注册下一棒再结束 turn，不要中途弃工。",
    );
    out
}

/// Stamp the successor's plan attribution in the `task_progress` side-channel.
///
/// Fleet spawns the successor, so it alone knows both the new session id and the
/// plan being relayed. Recording it here means the successor's card shows plan
/// progress from its first scan, instead of depending on the agent noticing the
/// `fleet plan resume` instruction in its opening prompt. A free-form relay
/// (no `plan_id`) attributes nothing — there is no plan to point at.
///
/// Takes an explicit record dir so tests don't race the process-global
/// `FLEET_HOME` that other suites mutate.
fn attribute_successor_in(dir: &Path, pending: &PendingHandoff, to_sid: &str) {
    let Some(plan_id) = pending.plan_id.as_deref() else {
        return;
    };
    let ws = Path::new(&pending.workspace_path);
    let current = crate::prd_tasks::resolve_current_task(ws, plan_id, pending.next_task.as_deref())
        .unwrap_or(None);
    // `register` stores the raw cwd, which may be a worktree. Stamp the main
    // checkout instead, matching what `fleet plan resume/check` record — the
    // card's workspace check compares main roots.
    let ws_root = crate::prd_tasks::discover_main_checkout_root(ws)
        .unwrap_or_else(|| ws.to_path_buf());
    if let Err(e) =
        crate::task_progress::set_current_in(dir, to_sid, &ws_root.to_string_lossy(), plan_id, current)
    {
        crate::log_debug(&format!(
            "handoff: could not attribute successor {to_sid} to plan {plan_id}: {e}"
        ));
    }
}

/// Stop-hook entrypoint: consume `session_id`'s pending handoff (if any),
/// spawn the successor session, and archive the chain link. Returns the new
/// session id when a relay fired. Errors never propagate to the hook exit
/// code — callers log and move on.
pub fn consume_and_spawn(session_id: &str) -> Result<Option<String>, String> {
    let pdir = pending_dir().ok_or("cannot determine home dir")?;
    let cdir = chain_dir().ok_or("cannot determine home dir")?;
    let progress = crate::task_progress::progress_dir();
    consume_and_spawn_in(
        &pdir,
        &cdir,
        progress.as_deref(),
        session_id,
        now_ms(),
        crate::session_launch::spawn_new_session_with_entrypoint,
    )
}

/// `consume_and_spawn` against explicit record dirs and an injectable spawner,
/// so tests can observe exactly which launch flags the successor receives
/// without starting a real `claude` process.
fn consume_and_spawn_in<S>(
    pending_dir: &Path,
    chain_dir: &Path,
    progress_dir: Option<&Path>,
    session_id: &str,
    now: u64,
    spawn: S,
) -> Result<Option<String>, String>
where
    S: FnOnce(
        &str,
        &str,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        &str,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String>,
{
    let Some(pending) = take_pending_in(pending_dir, session_id, now) else {
        return Ok(None);
    };
    let prompt = compose_successor_prompt(&pending);
    // The relay continues the predecessor's work, so it continues on the
    // predecessor's model and effort. Falling through to the CLI default here
    // would silently switch models mid-plan. `permission_mode` is deliberately
    // left to the default: a relay should not inherit an elevated mode.
    let resp = spawn(
        &pending.workspace_path,
        &prompt,
        pending.model.as_deref(),
        pending.effort.as_deref(),
        None,
        HANDOFF_ENTRYPOINT,
    )?;
    let to_sid = resp
        .session_id
        .ok_or_else(|| "spawn returned no session id".to_string())?;
    if let Some(dir) = progress_dir {
        attribute_successor_in(dir, &pending, &to_sid);
    }
    record_link_in(chain_dir, &pending, &to_sid, now)?;
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
        register_in(
            pdir,
            cdir,
            sid,
            "/ws",
            None,
            note,
            Some("my-plan"),
            Some("P4"),
            None,
            None,
            now,
        )
    }

    /// Launch flags the injected spawner observed, so a test can assert on what
    /// the successor would actually have been started with.
    #[derive(Default)]
    struct SpawnSpy {
        model: Option<String>,
        effort: Option<String>,
    }

    /// The relay exists to rescue a session whose context ran out. Dropping the
    /// predecessor's model on the floor silently demotes (or promotes) the
    /// successor to whatever `~/.claude/settings.json` happens to default to —
    /// a fable-5 relay would wake up as opus. The launch flags must follow the
    /// registration.
    #[test]
    fn successor_is_spawned_with_the_predecessors_model_and_effort() {
        let (root, pdir, cdir) = fresh_dirs("inherit");
        register_in(
            &pdir,
            &cdir,
            "s1",
            "/ws",
            None,
            "note",
            None,
            None,
            Some("claude-fable-5"),
            Some("high"),
            1000,
        )
        .unwrap();

        let spy = std::cell::RefCell::new(SpawnSpy::default());
        let to = consume_and_spawn_in(
            &pdir,
            &cdir,
            None,
            "s1",
            1001,
            |_ws, _prompt, model, effort, _perm, entrypoint| {
                assert_eq!(entrypoint, HANDOFF_ENTRYPOINT);
                let mut s = spy.borrow_mut();
                s.model = model.map(str::to_string);
                s.effort = effort.map(str::to_string);
                Ok(crate::session_launch::SpawnSessionResponse {
                    pid: 42,
                    session_id: Some("s2".to_string()),
                })
            },
        )
        .unwrap();

        assert_eq!(to.as_deref(), Some("s2"));
        let spy = spy.into_inner();
        assert_eq!(
            spy.model.as_deref(),
            Some("claude-fable-5"),
            "successor must inherit the predecessor's model, not the CLI default"
        );
        assert_eq!(spy.effort.as_deref(), Some("high"));
        let _ = fs::remove_dir_all(&root);
    }

    /// A relay registered without a model (old pending files, or a session whose
    /// model couldn't be resolved) must not invent one — the CLI default stands.
    #[test]
    fn successor_without_recorded_model_passes_no_override() {
        let (root, pdir, cdir) = fresh_dirs("inherit-none");
        register_simple(&pdir, &cdir, "s1", "note", 1000).unwrap();

        let spy = std::cell::RefCell::new(SpawnSpy::default());
        consume_and_spawn_in(&pdir, &cdir, None, "s1", 1001, |_w, _p, model, effort, _pm, _e| {
            let mut s = spy.borrow_mut();
            s.model = model.map(str::to_string);
            s.effort = effort.map(str::to_string);
            Ok(crate::session_launch::SpawnSessionResponse {
                pid: 1,
                session_id: Some("s2".to_string()),
            })
        })
        .unwrap();

        let spy = spy.into_inner();
        assert_eq!(spy.model, None);
        assert_eq!(spy.effort, None);
        let _ = fs::remove_dir_all(&root);
    }

    /// Build a `PendingHandoff` whose workspace holds `tasks_md`.
    fn relay_pending(ws: &Path, plan_id: Option<&str>, next: Option<&str>) -> PendingHandoff {
        PendingHandoff {
            from_session_id: "s1".into(),
            workspace_path: ws.to_string_lossy().into_owned(),
            predecessor_cwd: None,
            note: "n".into(),
            plan_id: plan_id.map(str::to_string),
            next_task: next.map(str::to_string),
            model: None,
            effort: None,
            chain_id: "c1".into(),
            hop: 1,
            created: 1,
        }
    }

    /// The successor is spawned by Fleet, so Fleet knows its session id and the
    /// plan being relayed — it must stamp the attribution side-channel itself
    /// rather than trusting the agent to run `fleet plan resume`.
    #[test]
    fn attribute_successor_stamps_the_relayed_plan() {
        let ws = tempfile::tempdir().unwrap();
        let recs = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"relay\" v=\"2\" -->\n\
**Plan:** Relay\n- [x] **P1** — a\n- [ ] **P2** — b\n- [ ] **P3** — c\n\
<!-- fleet:prd:end id=\"relay\" -->\n",
        )
        .unwrap();
        let pending = relay_pending(ws.path(), Some("relay"), Some("P3"));
        attribute_successor_in(recs.path(), &pending, "to-sid");
        let rec = crate::task_progress::read_in(recs.path(), "to-sid")
            .expect("successor must be attributed");
        assert_eq!(rec.plan_id, "relay");
        assert_eq!(rec.workspace_path, pending.workspace_path);
        // next_task "P3" resolves to that item's full text, not the bare token.
        assert_eq!(rec.current_task.as_deref(), Some("**P3** — c"));
    }

    /// A free-form relay carries no plan pointer — there is nothing to attribute,
    /// and the successor's card must stay blank rather than inherit a guess.
    #[test]
    fn attribute_successor_noop_for_free_form_relay() {
        let recs = tempfile::tempdir().unwrap();
        let pending = relay_pending(Path::new("/nonexistent-ws"), None, None);
        attribute_successor_in(recs.path(), &pending, "to-sid");
        assert!(crate::task_progress::read_in(recs.path(), "to-sid").is_none());
    }

    /// Relay says P9 but the plan omits a P9 item: fall back to the bare token
    /// rather than dropping attribution entirely.
    #[test]
    fn attribute_successor_falls_back_to_bare_token() {
        let ws = tempfile::tempdir().unwrap();
        let recs = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"relay\" v=\"2\" -->\n\
**Plan:** Relay\n- [ ] **P1** — a\n<!-- fleet:prd:end id=\"relay\" -->\n",
        )
        .unwrap();
        let pending = relay_pending(ws.path(), Some("relay"), Some("P9"));
        attribute_successor_in(recs.path(), &pending, "to-sid");
        let rec = crate::task_progress::read_in(recs.path(), "to-sid").expect("attributed");
        assert_eq!(rec.current_task.as_deref(), Some("**P9**"));
    }

    /// `fleet handoff` stores the raw cwd, which is often a worktree. The record
    /// must name the main checkout, because that is what the card's workspace
    /// check compares against — stamping the worktree path would make the
    /// successor's card go blank.
    #[test]
    fn attribute_successor_records_the_main_checkout_not_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let recs = tempfile::tempdir().unwrap();
        let main = tmp.path().canonicalize().unwrap();
        let main_gitdir = main.join(".git");
        std::fs::create_dir_all(&main_gitdir).unwrap();
        std::fs::write(
            main.join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"relay\" v=\"2\" -->\n\
**Plan:** Relay\n- [ ] **P1** — a\n<!-- fleet:prd:end id=\"relay\" -->\n",
        )
        .unwrap();
        let wt = main.join(".worktrees").join("feat");
        std::fs::create_dir_all(&wt).unwrap();
        let wt_gitdir = main_gitdir.join("worktrees").join("feat");
        std::fs::create_dir_all(&wt_gitdir).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}", wt_gitdir.display())).unwrap();
        std::fs::write(wt_gitdir.join("commondir"), "../..").unwrap();

        // The relay was registered from inside the worktree.
        let pending = relay_pending(&wt, Some("relay"), None);
        attribute_successor_in(recs.path(), &pending, "to-sid");
        let rec = crate::task_progress::read_in(recs.path(), "to-sid").expect("attributed");
        assert_eq!(
            Path::new(&rec.workspace_path).canonicalize().unwrap(),
            main,
            "record must name the main checkout, not the worktree"
        );
    }

    /// A relay whose plan isn't in the workspace's TASKS.md still attributes the
    /// plan id (the successor may create it), but carries no task text.
    #[test]
    fn attribute_successor_without_resolvable_plan_still_records_id() {
        let ws = tempfile::tempdir().unwrap();
        let recs = tempfile::tempdir().unwrap();
        let pending = relay_pending(ws.path(), Some("ghost"), Some("P2"));
        attribute_successor_in(recs.path(), &pending, "to-sid");
        let rec = crate::task_progress::read_in(recs.path(), "to-sid").expect("attributed");
        assert_eq!(rec.plan_id, "ghost");
        assert_eq!(rec.current_task, None);
    }

    /// Pending files written before `model`/`effort` existed must still load —
    /// an in-flight relay registered by the previous build must not be dropped
    /// on upgrade.
    #[test]
    fn legacy_pending_file_without_model_still_deserializes() {
        let (root, pdir, _cdir) = fresh_dirs("legacy");
        fs::write(
            pdir.join("s1.json"),
            r#"{"fromSessionId":"s1","workspacePath":"/ws","note":"n",
                "chainId":"c1","hop":1,"created":1000}"#,
        )
        .unwrap();
        let rec = read_pending_in(&pdir, "s1").expect("legacy record must load");
        assert_eq!(rec.model, None);
        assert_eq!(rec.effort, None);
        assert_eq!(rec.note, "n");
        let _ = fs::remove_dir_all(&root);
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
            predecessor_cwd: None,
            note: "P1-P3 已完成；P4 卡在 X".into(),
            plan_id: Some("auth-refactor".into()),
            next_task: Some("P4".into()),
            model: None,
            effort: None,
            chain_id: "c1".into(),
            hop: 2,
            created: 1,
        };
        let prompt = compose_successor_prompt(&p);
        assert!(prompt.contains("第 3 棒"));
        assert!(prompt.contains("src-sid"));
        assert!(prompt.contains("P1-P3 已完成；P4 卡在 X"));
        assert!(prompt.contains("auth-refactor"));
        assert!(prompt.contains("P4"));
        // The relay must not trust the predecessor's *claimed* verification: a
        // "tests pass / build green" line in the note could be fabricated (or
        // simply stale after the handoff), and blindly continuing would
        // propagate that unverified claim down the whole chain. The successor
        // is told to re-run before trusting.
        assert!(
            prompt.contains("重跑"),
            "successor prompt must instruct re-running the predecessor's claimed verification:\n{prompt}"
        );
        // `attribute_successor` already stamped the side-channel, so the prompt
        // must NOT ask the agent to declare focus — that ceremony is obsolete.
        assert!(
            !prompt.contains("fleet plan resume") && !prompt.contains("fleet plan start"),
            "successor is attributed by Fleet; prompt must not demand a resume:\n{prompt}"
        );

        // free-form handoff: no plan pointer, still carries the note
        let free = PendingHandoff {
            plan_id: None,
            next_task: None,
            ..p
        };
        let prompt = compose_successor_prompt(&free);
        assert!(prompt.contains("按交接信息继续完成这项工作"));
    }
}
