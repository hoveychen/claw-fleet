//! Session probe — "has session X finished?" for one session id, any engine,
//! without a full scan.
//!
//! Built for the session-target watch ([`crate::watch`]): a detached timer
//! polls every target every ~30s, and the only other way to learn a session's
//! status is `scan_all_sources`, which costs tens of seconds on a busy machine.
//! This module answers from the per-session side channels Fleet already keeps
//! — the launch note, the drive store, the card stores, the handoff chain — and
//! reads a transcript *tail* only once the session's process is gone.
//!
//! The strongest signal is the process: a session Fleet started runs as a
//! headless `-p` process that exits the moment its turn ends, so "process
//! alive" is "turn in flight". The transcript tail is consulted afterwards to
//! tell a clean finish from a rate limit, a server error, an account error or a
//! process that died mid-turn.
//!
//! Every failure verdict read off a transcript waits [`FAIL_GRACE_MS`] of
//! quiet first: a rate-limited or errored turn is usually picked up by the
//! auto-resume scheduler within one of its ticks, and between the turn ending
//! and that tick there is no intent on record yet. Reporting "failed" in that
//! window would wake a waiter for something that recovers by itself.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a failure read off the transcript must hold, with nothing on
/// record to recover it, before the probe calls it [`Phase::Failed`].
pub const FAIL_GRACE_MS: u64 = 3 * 60 * 1000;

/// A process that is alive while its transcript says the turn ended is a
/// headless run still shutting down (Stop hooks, the relay spawn) for a moment,
/// and an interactive TUI sitting at its prompt after that.
pub const ALIVE_IDLE_DONE_MS: u64 = 60 * 1000;

/// Tail window read from a transcript. Status only needs the last turn's
/// boundary; a Codex turn that scrolls its start out of the window is handled
/// by the caller (see [`codex_tail_verdict`]).
const TAIL_LINES: usize = 400;

/// Coarse state of one waited-on session.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// A turn is in flight, or something already on record will start one
    /// (a queued message, its own watch, a handoff about to spawn).
    Running,
    /// Waiting for the user: a decision card or permission prompt is open.
    Parked,
    /// Hit a recoverable failure that Fleet will retry by itself (rate limit
    /// with a resume on record, server error with retries left, a failure
    /// still inside its grace window). Keeps waiting.
    Blocked,
    /// The probe could not read the session's state this time (transcript
    /// missing, dsh server unreachable). Never counts as finished.
    Unknown,
    /// The turn ended cleanly and nothing will start another one.
    Done,
    /// The turn ended in a failure nothing on record will recover.
    Failed,
}

impl Phase {
    /// Whether this phase ends the wait on the target (one way or the other).
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed)
    }
}

/// What one probe concluded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub phase: Phase,
    /// One line for a human: why this phase. Product text (Chinese).
    pub detail: String,
    /// The session the verdict is about — the live end of the target's
    /// handoff / takeover chain, which is the target itself when it never
    /// handed off.
    pub effective_id: String,
    /// Engine of the effective session, when known (`claude` / `codex` / `dsh`).
    pub source: Option<String>,
}

/// How the last turn in a transcript tail ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TailVerdict {
    /// The last turn finished normally.
    Ended,
    /// The last turn has not reached its end (tool running, model replying, or
    /// the process died before it could finish).
    InFlight,
    /// Cut off by the user / Fleet (Esc, SIGINT, `turn_aborted`).
    Interrupted,
    RateLimited,
    /// A transient API failure Fleet's server-error retry handles.
    ServerError,
    /// A permanent API / account failure (auth, credits, invalid request).
    Fatal(String),
    /// Nothing readable in the tail.
    Empty,
}

/// Everything the classifier needs, gathered from disk / the dsh server.
/// Split out so [`classify`] is a pure function with tests.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub target_id: String,
    pub effective_id: String,
    pub source: Option<String>,
    /// The effective session registered a handoff whose successor has not
    /// spawned yet.
    pub relaying: bool,
    pub remote_disconnect: Option<String>,
    /// A transcript (or dsh roster entry) was found for the effective session.
    pub located: bool,
    /// Why locating failed, when it did.
    pub locate_error: Option<String>,
    /// A drive-store Running lease is holding the session.
    pub lease_running: bool,
    /// Drive-store intents (registered and derived) as `(mechanism, reason)`.
    pub intents: Vec<(String, String)>,
    /// A decision card / permission prompt is waiting for the user.
    pub pending_decision: bool,
    pub proc_alive: bool,
    /// dsh only: the roster's `running` bit.
    pub dsh_running: Option<bool>,
    pub tail: Option<TailVerdict>,
    /// Epoch ms the transcript was last written.
    pub tail_mtime_ms: Option<u64>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Probe one session. `target_id` must be a full session id.
pub fn probe(target_id: &str) -> Probe {
    classify(&gather(target_id), now_ms())
}

/// Shortest prefix [`resolve_id`] accepts — the 8-character short ids every
/// Fleet surface prints.
pub const MIN_PREFIX: usize = 6;

/// Turn a full id or a short-id prefix into a full session id.
///
/// A full id is accepted when any engine's store knows it. A prefix is matched
/// against the launch-note registry (every session Fleet started, any engine)
/// — a directory listing, not a scan; a session Fleet did not start has to be
/// given in full.
pub fn resolve_id(needle: &str) -> Result<String, String> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Err("会话 id 为空".to_string());
    }
    let known = |id: &str| {
        crate::launch_spec::get(id).is_some()
            || crate::session::find_session_jsonl(id).is_some()
            || crate::codex_source::find_codex_rollout(id).is_some()
    };
    if known(needle) {
        return Ok(needle.to_string());
    }
    if needle.len() < MIN_PREFIX {
        return Err(format!("会话 id「{needle}」太短，至少给 {MIN_PREFIX} 位（短 id 是 8 位）"));
    }
    let ids = crate::launch_spec::registry();
    let hits: Vec<&String> = ids.keys().filter(|id| id.starts_with(needle)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => {
            // dsh ids are `session-<uuid>`; let a bare uuid prefix find them.
            let dsh: Vec<&String> = ids
                .keys()
                .filter(|id| id.strip_prefix("session-").is_some_and(|u| u.starts_with(needle)))
                .collect();
            match dsh.as_slice() {
                [one] => Ok((*one).clone()),
                [] => Err(format!(
                    "找不到会话「{needle}」：Fleet 启动的会话里没有这个前缀；不是 Fleet 启动的会话要给完整 id"
                )),
                many => Err(ambiguous(needle, many)),
            }
        }
        many => Err(ambiguous(needle, many)),
    }
}

fn ambiguous(needle: &str, hits: &[&String]) -> String {
    let shown: Vec<&str> = hits.iter().take(5).map(|s| s.as_str()).collect();
    format!("会话前缀「{needle}」匹配到 {} 个会话（{}），请给更长的前缀", hits.len(), shown.join(", "))
}

/// Collect the facts for `target_id`, following its succession first.
pub fn gather(target_id: &str) -> Facts {
    let effective = crate::session_driver::live_end(target_id).unwrap_or_else(|| target_id.to_string());
    let mut f = Facts {
        target_id: target_id.to_string(),
        effective_id: effective.clone(),
        ..Default::default()
    };
    f.relaying = crate::handoff::read_pending(&effective).is_some();
    f.remote_disconnect = crate::remote_disconnect::read(&effective).map(|r| r.detail);

    let spec = crate::launch_spec::get(&effective);
    let mut source = spec.as_ref().and_then(|s| s.source.clone());

    let drive = crate::session_driver::snapshot(&effective);
    f.lease_running = drive.running.is_some();
    f.intents = drive
        .pending
        .iter()
        .map(|p| (p.mechanism.clone(), p.reason.clone()))
        .collect();
    f.pending_decision = crate::codex_source::session_has_pending_decision(&effective);

    // Locate the transcript: the launch note names it exactly; otherwise look
    // in each engine's store in turn.
    let mut transcript: Option<PathBuf> = spec
        .as_ref()
        .and_then(|s| s.transcript.as_deref())
        .map(PathBuf::from)
        .filter(|p| p.exists());
    if transcript.is_none() && source.as_deref() != Some("dsh") {
        if source.as_deref() != Some("codex") {
            if let Some(p) = crate::session::find_session_jsonl(&effective) {
                transcript = Some(p);
                source.get_or_insert_with(|| "claude".to_string());
            }
        }
        if transcript.is_none() && source.as_deref() != Some("claude") {
            if let Some(p) = crate::codex_source::find_codex_rollout(&effective) {
                transcript = Some(p);
                source.get_or_insert_with(|| "codex".to_string());
            }
        }
    }
    if transcript.is_none() && matches!(source.as_deref(), None | Some("dsh")) {
        match crate::dsh_source::session_running(&effective) {
            Ok(Some(running)) => {
                f.dsh_running = Some(running);
                f.located = true;
                source = Some("dsh".to_string());
            }
            Ok(None) => {}
            Err(e) => f.locate_error = Some(format!("dsh 查询失败：{e}")),
        }
    }
    f.source = source;

    if f.source.as_deref() != Some("dsh") {
        f.proc_alive = crate::parked::session_alive(&effective);
    }
    if let Some(path) = transcript {
        f.located = true;
        f.tail_mtime_ms = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        f.tail = Some(read_tail_verdict(&path, f.source.as_deref()));
    }
    if !f.located && f.locate_error.is_none() {
        f.locate_error = Some("找不到这个会话的 transcript".to_string());
    }
    f
}

fn read_tail_verdict(path: &Path, source: Option<&str>) -> TailVerdict {
    let is_plain = path.extension().and_then(|e| e.to_str()) == Some("jsonl");
    let lines: Vec<Value> = if is_plain {
        crate::jsonl_tail::read_tail_lines_as_json(path, TAIL_LINES).unwrap_or_default()
    } else {
        // Compressed (archived codex rollout): no seeking, read it whole.
        crate::codex_source::read_session_content(path)
            .map(|c| c.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default()
    };
    match source {
        Some("codex") => codex_tail_verdict(&lines),
        _ => claude_tail_verdict(&lines),
    }
}

/// Text Claude Code writes as the user turn when a request is cut off.
fn is_interrupt_record(v: &Value) -> bool {
    let content = &v["message"]["content"];
    let texts: Vec<&str> = match content {
        Value::String(s) => vec![s.as_str()],
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect(),
        _ => Vec::new(),
    };
    !texts.is_empty() && texts.iter().all(|t| t.trim_start().starts_with("[Request interrupted by user"))
}

/// How the last main-chain turn of a Claude transcript tail ended.
///
/// Claude Code flushes one content block per line and stamps every line of a
/// message with the message's final `stop_reason`, so the newest main-chain
/// `assistant` line carrying `end_turn` is a finished turn; any later `user`
/// line (a prompt or a tool result) reopens it.
pub fn claude_tail_verdict(lines: &[Value]) -> TailVerdict {
    for v in lines.iter().rev() {
        let t = v["type"].as_str();
        if !matches!(t, Some("user") | Some("assistant")) || v["isSidechain"] == true {
            continue;
        }
        if t == Some("user") {
            if is_interrupt_record(v) {
                return TailVerdict::Interrupted;
            }
            return TailVerdict::InFlight;
        }
        if v["isApiErrorMessage"] == true {
            return match v["error"].as_str() {
                Some("rate_limit") => TailVerdict::RateLimited,
                Some("server_error") => TailVerdict::ServerError,
                // The unparseable-tool-call record carries no enum and is
                // retried like a server error (see `detect_server_error`).
                None if crate::session::detect_server_error(std::slice::from_ref(v)) => {
                    TailVerdict::ServerError
                }
                other => TailVerdict::Fatal(api_error_text(v, other)),
            };
        }
        return match v["message"]["stop_reason"].as_str() {
            Some("end_turn") | Some("stop_sequence") | Some("max_tokens") | Some("refusal") => {
                TailVerdict::Ended
            }
            _ => TailVerdict::InFlight,
        };
    }
    TailVerdict::Empty
}

fn api_error_text(v: &Value, kind: Option<&str>) -> String {
    let text = v["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let text = crate::process_util::truncate_chars(text.trim(), 200);
    match (kind, text.is_empty()) {
        (Some(k), true) => k.to_string(),
        (Some(k), false) => format!("{k}: {text}"),
        (None, _) => text,
    }
}

/// How the last turn of a Codex rollout tail ended.
///
/// A turn whose start scrolled out of the tail with no boundary after it reads
/// as [`TailVerdict::Ended`]: Codex often ends a turn without writing
/// `task_complete` at all, and Fleet's own status pipeline likewise treats a
/// boundary-less rollout with a dead process as finished
/// (`clamp_dead_session_status`).
pub fn codex_tail_verdict(lines: &[Value]) -> TailVerdict {
    let mut saw_event = false;
    for v in lines.iter().rev() {
        if v["type"].as_str() != Some("event_msg") {
            continue;
        }
        saw_event = true;
        let p = &v["payload"];
        let err_text = |e: &Value| {
            let msg = e["message"].as_str().unwrap_or_default().trim().to_string();
            let info = e["codex_error_info"].as_str().unwrap_or_default();
            match (msg.is_empty(), info.is_empty()) {
                (false, false) => format!("{info}: {msg}"),
                (false, true) => msg,
                (true, false) => info.to_string(),
                (true, true) => "error".to_string(),
            }
        };
        match p["type"].as_str() {
            Some("task_complete") | Some("turn_complete") => {
                return match p.get("error").filter(|e| !e.is_null()) {
                    Some(e) => TailVerdict::Fatal(err_text(e)),
                    None => TailVerdict::Ended,
                };
            }
            Some("turn_aborted") => return TailVerdict::Interrupted,
            Some("error") | Some("stream_error") => {
                let e = p.get("error").unwrap_or(p);
                return TailVerdict::Fatal(err_text(e));
            }
            Some("task_started") | Some("turn_started") => return TailVerdict::InFlight,
            _ => {}
        }
    }
    if saw_event {
        TailVerdict::Ended
    } else {
        TailVerdict::Empty
    }
}

fn verdict(f: &Facts, phase: Phase, detail: impl Into<String>) -> Probe {
    Probe {
        phase,
        detail: detail.into(),
        effective_id: f.effective_id.clone(),
        source: f.source.clone(),
    }
}

/// Turn gathered facts into a verdict. Ordered from the side channels that
/// override everything (a pending relay, a lost remote) down to the transcript.
pub fn classify(f: &Facts, now: u64) -> Probe {
    if f.relaying {
        return verdict(f, Phase::Running, "已登记接力，等后继会话起来");
    }
    if let Some(d) = &f.remote_disconnect {
        return verdict(f, Phase::Failed, format!("远端工作区断开：{d}"));
    }
    if !f.located {
        return verdict(
            f,
            Phase::Unknown,
            f.locate_error.clone().unwrap_or_else(|| "读不到会话状态".to_string()),
        );
    }
    let has = |m: &str| f.intents.iter().find(|(mech, _)| mech == m);
    if f.pending_decision || has("parked_card").is_some() {
        return verdict(f, Phase::Parked, "在等老板答复决策卡");
    }
    if f.lease_running {
        return verdict(f, Phase::Running, "回合进行中");
    }
    if let Some((_, reason)) = has("auto_resume") {
        return verdict(f, Phase::Blocked, format!("被限流，Fleet 会自动恢复（{reason}）"));
    }
    if has("server_error_retry").is_some() {
        return verdict(f, Phase::Blocked, "API 服务端出错，Fleet 会自动重试");
    }
    if let Some((_, reason)) = has("watch") {
        return verdict(f, Phase::Running, format!("它自己也在等待（{reason}）"));
    }
    if has("pending_message").is_some() {
        return verdict(f, Phase::Running, "有排队的消息待投递");
    }
    if let Some((mech, reason)) = f.intents.first() {
        return verdict(f, Phase::Running, format!("{mech}: {reason}"));
    }

    if f.source.as_deref() == Some("dsh") {
        return match f.dsh_running {
            Some(true) => verdict(f, Phase::Running, "回合进行中"),
            _ => verdict(f, Phase::Done, "回合已结束"),
        };
    }

    let quiet_ms = f.tail_mtime_ms.map(|m| now.saturating_sub(m)).unwrap_or(u64::MAX);
    let tail = f.tail.clone().unwrap_or(TailVerdict::Empty);
    if f.proc_alive {
        // Alive and ended: a headless run on its way out, or a TUI at its
        // prompt. Only the second outlasts the window.
        if tail == TailVerdict::Ended && quiet_ms >= ALIVE_IDLE_DONE_MS {
            return verdict(f, Phase::Done, "回合已结束（进程仍在，等待输入）");
        }
        return verdict(f, Phase::Running, "回合进行中");
    }
    // Process gone. A failure counts only once it has held for the grace
    // window with nothing on record to recover it.
    let failed_or_wait = |what: String| {
        if quiet_ms >= FAIL_GRACE_MS {
            verdict(f, Phase::Failed, what)
        } else {
            verdict(f, Phase::Blocked, format!("{what}（观察中，暂不判定失败）"))
        }
    };
    match tail {
        TailVerdict::Ended => verdict(f, Phase::Done, "回合已结束"),
        TailVerdict::Interrupted => failed_or_wait("回合被中断".to_string()),
        TailVerdict::InFlight => failed_or_wait("进程在回合中途退出".to_string()),
        TailVerdict::RateLimited => failed_or_wait("被限流，且没有登记自动恢复".to_string()),
        TailVerdict::ServerError => failed_or_wait("API 服务端出错，自动重试已用尽或未登记".to_string()),
        TailVerdict::Fatal(e) => verdict(f, Phase::Failed, format!("API 错误：{e}")),
        TailVerdict::Empty => {
            if quiet_ms >= FAIL_GRACE_MS {
                verdict(f, Phase::Unknown, "transcript 里没有可读的回合记录")
            } else {
                verdict(f, Phase::Running, "会话刚启动")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 10_000_000_000;

    fn facts() -> Facts {
        Facts {
            target_id: "t".into(),
            effective_id: "t".into(),
            source: Some("claude".into()),
            located: true,
            tail: Some(TailVerdict::Ended),
            tail_mtime_ms: Some(NOW - 10 * 60 * 1000),
            ..Default::default()
        }
    }

    fn asst(stop: &str) -> Value {
        json!({"type":"assistant","message":{"stop_reason":stop,"content":[{"type":"text","text":"hi"}]}})
    }

    #[test]
    fn claude_tail_reads_the_last_main_chain_turn() {
        let user = json!({"type":"user","message":{"content":"go"}});
        assert_eq!(claude_tail_verdict(&[user.clone(), asst("end_turn")]), TailVerdict::Ended);
        assert_eq!(claude_tail_verdict(&[asst("end_turn"), user.clone()]), TailVerdict::InFlight);
        assert_eq!(claude_tail_verdict(&[user.clone(), asst("tool_use")]), TailVerdict::InFlight);
        // A sidechain line after the end is a subagent's, not the main turn.
        let side = json!({"type":"user","isSidechain":true,"message":{"content":"x"}});
        assert_eq!(claude_tail_verdict(&[asst("end_turn"), side]), TailVerdict::Ended);
        let interrupt = json!({"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}});
        assert_eq!(claude_tail_verdict(&[asst("tool_use"), interrupt]), TailVerdict::Interrupted);
        assert_eq!(claude_tail_verdict(&[]), TailVerdict::Empty);
    }

    #[test]
    fn claude_tail_classifies_api_errors() {
        let err = |e: &str| json!({"type":"assistant","isApiErrorMessage":true,"error":e,"message":{"content":[{"type":"text","text":"boom"}]}});
        assert_eq!(claude_tail_verdict(&[err("rate_limit")]), TailVerdict::RateLimited);
        assert_eq!(claude_tail_verdict(&[err("server_error")]), TailVerdict::ServerError);
        assert_eq!(
            claude_tail_verdict(&[err("authentication_failed")]),
            TailVerdict::Fatal("authentication_failed: boom".into())
        );
    }

    #[test]
    fn codex_tail_reads_turn_boundaries() {
        let ev = |t: &str| json!({"type":"event_msg","payload":{"type":t}});
        assert_eq!(codex_tail_verdict(&[ev("task_started"), ev("task_complete")]), TailVerdict::Ended);
        assert_eq!(codex_tail_verdict(&[ev("task_complete"), ev("task_started")]), TailVerdict::InFlight);
        assert_eq!(codex_tail_verdict(&[ev("turn_aborted")]), TailVerdict::Interrupted);
        let credits = json!({"type":"event_msg","payload":{"type":"task_complete","error":{"message":"out of credits","codex_error_info":"usage_limit_exceeded"}}});
        assert_eq!(
            codex_tail_verdict(&[credits]),
            TailVerdict::Fatal("usage_limit_exceeded: out of credits".into())
        );
        // Boundary-less tail with events: Codex skipped task_complete.
        assert_eq!(codex_tail_verdict(&[ev("token_count")]), TailVerdict::Ended);
        assert_eq!(codex_tail_verdict(&[json!({"type":"response_item"})]), TailVerdict::Empty);
    }

    #[test]
    fn a_clean_dead_session_is_done() {
        assert_eq!(classify(&facts(), NOW).phase, Phase::Done);
    }

    #[test]
    fn side_channels_outrank_the_transcript() {
        let mut f = facts();
        f.relaying = true;
        assert_eq!(classify(&f, NOW).phase, Phase::Running);

        let mut f = facts();
        f.remote_disconnect = Some("ssh lost".into());
        assert_eq!(classify(&f, NOW).phase, Phase::Failed);

        let mut f = facts();
        f.pending_decision = true;
        assert_eq!(classify(&f, NOW).phase, Phase::Parked);

        let mut f = facts();
        f.intents = vec![("watch".into(), "watch abc is armed".into())];
        assert_eq!(classify(&f, NOW).phase, Phase::Running, "a target that is itself waiting is not done");

        let mut f = facts();
        f.tail = Some(TailVerdict::RateLimited);
        f.intents = vec![("auto_resume".into(), "rate limit resets at x".into())];
        assert_eq!(classify(&f, NOW).phase, Phase::Blocked);
    }

    #[test]
    fn recoverable_failures_wait_out_the_grace_window() {
        for tail in [TailVerdict::RateLimited, TailVerdict::ServerError, TailVerdict::InFlight, TailVerdict::Interrupted] {
            let mut f = facts();
            f.tail = Some(tail.clone());
            f.tail_mtime_ms = Some(NOW - 30_000);
            assert_eq!(classify(&f, NOW).phase, Phase::Blocked, "{tail:?} inside grace");
            f.tail_mtime_ms = Some(NOW - FAIL_GRACE_MS);
            assert_eq!(classify(&f, NOW).phase, Phase::Failed, "{tail:?} past grace");
        }
        let mut f = facts();
        f.tail = Some(TailVerdict::Fatal("authentication_failed".into()));
        f.tail_mtime_ms = Some(NOW - 1_000);
        assert_eq!(classify(&f, NOW).phase, Phase::Failed, "a fatal error needs no grace");
    }

    #[test]
    fn a_live_process_is_running_until_it_idles_at_a_prompt() {
        let mut f = facts();
        f.proc_alive = true;
        f.tail = Some(TailVerdict::InFlight);
        assert_eq!(classify(&f, NOW).phase, Phase::Running);
        f.tail = Some(TailVerdict::Ended);
        f.tail_mtime_ms = Some(NOW - 5_000);
        assert_eq!(classify(&f, NOW).phase, Phase::Running);
        f.tail_mtime_ms = Some(NOW - ALIVE_IDLE_DONE_MS);
        assert_eq!(classify(&f, NOW).phase, Phase::Done);
    }

    #[test]
    fn unlocated_and_dsh_sessions() {
        let mut f = facts();
        f.located = false;
        f.locate_error = Some("dsh 查询失败：down".into());
        let p = classify(&f, NOW);
        assert_eq!(p.phase, Phase::Unknown);
        assert!(p.detail.contains("down"));

        let mut f = facts();
        f.source = Some("dsh".into());
        f.tail = None;
        f.dsh_running = Some(true);
        assert_eq!(classify(&f, NOW).phase, Phase::Running);
        f.dsh_running = Some(false);
        assert_eq!(classify(&f, NOW).phase, Phase::Done);
    }
}
