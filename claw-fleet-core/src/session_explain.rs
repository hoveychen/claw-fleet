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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    /// Decision-card follow-up: the strongest case a competent expert who
    /// disagrees would make, and the premises the current recommendation
    /// rests on. Summarised into the card's answer (see [`pre_answer_digest`]).
    Steelman,
    /// Decision-card follow-up: which observable evidence or thresholds would
    /// flip the conclusion. Summarised into the card's answer like `Steelman`.
    FlipConditions,
}

impl ExplainPreset {
    /// Presets whose conclusion line rides along with the boss's answer to
    /// the decision card they were asked about.
    pub fn is_card_review(self) -> bool {
        matches!(self, ExplainPreset::Steelman | ExplainPreset::FlipConditions)
    }

    /// Label used in the answer digest; product copy, so Chinese.
    fn digest_label(self) -> &'static str {
        match self {
            ExplainPreset::Steelman => "反方论证",
            ExplainPreset::FlipConditions => "翻转条件",
            ExplainPreset::Explain => "解释",
            ExplainPreset::Translate => "翻译",
            ExplainPreset::Rationale => "为什么",
            ExplainPreset::Custom => "追问",
        }
    }
}

/// Where in the transcript the quoted passage came from, so a client can scroll
/// back to it. Both fields optional: the uuid is the durable key, the index a
/// fallback for records without one.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ExplainAnchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_idx: Option<u64>,
}

/// A side question as a client submits it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ExplainStatus {
    Running,
    Done,
    Error,
}

/// One side question and its answer, as persisted and as served.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    /// Whether the reader took this card out of the rail.
    ///
    /// Stamped onto the record when it is read, never trusted from the record
    /// file: the flag is kept in a per-session sidecar (see
    /// [`set_dismissed`]) precisely so dismissing a *running* question cannot
    /// be clobbered by the worker that is still streaming the answer into the
    /// record it holds in memory.
    #[serde(default)]
    pub dismissed: bool,
}

// ── Storage ──────────────────────────────────────────────────────────────────

pub(crate) fn explain_dir() -> Option<PathBuf> {
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
    crate::atomic_json::write_atomic(&path, &bytes)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn write_record(rec: &ExplainRecord) -> Result<(), String> {
    let root = explain_dir().ok_or_else(|| "no home dir".to_string())?;
    write_record_in(&root, rec)
}

/// Sidecar holding the ids the reader dismissed in one session.
///
/// It sits beside the records rather than inside them because a record is
/// rewritten by the worker streaming its answer, from a copy held in memory
/// since before the ✕ was pressed — writing the flag into the record would let
/// the next streaming write undo it. The sidecar is only ever touched by the
/// dismiss path, so the two writers never race over the same bytes.
const DISMISSED_FILE: &str = "dismissed.json";

fn dismissed_path_in(root: &Path, session_id: &str) -> Option<PathBuf> {
    if !safe_component(session_id) {
        return None;
    }
    Some(root.join(session_id).join(DISMISSED_FILE))
}

fn read_dismissed_in(root: &Path, session_id: &str) -> std::collections::HashSet<String> {
    let Some(path) = dismissed_path_in(root, session_id) else {
        return std::collections::HashSet::new();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(std::collections::HashSet::from_iter)
        .unwrap_or_default()
}

/// Hide (or un-hide) one side question for every client that reads it back.
///
/// Dismissing used to be a `useState` in the desktop rail, so a ✕ lasted until
/// the next session switch re-read the store and the card came straight back.
pub fn set_dismissed_in(
    root: &Path,
    session_id: &str,
    id: &str,
    dismissed: bool,
) -> Result<(), String> {
    if !safe_component(id) {
        return Err("invalid record id".to_string());
    }
    let path =
        dismissed_path_in(root, session_id).ok_or_else(|| "invalid session id".to_string())?;
    let mut ids = read_dismissed_in(root, session_id);
    let changed = if dismissed {
        ids.insert(id.to_string())
    } else {
        ids.remove(id)
    };
    if !changed {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mut sorted: Vec<&String> = ids.iter().collect();
    sorted.sort();
    let bytes = serde_json::to_vec_pretty(&sorted).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path, &bytes)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// [`set_dismissed_in`] against the real store.
pub fn set_dismissed(session_id: &str, id: &str, dismissed: bool) -> Result<(), String> {
    let root = explain_dir().ok_or_else(|| "no home dir".to_string())?;
    set_dismissed_in(&root, session_id, id, dismissed)
}

pub fn get_in(root: &Path, session_id: &str, id: &str) -> Option<ExplainRecord> {
    let path = record_path_in(root, session_id, id)?;
    let mut rec: ExplainRecord = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    rec.dismissed = read_dismissed_in(root, session_id).contains(id);
    Some(rec)
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
    let dismissed = read_dismissed_in(root, session_id);
    let mut out: Vec<ExplainRecord> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        // The dismissal sidecar shares the directory and the extension.
        .filter(|e| e.file_name() != std::ffi::OsStr::new(DISMISSED_FILE))
        .filter_map(|e| {
            serde_json::from_str::<ExplainRecord>(&fs::read_to_string(e.path()).ok()?).ok()
        })
        .map(|mut r| {
            r.dismissed = dismissed.contains(&r.id);
            r
        })
        .collect();
    out.sort_by_key(|r| (r.created_ms, r.id.clone()));
    out
}

/// Every explanation recorded for `session_id`, oldest first.
pub fn list(session_id: &str) -> Vec<ExplainRecord> {
    explain_dir()
        .map(|d| list_in(&d, session_id))
        .unwrap_or_default()
}

/// Marker files naming the fork identities sources had to leave on disk
/// (`~/.fleet/explain/forks/<fork_session_id>`), so a scanner can ask about
/// one id in O(1) instead of reading every record on every poll.
///
/// Written **before** the fork exists — a scan between the fork's creation and
/// the record's completion would otherwise list it — and never removed: the
/// dsh child session persists for good (no ephemeral fork, no delete RPC), so
/// the marker has to outlive the record that explains it. Codex rollout copies
/// use `codex_image::mark_internal_thread` for the same purpose.
fn fork_marker_path_in(root: &Path, fork_session_id: &str) -> Option<PathBuf> {
    if !safe_component(fork_session_id) {
        return None;
    }
    Some(root.join("forks").join(fork_session_id))
}

/// Directory of the empty per-fork markers [`mark_fork_session`] writes.
pub fn fork_marker_dir() -> Option<PathBuf> {
    explain_dir().map(|d| d.join("forks"))
}

fn fork_marker_path(fork_session_id: &str) -> Option<PathBuf> {
    fork_marker_path_in(&explain_dir()?, fork_session_id)
}

/// Record that `fork_session_id` is a side-question fork, not a session.
pub fn mark_fork_session(fork_session_id: &str) {
    let Some(path) = fork_marker_path(fork_session_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            crate::log_debug(&format!("[session_explain] create fork marker dir: {e}"));
            return;
        }
    }
    if let Err(e) = fs::write(&path, b"") {
        crate::log_debug(&format!(
            "[session_explain] mark fork session {fork_session_id}: {e}"
        ));
    }
}

/// Whether `session_id` is a side-question fork that scanners must hide.
pub fn is_fork_session(session_id: &str) -> bool {
    fork_marker_path(session_id).is_some_and(|p| p.exists())
}

/// Every fork identity any explanation left on disk, across all sessions.
/// The record-derived view of [`is_fork_session`]'s markers, for audits.
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
        ExplainPreset::Explain => {
            "这段话是什么意思？请用通俗的话解释它在本任务里的含义。".to_string()
        }
        ExplainPreset::Translate => {
            "请把这段话翻译成中文（若原文已是中文则译为英文），保持术语准确，只给译文。".to_string()
        }
        ExplainPreset::Rationale => {
            "你为什么这么判断 / 这么做？说明背后的取舍、被放弃的备选方案，以及你当时的不确定之处。"
                .to_string()
        }
        ExplainPreset::Custom => custom
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .unwrap_or("请解释这段话。")
            .to_string(),
        ExplainPreset::Steelman => STEELMAN_QUESTION.to_string(),
        ExplainPreset::FlipConditions => FLIP_CONDITIONS_QUESTION.to_string(),
    }
}

/// Steelman prompt. The fork sees the whole card in the `fleet__ask` tool_use
/// above it; the quote names the question and the option under scrutiny. The
/// first line is a one-sentence conclusion that [`pre_answer_digest`] lifts.
const STEELMAN_QUESTION: &str = "请针对【原文】里这张决策卡上的这个选项，给出另一边最有力的论证（steelman）。整张卡就是你上面那次 fleet__ask 调用。\n\
要求：\n\
1. 回答的第一行以「结论：」开头，用一句话说清：一个称职但不同意当前推荐的专家会选什么、最主要的理由是什么。\n\
2. 写出这位专家的立场依赖哪些前提假设、做了哪些价值取舍。\n\
3. 点明当前推荐依赖哪些前提，也就是一旦这些前提不成立，推荐就站不住的那几条。\n\
4. 诚实地给出你能想到的最强论证，不要写成稻草人。你不需要改变立场，但也不要借机修改推荐、重申推荐，或在结尾评判哪一边更好。这里只陈述论证。";

/// Flip-conditions prompt; same first-line contract as [`STEELMAN_QUESTION`].
const FLIP_CONDITIONS_QUESTION: &str = "什么证据或条件出现时，你对【原文】这张决策卡的结论会翻转？整张卡就是你上面那次 fleet__ask 调用。\n\
要求：\n\
1. 回答的第一行以「结论：」开头，用一句话说清最关键的翻转条件。\n\
2. 逐条列出具体、可观测的翻转条件：要看到什么证据、越过什么阈值、哪个事实成立或不成立，以及怎么去观测它。「情况变化」「需求不同」这类说不出观测手段的条件不算。\n\
3. 如果说不出具体、可观测的条件，就直接明说，第一行写「结论：该结论主要来自先验，数据撑不住。」，然后说明缺的是哪类数据。\n\
4. 不要借机修改推荐或重申推荐。";

// ── Answer digest ────────────────────────────────────────────────────────────

/// Longest conclusion line carried into a digest, in chars.
const DIGEST_CONCLUSION_MAX: usize = 160;
/// Longest quote excerpt used as a digest item's subject, in chars.
const DIGEST_SUBJECT_MAX: usize = 24;

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// The answer's first non-empty line minus the `结论：` lead-in and any
/// markdown emphasis the model wrapped it in.
fn conclusion_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches(['#', '>', '-', '*', ' ']).trim();
    let line = line.trim_end_matches('*').trim();
    let line = ["结论：", "结论:"]
        .iter()
        .find_map(|p| line.strip_prefix(p))
        .unwrap_or(line)
        .trim_start_matches('*')
        .trim();
    (!line.is_empty()).then(|| truncate_chars(line, DIGEST_CONCLUSION_MAX))
}

/// What a digest item is about: the one card option label the quote names,
/// or else the quote's first line, shortened.
fn digest_subject(quote: &str, option_labels: &[String]) -> String {
    let mut hits = option_labels
        .iter()
        .filter(|l| !l.trim().is_empty() && quote.contains(l.trim()));
    if let (Some(only), None) = (hits.next(), hits.next()) {
        return only.trim().to_string();
    }
    let first = quote.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    truncate_chars(first, DIGEST_SUBJECT_MAX)
}

/// The "what the boss looked at before answering" note appended to a
/// decision-card answer, or `None` when they asked nothing.
///
/// Counts finished, undismissed `Steelman` / `FlipConditions` questions asked
/// in `session_id` at or after `since_ms` (the card's timestamp), oldest
/// first, one line each. `option_labels` are the card's option labels, used
/// to name the option a question was about.
pub fn pre_answer_digest_in(
    root: &Path,
    session_id: &str,
    since_ms: u64,
    option_labels: &[String],
) -> Option<String> {
    let lines: Vec<String> = list_in(root, session_id)
        .into_iter()
        .filter(|r| r.preset.is_card_review())
        .filter(|r| r.created_ms >= since_ms)
        .filter(|r| r.status == ExplainStatus::Done && !r.dismissed)
        .filter_map(|r| {
            let conclusion = conclusion_line(&r.text)?;
            Some(format!(
                "- {}（{}）：{}",
                r.preset.digest_label(),
                digest_subject(&r.quote, option_labels),
                conclusion
            ))
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!("老板作答前查看了：\n{}", lines.join("\n")))
}

/// [`pre_answer_digest_in`] against the real store.
pub fn pre_answer_digest(session_id: &str, since_ms: u64, option_labels: &[String]) -> Option<String> {
    pre_answer_digest_in(&explain_dir()?, session_id, since_ms, option_labels)
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
            out.push_str(&format!(
                "Q{}: {}\nA{}: {}\n",
                i + 1,
                p.question.trim(),
                i + 1,
                p.text.trim()
            ));
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
        dismissed: false,
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
            let text = if out.text.trim().is_empty() {
                streamed.clone()
            } else {
                out.text.clone()
            };
            if text.trim().is_empty() {
                settle_error(
                    &mut rec,
                    "the fork produced no text (it may have tried to call a tool); try again"
                        .to_string(),
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
/// Chat-workspace sessions add their setting-sources flag for the same reason
/// (it shapes the system prompt), and the caller appends
/// `claude_launch::fleet_launch_args_for` — the same hooks, MCP server and
/// permission-prompt tool the session itself runs with. The stream flags are taken from
/// `session_launch::live_thinking_stream_args` so the fork tracks whatever
/// the real launches use, rather than a second hand-maintained list.
pub fn claude_fork_args(
    session_id: &str,
    fork_session_id: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    stream_args: Vec<String>,
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
                    self.error_text = Some(if text.is_empty() {
                        "claude reported an error".to_string()
                    } else {
                        text.to_string()
                    });
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
            return Err(self
                .error_text
                .unwrap_or_else(|| "claude reported an error".to_string()));
        }
        let text = self
            .result_text
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(self.streamed);
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
pub(crate) struct ForgetLaunchSpec(pub(crate) String);

impl Drop for ForgetLaunchSpec {
    fn drop(&mut self) {
        crate::launch_spec::forget(&self.0);
    }
}

/// Removes a file the fork left on disk when the fork is over, whichever way
/// it ended (codex rollout copies).
pub(crate) struct RemoveOnDrop(pub(crate) PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn fork_stderr_log() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("session_explain_stderr.log"))
}

/// The forks' shared stderr sink, `~/.fleet/session_explain_stderr.log`, with
/// a header line naming the fork. `flags` is the argv minus the prompt: when a
/// fork misses the cache, the flags are the first thing to compare against the
/// session's own launch. Falls back to `/dev/null` when the log is unwritable.
pub(crate) fn fork_stderr_sink(header: &str, flags: &[String]) -> std::process::Stdio {
    let Some(p) = fork_stderr_log() else {
        return std::process::Stdio::null();
    };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match fs::OpenOptions::new().create(true).append(true).open(&p) {
        Ok(mut f) => {
            let _ = writeln!(
                f,
                "[{}] {header} flags={}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                flags.join(" ")
            );
            std::process::Stdio::from(f)
        }
        Err(_) => std::process::Stdio::null(),
    }
}

/// `args` minus the prompt that follows `prompt_flag` (`-p` for Claude) or
/// everything after `--` (codex), for logging.
pub(crate) fn flags_without_prompt(args: &[String], prompt_flag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a == "--" {
            break;
        }
        if a == prompt_flag {
            skip = true;
            continue;
        }
        out.push(a.clone());
    }
    out
}

/// How a fork process ended.
pub(crate) struct ForkExit {
    pub code: Option<i32>,
    /// The watchdog killed it at [`ANSWER_TIMEOUT`].
    pub timed_out: bool,
}

/// Spawn `cmd` (its stdout must be piped), hand every stdout line to
/// `on_line` as it arrives, and SIGKILL the child if it outlives
/// [`ANSWER_TIMEOUT`]. Shared by every backend that drives a CLI fork.
pub(crate) fn drive_fork_process(
    mut cmd: std::process::Command,
    label: &str,
    on_line: &mut dyn FnMut(&str),
) -> Result<ForkExit, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {label} fork: {e}"))?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label} fork: no stdout"))?;

    // Watchdog: `finished` disarms it on a normal exit; `killed` records that
    // it fired, so the caller can tell a timeout from a crash.
    let finished = Arc::new(AtomicBool::new(false));
    let killed = Arc::new(AtomicBool::new(false));
    {
        let finished = Arc::clone(&finished);
        let killed = Arc::clone(&killed);
        let label = label.to_string();
        std::thread::spawn(move || {
            let deadline = Instant::now() + ANSWER_TIMEOUT;
            while Instant::now() < deadline {
                if finished.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            if !finished.load(Ordering::Relaxed) {
                crate::log_debug(&format!(
                    "[session_explain] {label} fork pid {pid} timed out; killing"
                ));
                killed.store(true, Ordering::Relaxed);
                crate::llm_provider::kill_process(pid);
            }
        });
    }

    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        on_line(&line);
    }
    let status = child.wait();
    finished.store(true, Ordering::Relaxed);
    Ok(ForkExit {
        code: status.ok().and_then(|s| s.code()),
        timed_out: killed.load(Ordering::Relaxed),
    })
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
        return Err(format!(
            "workspace directory not found: {}",
            spec.workspace_path
        ));
    }

    let model = crate::session::resolve_session_model_spec(&spec.session_id);
    let effort = crate::launch_spec::effort_of(&spec.session_id);

    // The fork must look Fleet-owned to `fleet mcp`, or the prompt cache is
    // lost from the tools block on. `fleet mcp` advertises its 12 control
    // tools only to sessions with a `launch_spec` note; the source session has
    // one, a CLI-minted fork id does not, so the fork's MCP tool set — and with
    // it the deferred-tools listing Claude Code writes into the conversation
    // — differed from the source's (seen 2026-09-20 by capturing both request
    // bodies: 47 tools in the note-less fork vs 14 in the source). The note is
    // a lie about persistence (the fork writes no transcript), so it is dropped
    // again the moment the process ends — on every exit path, via the guard.
    //
    // Verified end to end 2026-09-21 on a 59K-token session whose first turn
    // made two requests: the fork read 59.3K from cache and wrote 1.2K, $0.07.
    // The one case that still misses is a session whose first turn issued a
    // single request: the entry that request writes (it ends in the
    // SessionStart hook's system-role message) is not matched by *any* later
    // request, not even the session's own second one — a Claude Code quirk,
    // not a fork one. Real sessions call at least one tool in their first
    // turn, so the fork lands on a later, matchable entry.
    let fork_session_id = uuid::Uuid::new_v4().to_string();
    crate::launch_spec::record(&fork_session_id, model.as_deref(), effort.as_deref());
    crate::launch_spec::note_spawn(
        &fork_session_id,
        crate::launch_spec::Spawn {
            source: "claude",
            kind: crate::launch_spec::SpawnKind::Fork,
            workspace: &spec.workspace_path,
            pid: None,
            parent: Some(&spec.session_id),
            transcript: None,
        },
    );
    let _forget = ForgetLaunchSpec(fork_session_id.clone());

    let mut args = claude_fork_args(
        &spec.session_id,
        &fork_session_id,
        &spec.prompt,
        model.as_deref(),
        effort.as_deref(),
        crate::session_launch::live_thinking_stream_args(),
        crate::chat_workspace::chat_launch_args(&spec.workspace_path),
    );
    // The same launch files the session's own spawns use: the fleet MCP tools
    // and the permission-prompt tool are part of the cached prefix.
    let launch = crate::claude_launch::fleet_launch_args_for(&args, &spec.workspace_path);
    args.extend(launch);

    let entrypoint = crate::session::session_entrypoint(&spec.session_id)
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| FORK_ENTRYPOINT_FALLBACK.to_string());
    // A fork of an incognito session writes its stderr into that session's
    // private dir, not the shared log.
    let stderr = match crate::incognito::private_stderr_log(&args) {
        Some(p) => fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .map(std::process::Stdio::from)
            .unwrap_or_else(|_| std::process::Stdio::null()),
        None => fork_stderr_sink(
            &format!(
                "claude fork session={} cwd={} entrypoint={entrypoint}",
                spec.session_id, spec.workspace_path
            ),
            &flags_without_prompt(&args, "-p"),
        ),
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
    cmd.env("CLAUDE_CODE_ENTRYPOINT", &entrypoint);
    // A fork of an incognito session runs in its origin's config dir.
    crate::incognito::apply_launch_env(&mut cmd, &args)?;

    let mut fold = ClaudeStreamFold::default();
    let exit = drive_fork_process(cmd, "claude", &mut |line| {
        if let Some(delta) = fold.feed(line) {
            on_delta(&delta);
        }
    })?;
    let no_answer = fold.result_text.is_none() && fold.error_text.is_none();
    if no_answer && fold.streamed.is_empty() {
        return Err(if exit.timed_out {
            format!("claude fork timed out after {}s", ANSWER_TIMEOUT.as_secs())
        } else {
            format!(
                "claude fork exited without an answer (status {:?}); see session_explain_stderr.log",
                exit.code
            )
        });
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
            dismissed: false,
        }
    }

    #[test]
    fn dismissal_survives_a_re_read_and_a_streaming_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        write_record_in(dir.path(), &rec("a", "s1", 10)).unwrap();
        write_record_in(dir.path(), &rec("b", "s1", 20)).unwrap();
        assert!(!get_in(dir.path(), "s1", "a").unwrap().dismissed);

        set_dismissed_in(dir.path(), "s1", "a", true).unwrap();
        assert!(get_in(dir.path(), "s1", "a").unwrap().dismissed);
        assert!(!get_in(dir.path(), "s1", "b").unwrap().dismissed);

        // A worker still streaming an answer rewrites the record from the copy
        // it has held since before the ✕ — the flag must not ride on that copy.
        write_record_in(dir.path(), &rec("a", "s1", 10)).unwrap();
        let listed = list_in(dir.path(), "s1");
        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(listed[0].dismissed);
        assert!(!listed[1].dismissed);

        set_dismissed_in(dir.path(), "s1", "a", false).unwrap();
        assert!(!get_in(dir.path(), "s1", "a").unwrap().dismissed);
    }

    #[test]
    fn dismissal_sidecar_is_not_listed_as_a_record() {
        let dir = tempfile::tempdir().unwrap();
        write_record_in(dir.path(), &rec("a", "s1", 10)).unwrap();
        set_dismissed_in(dir.path(), "s1", "a", true).unwrap();
        assert_eq!(list_in(dir.path(), "s1").len(), 1);
        assert!(dir.path().join("s1").join(DISMISSED_FILE).exists());
        assert!(set_dismissed_in(dir.path(), "../x", "a", true).is_err());
        assert!(set_dismissed_in(dir.path(), "s1", "a/b", true).is_err());
    }

    #[test]
    fn records_round_trip_and_list_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        write_record_in(dir.path(), &rec("b", "s1", 20)).unwrap();
        write_record_in(dir.path(), &rec("a", "s1", 10)).unwrap();
        write_record_in(dir.path(), &rec("c", "s2", 5)).unwrap();
        let got = list_in(dir.path(), "s1");
        assert_eq!(
            got.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
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
        assert!(fork_marker_path_in(dir.path(), "../x").is_none());
        assert_eq!(
            fork_marker_path_in(dir.path(), "session-1").unwrap(),
            dir.path().join("forks").join("session-1")
        );
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
        assert_eq!(
            preset_question(ExplainPreset::Custom, Some("  hm? ")),
            "hm?"
        );
        assert_eq!(
            preset_question(ExplainPreset::Custom, Some("   ")),
            "请解释这段话。"
        );
    }

    #[test]
    fn card_review_presets_use_snake_case_wire_names() {
        assert_eq!(
            serde_json::to_string(&ExplainPreset::Steelman).unwrap(),
            "\"steelman\""
        );
        assert_eq!(
            serde_json::to_string(&ExplainPreset::FlipConditions).unwrap(),
            "\"flip_conditions\""
        );
        assert!(ExplainPreset::Steelman.is_card_review());
        assert!(ExplainPreset::FlipConditions.is_card_review());
        assert!(!ExplainPreset::Rationale.is_card_review());
    }

    #[test]
    fn steelman_prompt_asks_for_the_strongest_case_without_switching_sides() {
        let q = preset_question(ExplainPreset::Steelman, Some("ignored"));
        assert!(q.contains("第一行以「结论：」开头"));
        assert!(q.contains("称职但不同意"));
        assert!(q.contains("前提假设"));
        assert!(q.contains("当前推荐依赖哪些前提"));
        assert!(q.contains("不要借机修改推荐、重申推荐"));
        assert!(q.contains("fleet__ask"));
    }

    #[test]
    fn flip_prompt_demands_observable_conditions_or_admits_priors() {
        let q = preset_question(ExplainPreset::FlipConditions, None);
        assert!(q.contains("第一行以「结论：」开头"));
        assert!(q.contains("可观测"));
        assert!(q.contains("阈值"));
        assert!(q.contains("该结论主要来自先验，数据撑不住"));
        assert!(q.contains("不要借机修改推荐"));
    }

    fn review(id: &str, preset: ExplainPreset, created: u64, quote: &str, text: &str) -> ExplainRecord {
        let mut r = rec(id, "s1", created);
        r.preset = preset;
        r.quote = quote.into();
        r.text = text.into();
        r
    }

    #[test]
    fn digest_is_none_without_card_review_questions() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(pre_answer_digest_in(dir.path(), "s1", 0, &[]), None);
        // Other presets, even on the card, are not digested.
        write_record_in(dir.path(), &rec("a", "s1", 100)).unwrap();
        assert_eq!(pre_answer_digest_in(dir.path(), "s1", 0, &[]), None);
    }

    #[test]
    fn digest_lists_finished_reviews_after_the_card_one_per_line() {
        let dir = tempfile::tempdir().unwrap();
        let labels = vec!["保留兼容".to_string(), "直接删除".to_string()];
        // Asked before the card existed: belongs to an earlier card.
        write_record_in(
            dir.path(),
            &review("old", ExplainPreset::Steelman, 50, "保留兼容", "结论：旧的"),
        )
        .unwrap();
        write_record_in(
            dir.path(),
            &review(
                "s",
                ExplainPreset::Steelman,
                200,
                "要不要保留兼容？\n选项：保留兼容",
                "**结论：** 专家会选直接删除，因为维护成本更高。\n\n细节……",
            ),
        )
        .unwrap();
        write_record_in(
            dir.path(),
            &review(
                "f",
                ExplainPreset::FlipConditions,
                300,
                "这里有一大段没有出现任何选项名的问题正文，长度超过二十四个字",
                "结论：该结论主要来自先验，数据撑不住。",
            ),
        )
        .unwrap();
        // Unfinished, dismissed and empty answers are skipped.
        let mut running = review("r", ExplainPreset::Steelman, 400, "保留兼容", "结论：半截");
        running.status = ExplainStatus::Running;
        write_record_in(dir.path(), &running).unwrap();
        write_record_in(
            dir.path(),
            &review("d", ExplainPreset::FlipConditions, 500, "直接删除", "结论：被关掉了"),
        )
        .unwrap();
        set_dismissed_in(dir.path(), "s1", "d", true).unwrap();
        write_record_in(
            dir.path(),
            &review("e", ExplainPreset::Steelman, 600, "直接删除", "  \n "),
        )
        .unwrap();

        let got = pre_answer_digest_in(dir.path(), "s1", 100, &labels).unwrap();
        assert_eq!(
            got,
            "老板作答前查看了：\n\
             - 反方论证（保留兼容）：专家会选直接删除，因为维护成本更高。\n\
             - 翻转条件（这里有一大段没有出现任何选项名的问题正文，长度超…）：该结论主要来自先验，数据撑不住。"
        );
    }

    #[test]
    fn digest_subject_falls_back_to_the_quote_when_labels_are_ambiguous() {
        let labels = vec!["A".to_string(), "B".to_string()];
        assert_eq!(digest_subject("pick A", &labels), "A");
        assert_eq!(digest_subject("A or B?", &labels), "A or B?");
        assert_eq!(digest_subject("\n  first\nsecond", &[]), "first");
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
            vec!["--setting-sources".into(), "project".into()],
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
        assert!(joined.ends_with("--setting-sources project"));
        // Permission mode is deliberately absent: it does not enter the prefix.
        assert!(!joined.contains("--permission-mode"));
    }

    #[test]
    fn fork_args_without_overrides_stay_minimal() {
        let args = claude_fork_args("sid", "fork-id", "ask", None, None, vec![], vec![]);
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
        fold.feed(
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":""}"#,
        );
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
        // Codex/dsh sessions need their workspace named (only Claude's is
        // resolvable from the transcript).
        let workspace_path = std::env::var("FLEET_EXPLAIN_PROBE_WORKSPACE").ok();
        let accepted = ask(ExplainRequest {
            session_id: session_id.clone(),
            session_path: path,
            workspace_path,
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
        assert_eq!(
            final_rec.status,
            ExplainStatus::Done,
            "{:?}",
            final_rec.error
        );
        assert!(!final_rec.text.trim().is_empty());
        assert!(
            final_rec.cache_read_tokens > 0,
            "fork did not hit the prompt cache"
        );
    }

    #[test]
    fn flags_without_prompt_drops_the_prompt_on_both_conventions() {
        let claude = vec![
            "--resume".to_string(),
            "s".into(),
            "-p".into(),
            "secret".into(),
            "--verbose".into(),
        ];
        assert_eq!(
            flags_without_prompt(&claude, "-p"),
            vec!["--resume", "s", "--verbose"]
        );
        let codex = vec![
            "exec".to_string(),
            "resume".into(),
            "f".into(),
            "--".into(),
            "secret".into(),
        ];
        assert_eq!(
            flags_without_prompt(&codex, "--"),
            vec!["exec", "resume", "f"]
        );
    }

    #[test]
    fn stream_fold_ignores_garbage_lines() {
        let mut fold = ClaudeStreamFold::default();
        assert!(fold.feed("not json").is_none());
        assert!(fold.feed("").is_none());
        assert!(fold.feed(r#"{"type":"rate_limit_event"}"#).is_none());
    }
}
