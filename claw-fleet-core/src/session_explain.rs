//! Side questions about a session's own transcript, answered in a **fork**.
//!
//! The reader of a long agent reply often wants one thing clarified — what a
//! sentence means, why a trade-off went that way, a translation — without
//! appending a follow-up to the real conversation (which would pollute the
//! agent's context and cost a full agentic turn). This module answers such a
//! question in a throwaway fork of the session:
//!
//! - **Fork, never a fresh session.** The fork replays the whole history, so
//!   the request prefix is byte-identical to the session's own requests and
//!   hits the provider's prompt cache. Measured 2026-09-20 on an 80K-token
//!   Claude session: a fresh-context paste costs $1.61, the fork $0.07.
//!   Whether the prefix really matches is the source's job
//!   ([`crate::agent_source::AgentSource::fork_ask`]); for Claude the flags
//!   that govern it are mirrored from the original launch — see
//!   [`claude_fork_ask`].
//! - **Nothing lands in the source transcript.** The fork is either not
//!   persisted at all (Claude) or persisted under its own identity that the
//!   scanners hide ([`ForkAskOutcome::fork_session_id`]).
//! - **One turn, text only.** The prompt says so, the source enforces what it
//!   can (`--max-turns 1`), and a turn that produced no text is reported as an
//!   error rather than an empty answer.
//!
//! Records live at `~/.fleet/explain/<session_id>/<explain_id>.json`. A record
//! is written the moment the question is accepted (`status: running`) and
//! rewritten as the answer streams in, so clients on every surface show the
//! answer progressively by polling [`get`] — there is no second streaming
//! channel to wire. Spend is logged to [`crate::llm_usage`] under
//! [`crate::llm_usage::SCENARIO_SESSION_EXPLAIN`] because the fork leaves no
//! transcript for the usage views to fold.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_source::{ForkAskOutcome, ForkAskSpec};
use crate::model_cost::TurnUsage;

/// Hard ceiling on one forked answer, from spawn to final record. Probes ran
/// 8–11 s; a cold cache adds a few seconds; anything past this is a hung
/// harness, not a slow model.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the streaming answer is flushed to disk while text arrives.
const FLUSH_EVERY: Duration = Duration::from_millis(120);

/// `CLAUDE_CODE_ENTRYPOINT` for a forked Claude process whose source session
/// recorded no entrypoint of its own. Normally the fork **mirrors the
/// session's entrypoint** instead: Claude Code writes the entrypoint into the
/// first system block (`x-anthropic-billing-header: … cc_entrypoint=…`), which
/// sits ahead of every cache breakpoint, so a different value here is a
/// different prefix and a guaranteed cache miss. Captured 2026-09-20 by
/// diffing the request bodies of a hitting and a missing fork: that header
/// was the only pre-message difference.
pub const FORK_ENTRYPOINT_FALLBACK: &str = "fleet-explain";

// ── Wire types ───────────────────────────────────────────────────────────────

/// The canned questions the selection toolbar offers.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExplainPreset {
    /// "What does this mean?"
    Explain,
    /// Translate the passage (zh ↔ en).
    Translate,
    /// "Why did you decide this? What were the alternatives?"
    Rationale,
    /// The user typed their own question.
    Custom,
}

/// Where in the transcript the quoted passage came from, so a client can scroll
/// back to it. Both fields optional: the uuid is the durable key, the index a
/// fallback for records without one.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplainAnchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_idx: Option<u64>,
}

/// A side question as a client submits it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplainRequest {
    /// Session id of the conversation being asked about.
    pub session_id: String,
    /// The session's path/URI exactly as the client holds it
    /// (`SessionInfo::jsonl_path`): bare path for Claude, `codex://…`,
    /// `dsh://…`. Picks the source that owns the session.
    pub session_path: String,
    /// The session's workspace. Optional for Claude sessions (resolved from
    /// the transcript when absent); required for the other sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    /// The selected passage, verbatim.
    pub quote: String,
    pub preset: ExplainPreset,
    /// The user's own question; required for `Custom`, ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ExplainAnchor>,
    /// Earlier explanations (their ids) this question continues. Their Q/A is
    /// folded into the prompt so a follow-up still runs as a single fork of
    /// the *session* — the fork itself is never resumed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thread: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExplainStatus {
    Running,
    Done,
    Error,
}

/// One side question and its answer, as persisted and as served.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExplainRecord {
    pub id: String,
    pub session_id: String,
    /// `AgentSource::name()` of the source that answered.
    pub source: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub preset: ExplainPreset,
    pub quote: String,
    /// The question actually asked (the preset's text, or the custom one).
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ExplainAnchor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thread: Vec<String>,
    pub status: ExplainStatus,
    /// The answer so far (grows while `Running`; final when `Done`).
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub duration_ms: u64,
    /// The fork's persisted identity when the source had to leave one on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_session_id: Option<String>,
}

// ── Storage ──────────────────────────────────────────────────────────────────

fn explain_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("explain"))
}

fn safe_component(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && !s.contains('\\') && !s.contains("..")
}

fn record_path_in(root: &Path, session_id: &str, id: &str) -> Option<PathBuf> {
    if !safe_component(session_id) || !safe_component(id) {
        return None;
    }
    Some(root.join(session_id).join(format!("{id}.json")))
}

fn write_record_in(root: &Path, rec: &ExplainRecord) -> Result<(), String> {
    let path = record_path_in(root, &rec.session_id, &rec.id)
        .ok_or_else(|| "invalid session or record id".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(rec).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

fn write_record(rec: &ExplainRecord) -> Result<(), String> {
    let root = explain_dir().ok_or_else(|| "no home dir".to_string())?;
    write_record_in(&root, rec)
}

pub fn get_in(root: &Path, session_id: &str, id: &str) -> Option<ExplainRecord> {
    let path = record_path_in(root, session_id, id)?;
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// One record, or `None` when it does not exist (or the ids are malformed).
pub fn get(session_id: &str, id: &str) -> Option<ExplainRecord> {
    get_in(&explain_dir()?, session_id, id)
}

pub fn list_in(root: &Path, session_id: &str) -> Vec<ExplainRecord> {
    if !safe_component(session_id) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(root.join(session_id)) else {
        return Vec::new();
    };
    let mut out: Vec<ExplainRecord> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|r| (r.created_ms, r.id.clone()));
    out
}

/// Every explanation recorded for `session_id`, oldest first.
pub fn list(session_id: &str) -> Vec<ExplainRecord> {
    explain_dir().map(|d| list_in(&d, session_id)).unwrap_or_default()
}

/// Every fork identity any explanation left on disk, across all sessions.
/// Scanners consult this to keep dsh child sessions / codex rollout copies out
/// of the session lists.
pub fn fork_session_ids() -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(root) = explain_dir() else {
        return out;
    };
    let Ok(sessions) = fs::read_dir(&root) else {
        return out;
    };
    for s in sessions.filter_map(|e| e.ok()) {
        let Some(name) = s.file_name().to_str().map(str::to_string) else {
            continue;
        };
        for rec in list_in(&root, &name) {
            if let Some(id) = rec.fork_session_id {
                out.insert(id);
            }
        }
    }
    out
}

// ── Prompt ───────────────────────────────────────────────────────────────────

fn preset_question(preset: ExplainPreset, custom: Option<&str>) -> String {
    match preset {
        ExplainPreset::Explain => "这段话是什么意思？请用通俗的话解释它在本任务里的含义。".to_string(),
        ExplainPreset::Translate => {
            "请把这段话翻译成中文（若原文已是中文则译为英文），保持术语准确，只给译文。".to_string()
        }
        ExplainPreset::Rationale => {
            "你为什么这么判断 / 这么做？说明背后的取舍、被放弃的备选方案，以及你当时的不确定之处。".to_string()
        }
        ExplainPreset::Custom => custom.map(str::trim).filter(|q| !q.is_empty()).unwrap_or("请解释这段话。").to_string(),
    }
}

/// The single-turn prompt handed to the fork. `prior` is the follow-up chain
/// (oldest first) already folded in as context; the fork itself is never
/// resumed, so this is the only way a follow-up sees its predecessors.
pub fn build_prompt(quote: &str, question: &str, prior: &[ExplainRecord]) -> String {
    let mut out = String::new();
    out.push_str(
        "这是老板对你上面回复中某段文字的旁路追问，不会进入主对话，也不会有后续回合。\n\
         请不要调用任何工具、不要发决策卡、不要建议下一步，直接用文字回答；篇幅精炼，够解释清楚即可。\n\n",
    );
    out.push_str("【原文】\n");
    for line in quote.trim().lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
    if !prior.is_empty() {
        out.push_str("\n【此前围绕这段原文的追问】\n");
        for (i, p) in prior.iter().enumerate() {
            out.push_str(&format!("Q{}: {}\nA{}: {}\n", i + 1, p.question.trim(), i + 1, p.text.trim()));
        }
    }
    out.push_str("\n【问题】\n");
    out.push_str(question.trim());
    out.push('\n');
    out
}

// ── Asking ───────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Accept a side question: persist a `running` record, start answering on a
/// background thread, and return the record immediately. Clients poll [`get`]
/// until `status` leaves `Running`.
pub fn ask(req: ExplainRequest) -> Result<ExplainRecord, String> {
    if req.quote.trim().is_empty() {
        return Err("nothing selected".to_string());
    }
    if !safe_component(&req.session_id) {
        return Err("invalid session id".to_string());
    }
    let sources = crate::agent_source::build_sources();
    let source = crate::agent_source::find_source_for_path(&sources, &req.session_path)
        .ok_or_else(|| format!("no agent source owns {}", req.session_path))?;
    let source_name = source.name().to_string();

    let workspace_path = match req.workspace_path.clone().filter(|w| !w.trim().is_empty()) {
        Some(w) => w,
        None => crate::session::resolve_session_cwd(&req.session_id)
            .ok_or_else(|| format!("cannot resolve the workspace of session {}", req.session_id))?,
    };

    let prior: Vec<ExplainRecord> = req
        .thread
        .iter()
        .filter_map(|id| get(&req.session_id, id))
        .filter(|r| r.status == ExplainStatus::Done)
        .collect();
    let question = preset_question(req.preset, req.question.as_deref());
    let prompt = build_prompt(&req.quote, &question, &prior);

    let now = now_ms();
    let rec = ExplainRecord {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: req.session_id.clone(),
        source: source_name,
        created_ms: now,
        updated_ms: now,
        preset: req.preset,
        quote: req.quote.clone(),
        question,
        anchor: req.anchor.clone(),
        thread: req.thread.clone(),
        status: ExplainStatus::Running,
        text: String::new(),
        error: None,
        model: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cost_usd: None,
        duration_ms: 0,
        fork_session_id: None,
    };
    write_record(&rec)?;

    let spec = ForkAskSpec {
        session_id: req.session_id.clone(),
        workspace_path,
        prompt,
    };
    let worker_rec = rec.clone();
    std::thread::Builder::new()
        .name(format!("explain-{}", &rec.id[..8]))
        .spawn(move || run(worker_rec, spec))
        .map_err(|e| format!("spawn explain worker: {e}"))?;
    Ok(rec)
}

/// The worker: drive the source's fork, stream text into the record, settle
/// it as `Done`/`Error`, account the spend.
fn run(mut rec: ExplainRecord, spec: ForkAskSpec) {
    let started = Instant::now();
    // Sources are rebuilt on the worker rather than moved across the thread:
    // `build_sources` is cheap and `dyn AgentSource` is not `'static`-borrowable
    // from `ask`'s stack frame.
    let sources = crate::agent_source::build_sources();
    let Some(source) = sources.iter().find(|s| s.name() == rec.source) else {
        let msg = format!("agent source {} vanished", rec.source);
        settle_error(&mut rec, msg, started);
        return;
    };

    let mut last_flush = Instant::now() - FLUSH_EVERY;
    let mut streamed = String::new();
    let mut on_delta = |delta: &str| {
        streamed.push_str(delta);
        if last_flush.elapsed() >= FLUSH_EVERY {
            rec.text = streamed.clone();
            rec.updated_ms = now_ms();
            let _ = write_record(&rec);
            last_flush = Instant::now();
        }
    };
    let outcome = source.fork_ask(&spec, &mut on_delta);
    drop(on_delta);

    match outcome {
        Ok(out) => {
            let text = if out.text.trim().is_empty() { streamed.clone() } else { out.text.clone() };
            if text.trim().is_empty() {
                settle_error(
                    &mut rec,
                    "the fork produced no text (it may have tried to call a tool); try again".to_string(),
                    started,
                );
                return;
            }
            rec.text = text;
            rec.model = out.model.clone();
            rec.fork_session_id = out.fork_session_id.clone();
            if let Some(u) = out.usage {
                rec.input_tokens = u.input_tokens;
                rec.output_tokens = u.output_tokens;
                rec.cache_read_tokens = u.cache_read_tokens;
                rec.cache_creation_tokens = u.cache_creation_tokens;
            }
            let priced = out.cost_usd.or_else(|| {
                let (Some(m), Some(u)) = (out.model.as_deref(), out.usage.as_ref()) else {
                    return None;
                };
                (rec.source == "claude-code").then(|| crate::model_cost::turn_cost_usd(m, u))
            });
            rec.cost_usd = priced;
            rec.status = ExplainStatus::Done;
            rec.error = None;
            rec.duration_ms = started.elapsed().as_millis() as u64;
            rec.updated_ms = now_ms();
            let _ = write_record(&rec);
            account(&rec, out.usage.as_ref());
        }
        Err(e) => settle_error(&mut rec, e, started),
    }
}

fn settle_error(rec: &mut ExplainRecord, err: String, started: Instant) {
    crate::log_debug(&format!("[session_explain] {} failed: {err}", rec.id));
    rec.status = ExplainStatus::Error;
    rec.error = Some(err);
    rec.duration_ms = started.elapsed().as_millis() as u64;
    rec.updated_ms = now_ms();
    let _ = write_record(rec);
}

fn account(rec: &ExplainRecord, usage: Option<&TurnUsage>) {
    let provider = match rec.source.as_str() {
        "claude-code" => "claude",
        other => other,
    };
    let entry = crate::llm_usage::FleetLlmUsageEntry {
        timestamp_ms: rec.updated_ms,
        scenario: crate::llm_usage::SCENARIO_SESSION_EXPLAIN.to_string(),
        provider: provider.to_string(),
        model: rec.model.clone().unwrap_or_default(),
        input_tokens: rec.input_tokens,
        output_tokens: rec.output_tokens,
        cache_creation_tokens: rec.cache_creation_tokens,
        cache_creation_1h_tokens: usage.map(|u| u.cache_creation_1h_tokens).unwrap_or(0),
        cache_read_tokens: rec.cache_read_tokens,
        duration_ms: rec.duration_ms,
        cost_usd: rec.cost_usd.unwrap_or(0.0),
        token_accurate: usage.is_some(),
        cost_accurate: rec.cost_usd.is_some(),
    };
    crate::llm_usage::append_usage_entry(&entry);
}

// ── Claude backend ───────────────────────────────────────────────────────────

/// What the Claude fork must be launched with, given how the session itself
/// was launched. Pure so the cache-governing flags can be asserted.
///
/// Measured 2026-09-20 (claude 2.1.263) on an 80K–150K token session:
/// - `--permission-prompt-tool` shapes the system prompt: without it the fork
///   read only the tools block (13.9K) from cache and wrote the rest fresh.
/// - `--thinking-display summarized` (part of Fleet's stream args) shapes the
///   request's thinking config; a thinking-config change invalidates the
///   *message* cache while tools/system stay cached — the first end-to-end
///   run omitted it and read 15.5K of 162K, at $2.98 instead of $0.09.
/// - `--model` must match, else the prefix is a different model's.
/// - `--permission-mode`, `--max-turns` and `--no-session-persistence` do not
///   enter the prefix.
/// Chat-workspace sessions add their settings/MCP flags for the same reason
/// (they shape the system prompt). The stream flags are taken from
/// `session_launch::live_thinking_stream_args` so the fork tracks whatever
/// the real launches use, rather than a second hand-maintained list.
pub fn claude_fork_args(
    session_id: &str,
    fork_session_id: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    stream_args: Vec<String>,
    permission_prompt_tool_args: Vec<String>,
    chat_args: Vec<String>,
) -> Vec<String> {
    let mut args = vec![
        "--resume".to_string(),
        session_id.to_string(),
        "--fork-session".to_string(),
        // Name the fork's identity up front instead of letting the CLI mint
        // one: `fleet mcp` decides which tools to advertise by looking the
        // child's `CLAUDE_CODE_SESSION_ID` up in `launch_spec`, so the id has
        // to be recorded there *before* the process starts. See
        // `claude_fork_ask` for the cache consequence.
        "--session-id".to_string(),
        fork_session_id.to_string(),
        "--no-session-persistence".to_string(),
        "--max-turns".to_string(),
        "1".to_string(),
        "-p".to_string(),
        prompt.to_string(),
    ];
    args.extend(stream_args);
    // `push_session_override_args` cannot fail without a permission mode.
    let _ = crate::session_launch::push_session_override_args(&mut args, model, effort, None);
    args.extend(permission_prompt_tool_args);
    args.extend(chat_args);
    args
}

/// Fold of a `claude -p --output-format stream-json` stdout, one line at a
/// time. Text deltas are surfaced as they arrive; the final `result` event
/// carries the authoritative text, usage and cost.
#[derive(Default, Debug)]
pub struct ClaudeStreamFold {
    pub streamed: String,
    pub model: Option<String>,
    pub result_text: Option<String>,
    pub usage: Option<TurnUsage>,
    pub cost_usd: Option<f64>,
    pub is_error: bool,
    pub error_text: Option<String>,
}

impl ClaudeStreamFold {
    /// Feed one stdout line. Returns the text delta it carried, if any.
    pub fn feed(&mut self, line: &str) -> Option<String> {
        let v: Value = serde_json::from_str(line.trim()).ok()?;
        match v.get("type").and_then(Value::as_str)? {
            "stream_event" => {
                let ev = v.get("event")?;
                match ev.get("type").and_then(Value::as_str)? {
                    "message_start" => {
                        if let Some(m) = ev.pointer("/message/model").and_then(Value::as_str) {
                            self.model = Some(m.to_string());
                        }
                        None
                    }
                    "content_block_delta" => {
                        let d = ev.get("delta")?;
                        if d.get("type").and_then(Value::as_str) != Some("text_delta") {
                            return None;
                        }
                        let t = d.get("text").and_then(Value::as_str)?.to_string();
                        self.streamed.push_str(&t);
                        Some(t)
                    }
                    _ => None,
                }
            }
            "result" => {
                // A resumed fork can emit a spurious zero-turn `result` first
                // (hook / notification turns that produced nothing). Only a
                // result with real content or a real error is the answer.
                let turns = v.get("num_turns").and_then(Value::as_u64).unwrap_or(0);
                let text = v.get("result").and_then(Value::as_str).unwrap_or("");
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                if turns == 0 && text.is_empty() && !is_error {
                    return None;
                }
                self.is_error = is_error;
                if is_error {
                    self.error_text = Some(if text.is_empty() { "claude reported an error".to_string() } else { text.to_string() });
                } else {
                    self.result_text = Some(text.to_string());
                }
                if let Some(u) = v.get("usage") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    self.usage = Some(TurnUsage {
                        input_tokens: n("input_tokens"),
                        output_tokens: n("output_tokens"),
                        cache_creation_tokens: n("cache_creation_input_tokens"),
                        cache_creation_1h_tokens: u
                            .pointer("/cache_creation/ephemeral_1h_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_read_tokens: n("cache_read_input_tokens"),
                        web_search_requests: u
                            .pointer("/server_tool_use/web_search_requests")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    });
                }
                if let Some(c) = v.get("total_cost_usd").and_then(Value::as_f64) {
                    self.cost_usd = Some(c);
                }
                if self.model.is_none() {
                    if let Some(mu) = v.get("modelUsage").and_then(Value::as_object) {
                        self.model = mu.keys().next().cloned();
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn into_outcome(self) -> Result<ForkAskOutcome, String> {
        if self.is_error {
            return Err(self.error_text.unwrap_or_else(|| "claude reported an error".to_string()));
        }
        let text = self.result_text.filter(|t| !t.trim().is_empty()).unwrap_or(self.streamed);
        Ok(ForkAskOutcome {
            text,
            model: self.model,
            usage: self.usage,
            cost_usd: self.cost_usd,
            fork_session_id: None,
        })
    }
}

/// Drops the transient `launch_spec` note of a fork identity when the fork is
/// over, whichever way it ended.
struct ForgetLaunchSpec(String);

impl Drop for ForgetLaunchSpec {
    fn drop(&mut self) {
        crate::launch_spec::forget(&self.0);
    }
}

fn claude_stderr_log() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("session_explain_stderr.log"))
}

/// Fork a Claude Code session for one answer. See [`claude_fork_args`] for the
/// flags and why; this function owns the process: piped stdout folded line by
/// line, stderr appended to `~/.fleet/session_explain_stderr.log`, a watchdog
/// that kills the child at [`ANSWER_TIMEOUT`].
pub(crate) fn claude_fork_ask(
    spec: &ForkAskSpec,
    on_delta: &mut dyn FnMut(&str),
) -> Result<ForkAskOutcome, String> {
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("claude CLI not found".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    if !Path::new(&spec.workspace_path).is_dir() {
        return Err(format!("workspace directory not found: {}", spec.workspace_path));
    }

    let model = crate::session::resolve_session_model_spec(&spec.session_id);
    let effort = crate::launch_spec::effort_of(&spec.session_id);

    // The fork must look Fleet-owned to `fleet mcp`, or the prompt cache is
    // lost from the first message on. `fleet mcp` advertises its 12 control
    // tools only to sessions with a `launch_spec` note; the source session has
    // one, a CLI-minted fork id does not, so the fork's MCP tool set — and with
    // it the deferred-tools listing Claude Code writes into the conversation
    // — differed from the source's. Measured 2026-09-20 on a 58K-token
    // session: a fork without the note read 15.5K (tools + system only) at
    // $0.89; the next fork *with* the note read 58.5K at $0.04. The note is a
    // lie about persistence (the fork writes no transcript), so it is dropped
    // again the moment the process ends — on every exit path, via the guard.
    let fork_session_id = uuid::Uuid::new_v4().to_string();
    crate::launch_spec::record(&fork_session_id, model.as_deref(), effort.as_deref());
    let _forget = ForgetLaunchSpec(fork_session_id.clone());

    let args = claude_fork_args(
        &spec.session_id,
        &fork_session_id,
        &spec.prompt,
        model.as_deref(),
        effort.as_deref(),
        crate::session_launch::live_thinking_stream_args(),
        crate::session_launch::permission_prompt_tool_args(),
        crate::chat_workspace::chat_launch_args(&spec.workspace_path),
    );

    let stderr = match claude_stderr_log() {
        Some(p) => {
            if let Some(parent) = p.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::OpenOptions::new().create(true).append(true).open(&p) {
                Ok(mut f) => {
                    // Log the argv minus the prompt: when a fork misses the
                    // cache, the flags are the first thing to compare against
                    // the session's own launch.
                    let flags: Vec<&str> = {
                        let mut v = Vec::new();
                        let mut skip = false;
                        for a in &args {
                            if skip {
                                skip = false;
                                continue;
                            }
                            if a == "-p" {
                                skip = true;
                                continue;
                            }
                            v.push(a.as_str());
                        }
                        v
                    };
                    let _ = writeln!(
                        f,
                        "[{}] fork session={} cwd={} entrypoint={} flags={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        spec.session_id,
                        spec.workspace_path,
                        crate::session::session_entrypoint(&spec.session_id).unwrap_or_default(),
                        flags.join(" ")
                    );
                    std::process::Stdio::from(f)
                }
                Err(_) => std::process::Stdio::null(),
            }
        }
        None => std::process::Stdio::null(),
    };

    let mut cmd = crate::process_util::command(&claude);
    cmd.args(&args)
        .current_dir(&spec.workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(stderr);
    crate::session_launch::strip_inherited_agent_env(&mut cmd);
    if let Some(home) = crate::session_launch::spawn_home_dir() {
        cmd.env("HOME", home);
    }
    let entrypoint = crate::session::session_entrypoint(&spec.session_id)
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| FORK_ENTRYPOINT_FALLBACK.to_string());
    cmd.env("CLAUDE_CODE_ENTRYPOINT", &entrypoint);

    let mut child = cmd.spawn().map_err(|e| format!("spawn claude fork: {e}"))?;
    let pid = child.id();
    let stdout = child.stdout.take().ok_or_else(|| "claude fork: no stdout".to_string())?;

    // Watchdog: SIGKILL the fork if it outlives the ceiling. `finished` lets a
    // normal exit disarm it.
    let finished = Arc::new(AtomicBool::new(false));
    {
        let finished = Arc::clone(&finished);
        std::thread::spawn(move || {
            let deadline = Instant::now() + ANSWER_TIMEOUT;
            while Instant::now() < deadline {
                if finished.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            if !finished.load(Ordering::Relaxed) {
                crate::log_debug(&format!("[session_explain] fork pid {pid} timed out; killing"));
                crate::llm_provider::kill_process(pid);
            }
        });
    }

    let mut fold = ClaudeStreamFold::default();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if let Some(delta) = fold.feed(&line) {
            on_delta(&delta);
        }
    }
    let status = child.wait();
    let timed_out = !finished.swap(true, Ordering::Relaxed) && status.as_ref().map(|s| !s.success()).unwrap_or(true)
        && fold.result_text.is_none()
        && fold.error_text.is_none();
    if timed_out && fold.streamed.is_empty() {
        return Err(format!(
            "claude fork exited without an answer (status {:?}); see session_explain_stderr.log",
            status.ok().and_then(|s| s.code())
        ));
    }
    fold.into_outcome()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, session: &str, created: u64) -> ExplainRecord {
        ExplainRecord {
            id: id.into(),
            session_id: session.into(),
            source: "claude-code".into(),
            created_ms: created,
            updated_ms: created,
            preset: ExplainPreset::Explain,
            quote: "q".into(),
            question: "why".into(),
            anchor: None,
            thread: vec![],
            status: ExplainStatus::Done,
            text: "because".into(),
            error: None,
            model: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cost_usd: None,
            duration_ms: 0,
            fork_session_id: None,
        }
    }

    #[test]
    fn records_round_trip_and_list_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        write_record_in(dir.path(), &rec("b", "s1", 20)).unwrap();
        write_record_in(dir.path(), &rec("a", "s1", 10)).unwrap();
        write_record_in(dir.path(), &rec("c", "s2", 5)).unwrap();
        let got = list_in(dir.path(), "s1");
        assert_eq!(got.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(get_in(dir.path(), "s2", "c").unwrap().created_ms, 5);
        assert!(get_in(dir.path(), "s1", "zzz").is_none());
        assert!(list_in(dir.path(), "nope").is_empty());
    }

    #[test]
    fn malformed_ids_never_escape_the_store() {
        let dir = tempfile::tempdir().unwrap();
        assert!(record_path_in(dir.path(), "../x", "a").is_none());
        assert!(record_path_in(dir.path(), "s", "a/b").is_none());
        assert!(record_path_in(dir.path(), "", "a").is_none());
        assert!(list_in(dir.path(), "../..").is_empty());
    }

    #[test]
    fn prompt_quotes_the_selection_and_forbids_tools() {
        let p = build_prompt("first line\nsecond", "what?", &[]);
        assert!(p.contains("> first line\n> second\n"));
        assert!(p.contains("不要调用任何工具"));
        assert!(p.trim_end().ends_with("【问题】\nwhat?"));
        assert!(!p.contains("此前"));
    }

    #[test]
    fn prompt_folds_the_follow_up_chain_in_order() {
        let mut a = rec("a", "s", 1);
        a.question = "Q one".into();
        a.text = "A one".into();
        let mut b = rec("b", "s", 2);
        b.question = "Q two".into();
        b.text = "A two".into();
        let p = build_prompt("x", "and now?", &[a, b]);
        let q1 = p.find("Q1: Q one").unwrap();
        let a1 = p.find("A1: A one").unwrap();
        let q2 = p.find("Q2: Q two").unwrap();
        assert!(q1 < a1 && a1 < q2);
    }

    #[test]
    fn presets_have_questions_and_custom_falls_back() {
        assert!(preset_question(ExplainPreset::Translate, None).contains("翻译"));
        assert!(preset_question(ExplainPreset::Rationale, None).contains("取舍"));
        assert_eq!(preset_question(ExplainPreset::Custom, Some("  hm? ")), "hm?");
        assert_eq!(preset_question(ExplainPreset::Custom, Some("   ")), "请解释这段话。");
    }

    #[test]
    fn fork_args_mirror_the_cache_governing_flags() {
        let args = claude_fork_args(
            "sid",
            "fork-id",
            "ask",
            Some("claude-fable-5-1"),
            Some("high"),
            crate::session_launch::live_thinking_stream_args(),
            vec!["--permission-prompt-tool".into(), "mcp__fleet__fleet__permission_prompt".into()],
            vec![],
        );
        let joined = args.join(" ");
        assert!(joined.starts_with(
            "--resume sid --fork-session --session-id fork-id --no-session-persistence --max-turns 1 -p ask"
        ));
        assert!(joined.contains("--output-format stream-json --verbose --include-partial-messages"));
        // The thinking config is part of what the cache keys on.
        assert!(joined.contains("--thinking-display summarized"));
        assert!(joined.contains("--model claude-fable-5-1"));
        assert!(joined.contains("--effort high"));
        assert!(joined.ends_with("--permission-prompt-tool mcp__fleet__fleet__permission_prompt"));
        // Permission mode is deliberately absent: it does not enter the prefix.
        assert!(!joined.contains("--permission-mode"));
    }

    #[test]
    fn fork_args_without_overrides_stay_minimal() {
        let args = claude_fork_args("sid", "fork-id", "ask", None, None, vec![], vec![], vec![]);
        assert!(!args.iter().any(|a| a == "--model" || a == "--effort"));
    }

    /// Shapes copied from a real `claude --resume … --fork-session` stdout
    /// (2026-09-20): a spurious zero-turn result precedes the real turn.
    #[test]
    fn stream_fold_skips_the_spurious_result_and_keeps_the_real_one() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"f1","model":"claude-fable-5-1"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","session_id":"f1"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-fable-5-1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"「辅助"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"栏」"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"…"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"「辅助栏」"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"「辅助栏」","total_cost_usd":0.0727,"usage":{"input_tokens":2,"cache_creation_input_tokens":1843,"cache_read_input_tokens":108744,"output_tokens":152,"cache_creation":{"ephemeral_1h_input_tokens":1843,"ephemeral_5m_input_tokens":0}},"modelUsage":{"claude-fable-5-1":{}}}"#,
        ];
        let mut fold = ClaudeStreamFold::default();
        let deltas: Vec<String> = lines.iter().filter_map(|l| fold.feed(l)).collect();
        assert_eq!(deltas, vec!["「辅助", "栏」"]);
        let out = fold.into_outcome().unwrap();
        assert_eq!(out.text, "「辅助栏」");
        assert_eq!(out.model.as_deref(), Some("claude-fable-5-1"));
        let u = out.usage.unwrap();
        assert_eq!(u.cache_read_tokens, 108744);
        assert_eq!(u.cache_creation_tokens, 1843);
        assert_eq!(u.cache_creation_1h_tokens, 1843);
        assert_eq!(u.output_tokens, 152);
        assert_eq!(out.cost_usd, Some(0.0727));
    }

    #[test]
    fn stream_fold_reports_an_error_result() {
        let mut fold = ClaudeStreamFold::default();
        fold.feed(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"result":"boom"}"#);
        assert_eq!(fold.into_outcome().unwrap_err(), "boom");
    }

    #[test]
    fn stream_fold_falls_back_to_streamed_text_when_result_is_blank() {
        let mut fold = ClaudeStreamFold::default();
        fold.feed(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}}}"#);
        fold.feed(r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":""}"#);
        assert_eq!(fold.into_outcome().unwrap().text, "partial");
    }

    /// Live probe against a real Claude session — spends real tokens, so it
    /// is opt-in:
    /// `FLEET_EXPLAIN_PROBE_SESSION=<id> FLEET_EXPLAIN_PROBE_PATH=<jsonl path> \
    ///  cargo test -p claw-fleet-core --lib live_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_probe_forks_a_real_session() {
        let Ok(session_id) = std::env::var("FLEET_EXPLAIN_PROBE_SESSION") else {
            eprintln!("FLEET_EXPLAIN_PROBE_SESSION unset; skipping");
            return;
        };
        let path = std::env::var("FLEET_EXPLAIN_PROBE_PATH").unwrap_or_default();
        let accepted = ask(ExplainRequest {
            session_id: session_id.clone(),
            session_path: path,
            workspace_path: None,
            quote: "一定是fork，而不是单独一个新会话贴文本进去，这样才能命中input缓存".into(),
            preset: ExplainPreset::Explain,
            question: None,
            anchor: None,
            thread: vec![],
        })
        .unwrap();
        assert_eq!(accepted.status, ExplainStatus::Running);
        let deadline = Instant::now() + ANSWER_TIMEOUT + Duration::from_secs(5);
        let mut last_len = 0;
        let final_rec = loop {
            std::thread::sleep(Duration::from_millis(300));
            let r = get(&session_id, &accepted.id).expect("record persists");
            if r.text.len() != last_len {
                eprintln!("[{}] {} chars", r.status as u8, r.text.chars().count());
                last_len = r.text.len();
            }
            if r.status != ExplainStatus::Running {
                break r;
            }
            assert!(Instant::now() < deadline, "probe timed out");
        };
        eprintln!("{}", serde_json::to_string_pretty(&final_rec).unwrap());
        assert_eq!(final_rec.status, ExplainStatus::Done, "{:?}", final_rec.error);
        assert!(!final_rec.text.trim().is_empty());
        assert!(final_rec.cache_read_tokens > 0, "fork did not hit the prompt cache");
    }

    #[test]
    fn stream_fold_ignores_garbage_lines() {
        let mut fold = ClaudeStreamFold::default();
        assert!(fold.feed("not json").is_none());
        assert!(fold.feed("").is_none());
        assert!(fold.feed(r#"{"type":"rate_limit_event"}"#).is_none());
    }
}
