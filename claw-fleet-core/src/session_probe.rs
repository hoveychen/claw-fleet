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

/// Fold a tail verdict into the coarse [`TurnEnd`] carried on `SessionInfo`,
/// along the same lines [`crate::subtask_recovery::class_of`] splits a failed
/// sub-task: transient failures read as mid-turn (recoverable), account errors
/// as auth failures.
///
/// [`TurnEnd`]: crate::session::TurnEnd
pub fn turn_end_of(v: &TailVerdict) -> Option<crate::session::TurnEnd> {
    use crate::session::TurnEnd;
    Some(match v {
        TailVerdict::Ended => TurnEnd::Clean,
        TailVerdict::InFlight | TailVerdict::RateLimited | TailVerdict::ServerError => {
            TurnEnd::MidTurn
        }
        TailVerdict::Interrupted => TurnEnd::Interrupted,
        TailVerdict::Fatal(e) if crate::subtask_recovery::is_account_error(e) => {
            TurnEnd::AuthFailed
        }
        TailVerdict::Fatal(_) => TurnEnd::Failed,
        TailVerdict::Empty => return None,
    })
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
    /// Background tasks the last Stop left running (from the idle sentinel,
    /// [`crate::idle::IdleRecord::background`]), as their descriptions. Only
    /// read while the process is alive — a dead process took them with it.
    pub background_running: Vec<String>,
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
    if f.source.as_deref() == Some("dsh") && f.dsh_running == Some(false) {
        if let Some((tail, mtime_ms)) = crate::dsh_source::session_log_verdict(&effective) {
            f.tail = Some(tail);
            f.tail_mtime_ms = Some(mtime_ms);
        }
    }

    if f.source.as_deref() != Some("dsh") {
        f.proc_alive = crate::parked::session_alive(&effective);
    }
    if f.proc_alive {
        f.background_running = crate::idle::read_idle(&effective)
            .map(|r| r.background.iter().map(background_label).collect())
            .unwrap_or_default();
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

fn background_label(t: &crate::bg_guard::BackgroundTask) -> String {
    match (t.description.is_empty(), t.task_type.is_empty()) {
        (false, false) => format!("{}：{}", t.task_type, t.description),
        (false, true) => t.description.clone(),
        (true, false) => format!("{} {}", t.task_type, t.id),
        (true, true) => t.id.clone(),
    }
}

fn read_tail_verdict(path: &Path, source: Option<&str>) -> TailVerdict {
    if source == Some("codex") {
        return codex_file_verdict(path);
    }
    let lines = crate::jsonl_tail::read_tail_lines_as_json(path, TAIL_LINES).unwrap_or_default();
    claude_tail_verdict(&lines)
}

/// How the last turn of the Codex rollout at `path` ended.
///
/// Reads the tail window first; only when no turn boundary is in it and the
/// window may have been cut short does it fall back to the whole file, so a
/// long turn whose `task_started` scrolled out still reads as in flight. That
/// makes the answer identical to [`codex_tail_verdict`] over the full parse,
/// which the scan computes for rollouts it has already read in full.
pub(crate) fn codex_file_verdict(path: &Path) -> TailVerdict {
    let read_all = || -> Vec<Value> {
        crate::codex_source::read_session_content(path)
            .map(|c| {
                c.lines()
                    .filter(|l| is_codex_event_line(l))
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let is_plain = path.extension().and_then(|e| e.to_str()) == Some("jsonl");
    if !is_plain {
        // Compressed (archived rollout): no seeking, read it whole.
        return codex_tail_verdict(&read_all());
    }
    let (tail, seen) =
        crate::jsonl_tail::read_tail_lines_filtered(path, TAIL_LINES, is_codex_event_line)
            .unwrap_or_default();
    if let Some(v) = codex_boundary_verdict(&tail) {
        return v;
    }
    // Fewer lines than asked for: the tail already was the whole file.
    if seen < TAIL_LINES {
        return codex_tail_verdict(&tail);
    }
    codex_tail_verdict(&read_all())
}

/// Whether a raw rollout line is an `event_msg` record — the only kind
/// [`codex_tail_verdict`] looks at. Checked on the line's head, where Codex
/// writes `timestamp` / `ordinal` / `type`, so the large `response_item`
/// lines are never JSON-parsed. A false positive only costs a parse.
fn is_codex_event_line(line: &str) -> bool {
    let head = &line.as_bytes()[..line.len().min(160)];
    head.windows(18).any(|w| w == br#""type":"event_msg""#)
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

/// How the last turn of a Codex rollout ended, given its parsed lines (the
/// whole rollout, or a tail of it).
///
/// A tail with events but no turn boundary reads as [`TailVerdict::Ended`]:
/// older Codex builds could end a turn without writing `task_complete`, and
/// Fleet's own status pipeline likewise treats a boundary-less rollout with a
/// dead process as finished (`clamp_dead_session_status`). Current builds write
/// `task_complete` on every turn, so over a whole rollout this only applies to
/// such old files; [`codex_file_verdict`] reads the whole file before falling
/// back to it.
pub fn codex_tail_verdict(lines: &[Value]) -> TailVerdict {
    if let Some(v) = codex_boundary_verdict(lines) {
        return v;
    }
    if lines.iter().any(|v| v["type"].as_str() == Some("event_msg")) {
        TailVerdict::Ended
    } else {
        TailVerdict::Empty
    }
}

/// The verdict of the newest turn-boundary event in `lines`, if there is one.
fn codex_boundary_verdict(lines: &[Value]) -> Option<TailVerdict> {
    for v in lines.iter().rev() {
        if v["type"].as_str() != Some("event_msg") {
            continue;
        }
        let p = &v["payload"];
        match p["type"].as_str() {
            Some("task_complete") | Some("turn_complete") => {
                return Some(match p.get("error").filter(|e| !e.is_null()) {
                    Some(e) => codex_error_verdict(e),
                    None => TailVerdict::Ended,
                });
            }
            Some("turn_aborted") => return Some(TailVerdict::Interrupted),
            Some("error") | Some("stream_error") => {
                return Some(codex_error_verdict(p.get("error").unwrap_or(p)));
            }
            Some("task_started") | Some("turn_started") => return Some(TailVerdict::InFlight),
            _ => {}
        }
    }
    None
}

/// Classify a Codex turn error (`{message, codex_error_info}`). Capacity and
/// dropped-stream errors are transient — the same request goes through on a
/// retry — so they read as [`TailVerdict::ServerError`]; everything else is
/// fatal, with the text kept for [`crate::subtask_recovery::is_account_error`].
fn codex_error_verdict(e: &Value) -> TailVerdict {
    let msg = e["message"].as_str().unwrap_or_default().trim().to_string();
    let info = e["codex_error_info"].as_str().unwrap_or_default();
    let transient = info == "server_overloaded"
        || msg.starts_with("stream disconnected before completion");
    if transient {
        return TailVerdict::ServerError;
    }
    TailVerdict::Fatal(match (msg.is_empty(), info.is_empty()) {
        (false, false) => format!("{info}: {msg}"),
        (false, true) => msg,
        (true, false) => info.to_string(),
        (true, true) => "error".to_string(),
    })
}

/// How the last turn of a dsh session log ended. dsh closes every turn with a
/// `turn/end` whose `data.reason.kind` says why; a trailing `turn/start` is a
/// turn still open (or one whose server died before the restart repaired it).
///
/// `interrupted` is not the user: dsh writes it when it reopens a log whose
/// tail turn was cut off by a crash or server restart (`interruptedTurnClosers`)
/// — the process-died-mid-turn case. The user's cancel is `aborted`.
pub fn dsh_tail_verdict(lines: &[Value]) -> TailVerdict {
    for v in lines.iter().rev() {
        match v["type"].as_str() {
            Some("turn/start") => return TailVerdict::InFlight,
            Some("turn/end") => {}
            _ => continue,
        }
        let reason = &v["data"]["reason"];
        return match reason["kind"].as_str() {
            Some("aborted") => TailVerdict::Interrupted,
            Some("interrupted") => TailVerdict::InFlight,
            Some("error") => {
                let e = &reason["error"];
                let code = e["code"].as_str().unwrap_or_default();
                match code {
                    "RATE_LIMIT" => TailVerdict::RateLimited,
                    "SERVER" | "TRANSPORT" | "TIMEOUT" | "LLM_STREAM_IDLE_TIMEOUT"
                    | "STREAM_CLOSED" => {
                        TailVerdict::ServerError
                    }
                    _ => {
                        let msg = e["message"].as_str().unwrap_or_default().trim();
                        let msg = crate::process_util::truncate_chars(msg, 200);
                        TailVerdict::Fatal(match (code.is_empty(), msg.is_empty()) {
                            (false, false) => format!("{code}: {msg}"),
                            (false, true) => code.to_string(),
                            (true, _) => msg,
                        })
                    }
                }
            }
            // `completed`, `max-tokens`, and the kinds that are not failures
            // (`blocked`, `forked`) or not yet known.
            _ => TailVerdict::Ended,
        };
    }
    TailVerdict::Empty
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
        return verdict(f, Phase::Running, "上下文正在续接");
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
        // Every dsh session shares one server, so the roster's `running` bit
        // stands in for process liveness; a stopped session's log says how its
        // last turn ended. No readable log keeps the old reading (done).
        if f.dsh_running == Some(true) {
            return verdict(f, Phase::Running, "回合进行中");
        }
        if matches!(f.tail, None | Some(TailVerdict::Empty)) {
            return verdict(f, Phase::Done, "回合已结束");
        }
    }

    let quiet_ms = f.tail_mtime_ms.map(|m| now.saturating_sub(m)).unwrap_or(u64::MAX);
    let tail = f.tail.clone().unwrap_or(TailVerdict::Empty);
    if f.proc_alive {
        // Ended with background tasks still running: a headless `-p` run stays
        // alive waiting for its subagents and re-invokes the model with their
        // results; a TUI is re-invoked the same way. Another turn is coming, so
        // this Stop is not the finish — however long the quiet lasts.
        if tail == TailVerdict::Ended && !f.background_running.is_empty() {
            let n = f.background_running.len();
            let shown: Vec<&str> =
                f.background_running.iter().take(3).map(|s| s.as_str()).collect();
            return verdict(
                f,
                Phase::Running,
                format!("回合已收尾，但还有 {n} 个后台任务在跑（{}）", shown.join("；")),
            );
        }
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
    fn turn_end_matches_the_recovery_split() {
        use crate::session::TurnEnd;
        let f = |v: TailVerdict| turn_end_of(&v);
        assert_eq!(f(TailVerdict::Ended), Some(TurnEnd::Clean));
        assert_eq!(f(TailVerdict::InFlight), Some(TurnEnd::MidTurn));
        assert_eq!(f(TailVerdict::ServerError), Some(TurnEnd::MidTurn));
        assert_eq!(f(TailVerdict::RateLimited), Some(TurnEnd::MidTurn));
        assert_eq!(f(TailVerdict::Interrupted), Some(TurnEnd::Interrupted));
        assert_eq!(
            f(TailVerdict::Fatal("authentication_failed: boom".into())),
            Some(TurnEnd::AuthFailed)
        );
        assert_eq!(f(TailVerdict::Fatal("invalid_request: too long".into())), Some(TurnEnd::Failed));
        assert_eq!(f(TailVerdict::Empty), None);
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

    /// Replays session 8c57567d (2026-10-20): its 11:20:16 UTC Stop left a
    /// Codex-research subagent and a shell running while the `-p` process
    /// waited; a session watch read the three quiet minutes after it as
    /// "回合已结束（进程仍在，等待输入）" and fired at 11:21:39. The payloads
    /// below are that Stop and the real final one (12:04:02), trimmed to the
    /// fields the probe reads.
    #[test]
    fn a_stop_that_leaves_background_tasks_running_is_not_done() {
        let early = r#"{"session_id":"8c57567d-7a13-4f85-a292-bc827a777469","hook_event_name":"Stop","stop_hook_active":true,"background_tasks":[{"id":"a2cb887591e038bbd","type":"subagent","status":"running","description":"调研 Codex CLI 记忆机制","agent_type":"general-purpose"},{"id":"buxi2o9me","type":"shell","status":"running","description":"Shallow clone codex repo and list core files","command":"git clone --depth 1 https://github.com/openai/codex.git codex"}],"session_crons":[]}"#;
        let last = r#"{"session_id":"8c57567d-7a13-4f85-a292-bc827a777469","hook_event_name":"Stop","stop_hook_active":false,"background_tasks":[],"session_crons":[]}"#;
        let facts_at = |payload: &str| {
            let p = crate::bg_guard::parse_stop_payload(payload).unwrap();
            let mut f = facts();
            f.proc_alive = true;
            f.tail = Some(TailVerdict::Ended);
            f.tail_mtime_ms = Some(NOW - 97_000); // probe at 11:21:39 → 83s quiet; any ≥60s
            f.background_running = crate::bg_guard::running_tasks(&p)
                .into_iter()
                .map(background_label)
                .collect();
            f
        };
        let p = classify(&facts_at(early), NOW);
        assert_eq!(p.phase, Phase::Running, "{}", p.detail);
        assert!(p.detail.contains("2 个后台任务"), "{}", p.detail);
        assert!(p.detail.contains("subagent：调研 Codex CLI 记忆机制"), "{}", p.detail);
        assert_eq!(classify(&facts_at(last), NOW).phase, Phase::Done);

        // The record is only consulted while the process lives.
        let mut gone = facts_at(early);
        gone.proc_alive = false;
        gone.background_running.clear();
        assert_eq!(classify(&gone, NOW).phase, Phase::Done);
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

        // A stopped dsh session whose log says the turn died reads like any
        // other dead mid-turn session: blocked inside the grace window, failed
        // after it.
        f.tail = Some(TailVerdict::InFlight);
        f.tail_mtime_ms = Some(NOW - 1000);
        assert_eq!(classify(&f, NOW).phase, Phase::Blocked);
        f.tail_mtime_ms = Some(NOW - FAIL_GRACE_MS);
        assert_eq!(classify(&f, NOW).phase, Phase::Failed);
        f.tail = Some(TailVerdict::Ended);
        assert_eq!(classify(&f, NOW).phase, Phase::Done);
        f.dsh_running = Some(true);
        f.tail = Some(TailVerdict::InFlight);
        assert_eq!(classify(&f, NOW).phase, Phase::Running);
    }

    // Real-shaped Codex rollout lines (copied from ~/.codex/sessions, trimmed).
    fn cx_started() -> Value {
        json!({"timestamp":"2026-10-17T05:13:14.000Z","type":"event_msg","payload":{"type":"task_started","turn_id":"01a14847-4b12","model_context_window":258400}})
    }
    fn cx_call() -> Value {
        json!({"timestamp":"2026-10-17T05:13:20.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\"}","call_id":"call_1"}})
    }
    fn cx_tokens() -> Value {
        json!({"timestamp":"2026-10-17T05:13:21.000Z","type":"event_msg","payload":{"type":"token_count","info":null}})
    }
    fn cx_complete(error: Value) -> Value {
        json!({"timestamp":"2026-10-17T05:14:00.000Z","ordinal":232,"type":"event_msg","payload":{"type":"task_complete","turn_id":"01a14847-4b12","last_agent_message":null,"error":error,"started_at":1792228704,"completed_at":1792228720,"duration_ms":3850}})
    }

    #[test]
    fn codex_turn_end_from_real_shaped_rollouts() {
        use crate::session::TurnEnd;
        let end = |lines: &[Value]| turn_end_of(&codex_tail_verdict(lines));
        // Clean end.
        assert_eq!(
            end(&[cx_started(), cx_call(), cx_tokens(), cx_complete(Value::Null)]),
            Some(TurnEnd::Clean)
        );
        // Aborted turn (Esc / SIGINT / Fleet interrupt).
        let aborted = json!({"timestamp":"2026-10-17T09:18:40.000Z","type":"event_msg","payload":{"type":"turn_aborted","turn_id":"01a14927-c04a","reason":"interrupted","started_at":1792228704,"completed_at":1792228720,"duration_ms":16228}});
        assert_eq!(end(&[cx_started(), cx_call(), aborted]), Some(TurnEnd::Interrupted));
        // Process died mid-turn: started, tool calls, no boundary after.
        assert_eq!(
            end(&[cx_complete(Value::Null), cx_started(), cx_call(), cx_tokens()]),
            Some(TurnEnd::MidTurn)
        );
        // Server error: model at capacity is transient.
        let overloaded = json!({"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"});
        assert_eq!(codex_tail_verdict(&[cx_started(), cx_complete(overloaded.clone())]), TailVerdict::ServerError);
        assert_eq!(end(&[cx_started(), cx_complete(overloaded)]), Some(TurnEnd::MidTurn));
        let dropped = json!({"message":"stream disconnected before completion: Transport error: network error: error decoding response body","codex_error_info":"other"});
        assert_eq!(end(&[cx_started(), cx_complete(dropped)]), Some(TurnEnd::MidTurn));
        // Auth errors.
        let revoked = json!({"message":"Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.","codex_error_info":"unauthorized"});
        assert_eq!(end(&[cx_started(), cx_complete(revoked)]), Some(TurnEnd::AuthFailed));
        let credits = json!({"message":"Your workspace is out of credits. Ask your workspace owner to refill in order to continue.","codex_error_info":"usage_limit_exceeded"});
        assert_eq!(end(&[cx_started(), cx_complete(credits)]), Some(TurnEnd::AuthFailed));
        // Any other permanent error.
        let env = json!({"message":"Missing environment variable: `PROBE_KEY_NOT_SET`.","codex_error_info":"other"});
        assert_eq!(end(&[cx_started(), cx_complete(env)]), Some(TurnEnd::Failed));
    }

    /// A rollout line as Codex serializes it: `timestamp`, `type`, `payload`,
    /// in that order (`json!` would sort the keys and bury `type`).
    fn cx_line(v: &Value) -> String {
        format!(r#"{{"timestamp":{},"type":{},"payload":{}}}"#, v["timestamp"], v["type"], v["payload"])
    }

    #[test]
    fn codex_file_verdict_sees_a_turn_start_past_the_tail_window() {
        use std::io::Write as _;
        let mut f = tempfile::Builder::new().suffix(".jsonl").tempfile().unwrap();
        let mut write = |v: &Value| writeln!(f, "{}", cx_line(v)).unwrap();
        write(&cx_complete(Value::Null));
        write(&cx_started());
        for _ in 0..TAIL_LINES + 50 {
            write(&cx_call());
        }
        // The tail alone has events but no boundary — it would read as Ended.
        let tail = crate::jsonl_tail::read_tail_lines_as_json(f.path(), TAIL_LINES).unwrap();
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(codex_boundary_verdict(&tail), None);
        assert_eq!(codex_file_verdict(f.path()), TailVerdict::InFlight);

        // A short file is read whole by the tail read.
        let mut short = tempfile::Builder::new().suffix(".jsonl").tempfile().unwrap();
        writeln!(short, "{}", cx_line(&cx_started())).unwrap();
        writeln!(short, "{}", cx_line(&cx_complete(Value::Null))).unwrap();
        assert_eq!(codex_file_verdict(short.path()), TailVerdict::Ended);
    }

    #[test]
    fn dsh_turn_end_from_real_shaped_logs() {
        use crate::session::TurnEnd;
        let start = json!({"type":"turn/start","seq":40,"time":1789311190000u64,"data":{"turn":2}});
        let step = json!({"type":"step/end","seq":49,"time":1789311199755u64,"data":{"turn":2,"step":6}});
        let end_with = |reason: Value| json!({"type":"turn/end","seq":50,"time":1789311199756u64,"data":{"turn":2,"reason":reason}});
        let v = |reason: Value| dsh_tail_verdict(&[start.clone(), step.clone(), end_with(reason)]);
        let te = |reason: Value| turn_end_of(&v(reason));

        assert_eq!(te(json!({"kind":"completed"})), Some(TurnEnd::Clean));
        assert_eq!(te(json!({"kind":"aborted","reason":{"kind":"user"}})), Some(TurnEnd::Interrupted));
        // Crash / restart recovery closes the open turn as `interrupted`.
        assert_eq!(te(json!({"kind":"interrupted"})), Some(TurnEnd::MidTurn));
        assert_eq!(
            te(json!({"kind":"error","error":{"message":"DeepSeek API request to https://api.deepseek.com failed","code":"TRANSPORT"}})),
            Some(TurnEnd::MidTurn)
        );
        assert_eq!(v(json!({"kind":"error","error":{"message":"slow down","code":"RATE_LIMIT"}})), TailVerdict::RateLimited);
        assert_eq!(
            v(json!({"kind":"error","error":{"message":"SSE stream ended without [DONE]","code":"STREAM_CLOSED"}})),
            TailVerdict::ServerError
        );
        assert_eq!(
            te(json!({"kind":"error","error":{"message":"llm-deepseek: no API key for provider route deepseek-official","code":"MISSING_CREDENTIAL"}})),
            Some(TurnEnd::AuthFailed)
        );
        assert_eq!(
            te(json!({"kind":"error","error":{"message":"invalid api key","code":"AUTH"}})),
            Some(TurnEnd::AuthFailed)
        );
        assert_eq!(
            te(json!({"kind":"error","error":{"message":"balance exhausted","code":"ACCOUNT_QUOTA"}})),
            Some(TurnEnd::AuthFailed)
        );
        assert_eq!(
            v(json!({"kind":"error","error":{"message":"Cannot read properties of undefined (reading 'length')","code":"UNKNOWN"}})),
            TailVerdict::Fatal("UNKNOWN: Cannot read properties of undefined (reading 'length')".into())
        );
        assert_eq!(
            te(json!({"kind":"error","error":{"message":"Cannot read properties of undefined (reading 'length')","code":"UNKNOWN"}})),
            Some(TurnEnd::Failed)
        );
        // An open turn, and a log with no turns.
        assert_eq!(dsh_tail_verdict(&[end_with(json!({"kind":"completed"})), start.clone(), step]), TailVerdict::InFlight);
        assert_eq!(dsh_tail_verdict(&[json!({"type":"session/end-seed","seq":51,"data":{}})]), TailVerdict::Empty);
    }
}
