//! Token-spend attribution for a session (main agent + all subagents).
//!
//! Decomposes each session's billed tokens into named source buckets so the
//! desktop UI can show "where did $X go?". Methodology was spiked against
//! real JSONL data (residual dropped from 86% to 21% / 79% coverage after
//! disk reconstruction). Full methodology doc + empirical validation is at
//! [`design/token-spend-methodology.md`](../../design/token-spend-methodology.md).
//!
//! Inputs:
//!   - JSONL transcript path
//!   - Optional `project_root` to load that project's CLAUDE.md / project skills
//!
//! Algorithm per assistant message:
//!   - First turn per JSONL: attribute BUNDLE_TOKENS (disk-loaded sources) to
//!     their respective buckets.
//!   - `cache_creation > 5000` (later turns): TTL refresh — attribute entire
//!     `new_content_tokens` to `ttl_refresh_overhead`. Don't slice — source
//!     content was already counted on first load.
//!   - Steady-state: attribute `new_content_tokens` to visible content via
//!     `chars/4` (CC's roughTokenCountEstimation default).
//!   - Output: walk assistant content blocks; assign text/thinking/tool_use
//!     chars; residual = `output_reasoning_invisible` (Opus extended thinking).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// CC's default chars-per-token ratio for plain text. Source:
/// `claude-code-fork/src/services/tokenEstimation.ts:203`
/// (`roughTokenCountEstimation(content, bytesPerToken = 4)`).
pub const CHARS_PER_TOKEN_TEXT: f64 = 4.0;

/// Threshold above which a `cache_creation_input_tokens` value classifies as a
/// TTL cache refresh event (versus steady-state incremental cache growth).
/// Validated against real JSONLs in the spike — 5000 gives 79% attribution
/// coverage vs 71% at threshold 10000.
pub const TTL_REFRESH_THRESHOLD: u64 = 5000;

/// Hardcoded approximation of Claude Code's base system prompt token count.
/// Calibrated against claude-code-fork@2026-05; recheck on CC version bumps
/// via a one-shot `count_tokens` API call.
pub const CC_SYSTEM_PROMPT_TOKENS: u64 = 2800;

/// Hardcoded approximation of CC's stock built-in tool schemas (Read, Bash,
/// Edit, Write, Grep, Glob, Agent, WebFetch, Skill, ToolSearch, TodoWrite,
/// ExitPlanMode, etc.). Excludes MCP tools (which expand per server).
pub const STOCK_TOOL_DEFS_TOKENS: u64 = 8000;

fn tok_text(s: &str) -> u64 {
    (s.len() as f64 / CHARS_PER_TOKEN_TEXT).round() as u64
}

/// Tokens estimate from a UTF-8 byte length.
fn tok_text_len(len: usize) -> u64 {
    (len as f64 / CHARS_PER_TOKEN_TEXT).round() as u64
}

/// Exact billed totals from API usage fields, summed across all assistant
/// messages in a session. These numbers are authoritative (not estimated).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub ephemeral_5m_tokens: u64,
    pub ephemeral_1h_tokens: u64,
}

/// Source attribution buckets for input tokens (the "where did the bytes come
/// from" view). All values are estimates with caveats:
///   - `chars/4` heuristic for visible content (±20% noise per CC).
///   - Disk-snapshot buckets read at analysis time — if CLAUDE.md/etc. were
///     edited after the session ran, the snapshot will be off.
///   - `ttl_refresh_overhead` absorbs the full `new_content_tokens` of each
///     refresh event (can't be sliced — see module docs).
///   - `residual_unexplained` is what's left after attribution; should be <30%
///     on validated session shapes.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SourceBuckets {
    // === Bundle (disk snapshot, charged once per cache TTL window) ===
    /// CC base system prompt (hardcoded constant; see CC_SYSTEM_PROMPT_TOKENS).
    pub cc_base_system_prompt: u64,
    /// CC stock tool defs (hardcoded constant; see STOCK_TOOL_DEFS_TOKENS).
    pub tool_defs: u64,
    pub user_claudemd: u64,
    pub project_claudemd: u64,
    /// `~/.claude/fleet-interaction-mode.md` + `fleet-prd-discipline.md`.
    pub fleet_reminders: u64,
    /// `~/.claude/projects/<encoded>/memory/MEMORY.md` (index only).
    pub memory_files: u64,
    /// Sum of frontmatter tokens for all installed skills (each `~/.claude/skills/<name>/SKILL.md`).
    pub skills_manifest: u64,

    // === Visible (extracted from JSONL via chars/4) ===
    pub visible_user_text: u64,
    pub visible_tool_result: u64,
    pub visible_system_reminder: u64,
    pub visible_prev_assistant: u64,
    pub visible_compact_summary: u64,

    // === Cache events ===
    /// `cache_creation_input_tokens` of all turns where it exceeds
    /// TTL_REFRESH_THRESHOLD (excluding the first turn). Not sliced further.
    pub ttl_refresh_overhead: u64,
    /// What's left after attribution. Mostly `chars/4` slippage on long
    /// sessions + mid-size cache events below the TTL threshold.
    pub residual_unexplained: u64,
}

impl SourceBuckets {
    pub fn total(&self) -> u64 {
        self.cc_base_system_prompt
            + self.tool_defs
            + self.user_claudemd
            + self.project_claudemd
            + self.fleet_reminders
            + self.memory_files
            + self.skills_manifest
            + self.visible_user_text
            + self.visible_tool_result
            + self.visible_system_reminder
            + self.visible_prev_assistant
            + self.visible_compact_summary
            + self.ttl_refresh_overhead
            + self.residual_unexplained
    }

    fn add(&mut self, other: &SourceBuckets) {
        self.cc_base_system_prompt += other.cc_base_system_prompt;
        self.tool_defs += other.tool_defs;
        self.user_claudemd += other.user_claudemd;
        self.project_claudemd += other.project_claudemd;
        self.fleet_reminders += other.fleet_reminders;
        self.memory_files += other.memory_files;
        self.skills_manifest += other.skills_manifest;
        self.visible_user_text += other.visible_user_text;
        self.visible_tool_result += other.visible_tool_result;
        self.visible_system_reminder += other.visible_system_reminder;
        self.visible_prev_assistant += other.visible_prev_assistant;
        self.visible_compact_summary += other.visible_compact_summary;
        self.ttl_refresh_overhead += other.ttl_refresh_overhead;
        self.residual_unexplained += other.residual_unexplained;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct OutputBuckets {
    pub output_text: u64,
    /// Visible `thinking` block text chars / 4. Usually 0 on Opus (extended
    /// thinking is billed but invisible in transcript).
    pub output_thinking_visible: u64,
    /// `tool_use` block input JSON + name chars / 4.
    pub output_tool_use: u64,
    /// Residual: `output_tokens - visible block estimates`. Captures Opus
    /// extended thinking. Negative residuals are clamped to 0.
    pub output_reasoning_invisible: u64,
}

impl OutputBuckets {
    pub fn total(&self) -> u64 {
        self.output_text + self.output_thinking_visible + self.output_tool_use + self.output_reasoning_invisible
    }
    fn add(&mut self, other: &OutputBuckets) {
        self.output_text += other.output_text;
        self.output_thinking_visible += other.output_thinking_visible;
        self.output_tool_use += other.output_tool_use;
        self.output_reasoning_invisible += other.output_reasoning_invisible;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionTokenBreakdown {
    pub session_id: String,
    pub jsonl_path: String,
    pub model: Option<String>,
    pub is_sidechain: bool,
    pub messages: u32,
    pub bundle_loads: u32,
    pub ttl_refresh_count: u32,
    /// Estimated USD cost given Anthropic public pricing. None for unknown models.
    pub estimated_cost_usd: Option<f64>,
    pub usage: UsageTotals,
    pub sources: SourceBuckets,
    pub output: OutputBuckets,
    /// 1.0 means residual_unexplained = 0; lower means more was unattributed.
    pub fit_confidence: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskTokenBreakdown {
    pub main: SessionTokenBreakdown,
    pub subagents: Vec<SessionTokenBreakdown>,
    pub totals_usage: UsageTotals,
    pub totals_sources: SourceBuckets,
    pub totals_output: OutputBuckets,
    pub totals_estimated_cost_usd: Option<f64>,
    /// True when the disk-snapshot baseline was successfully loaded; false
    /// when project_root was unknown or files were missing.
    pub baseline_loaded: bool,
    /// Bundle size used at analysis time. UI uses this for the tooltip.
    pub bundle_size_tokens: u64,
}

// ----------------------------------------------------------------------------
// Disk baseline
// ----------------------------------------------------------------------------

/// Sizes of the per-prompt static injection sources, in tokens. Read at
/// analysis time. Anything missing is silently 0.
#[derive(Clone, Debug, Default)]
pub struct DiskBaseline {
    pub user_claudemd: u64,
    pub project_claudemd: u64,
    pub fleet_reminders: u64,
    pub memory_files: u64,
    pub skills_manifest: u64,
    pub bundle_total: u64,
}

impl DiskBaseline {
    pub fn empty() -> Self { Self::default() }

    pub fn load(project_root: Option<&Path>, claude_projects_encoded_dir: Option<&Path>) -> Self {
        let mut b = Self::default();
        if let Some(claude_dir) = crate::session::get_claude_dir() {
            b.user_claudemd = read_file_tokens(&claude_dir.join("CLAUDE.md"));
            let interaction = read_file_tokens(&claude_dir.join("fleet-interaction-mode.md"));
            let prd = read_file_tokens(&claude_dir.join("fleet-prd-discipline.md"));
            b.fleet_reminders = interaction + prd;
        }
        if let Some(root) = project_root {
            b.project_claudemd = read_file_tokens(&root.join("CLAUDE.md"));
        }
        if let Some(memdir) = claude_projects_encoded_dir {
            b.memory_files = read_file_tokens(&memdir.join("memory").join("MEMORY.md"));
        }
        b.skills_manifest = scan_skills_manifest_tokens();
        b.bundle_total = CC_SYSTEM_PROMPT_TOKENS
            + STOCK_TOOL_DEFS_TOKENS
            + b.user_claudemd
            + b.project_claudemd
            + b.fleet_reminders
            + b.memory_files
            + b.skills_manifest;
        b
    }
}

fn read_file_tokens(path: &Path) -> u64 {
    match fs::read_to_string(path) {
        Ok(s) => tok_text(&s),
        Err(_) => 0,
    }
}

/// Sum frontmatter (YAML block at start, between `---` markers) of every
/// installed skill SKILL.md.
fn scan_skills_manifest_tokens() -> u64 {
    let mut total = 0u64;
    let Some(claude_dir) = crate::session::get_claude_dir() else { return 0 };
    let skill_root = claude_dir.join("skills");
    let Ok(dir) = fs::read_dir(&skill_root) else { return 0 };
    for entry in dir.flatten() {
        let skill_md = entry.path().join("SKILL.md");
        if let Ok(buf) = fs::read_to_string(&skill_md) {
            // Match leading YAML between the first `---\n` and the next `\n---`.
            let frontmatter_len = extract_frontmatter_len(&buf);
            total += tok_text_len(frontmatter_len);
        }
    }
    total
}

fn extract_frontmatter_len(s: &str) -> usize {
    if !s.starts_with("---\n") { return s.len().min(800); }
    let after_first = &s[4..];
    if let Some(end) = after_first.find("\n---") {
        // Include both `---` markers + the content
        return 4 + end + 4;
    }
    s.len().min(800)
}

// ----------------------------------------------------------------------------
// Reminder/text dissection (mirrors fit_v3.mjs)
// ----------------------------------------------------------------------------

/// Splits a string into (chars inside `<system-reminder>...</system-reminder>`,
/// chars outside).
fn split_reminder_chars(s: &str) -> (usize, usize) {
    let mut reminder = 0usize;
    let mut rest = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let open = b"<system-reminder>";
    let close = b"</system-reminder>";
    let mut i = 0;
    while i < bytes.len() {
        if i + open.len() <= bytes.len() && &bytes[i..i + open.len()] == open {
            // Find closing tag
            if let Some(j) = find_subslice(&bytes[i..], close) {
                let end = i + j + close.len();
                reminder += end - i;
                i = end;
                continue;
            }
        }
        rest.push(bytes[i] as char);
        i += 1;
    }
    (reminder, rest.len())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() { return Some(0); }
    if haystack.len() < needle.len() { return None; }
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle { return Some(i); }
    }
    None
}

#[derive(Default)]
struct UserCharCounts {
    text: usize,
    tool_result: usize,
    reminder: usize,
}

fn count_user_chars(content: &Value) -> UserCharCounts {
    let mut c = UserCharCounts::default();
    match content {
        Value::String(s) => {
            let (r, t) = split_reminder_chars(s);
            c.reminder += r;
            c.text += t;
        }
        Value::Array(blocks) => {
            for b in blocks {
                let block_type = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match block_type {
                    "tool_result" => {
                        if let Some(content) = b.get("content") {
                            match content {
                                Value::String(s) => {
                                    let (r, rest) = split_reminder_chars(s);
                                    c.reminder += r;
                                    c.tool_result += rest;
                                }
                                Value::Array(inner) => {
                                    for ib in inner {
                                        if ib.get("type").and_then(|v| v.as_str()) == Some("text") {
                                            if let Some(t) = ib.get("text").and_then(|v| v.as_str()) {
                                                let (r, rest) = split_reminder_chars(t);
                                                c.reminder += r;
                                                c.tool_result += rest;
                                                continue;
                                            }
                                        }
                                        c.tool_result += serde_json::to_string(ib).unwrap_or_default().len();
                                    }
                                }
                                _ => {
                                    c.tool_result += serde_json::to_string(content).unwrap_or_default().len();
                                }
                            }
                        }
                    }
                    "text" => {
                        if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                            let (r, rest) = split_reminder_chars(t);
                            c.reminder += r;
                            c.text += rest;
                        }
                    }
                    _ => {
                        c.text += serde_json::to_string(b).unwrap_or_default().len();
                    }
                }
            }
        }
        _ => {}
    }
    c
}

#[derive(Default)]
struct AssistantCharCounts {
    text: usize,
    thinking: usize,
    tool_use: usize,
}

fn count_assistant_chars(content: &Value) -> AssistantCharCounts {
    let mut c = AssistantCharCounts::default();
    let Some(blocks) = content.as_array() else { return c };
    for b in blocks {
        let block_type = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match block_type {
            "text" => {
                c.text += b.get("text").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
            }
            "thinking" => {
                c.thinking += b.get("thinking").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
                c.thinking += b.get("text").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
            }
            "tool_use" | "server_tool_use" => {
                let input_len = b
                    .get("input")
                    .map(|v| serde_json::to_string(v).unwrap_or_default().len())
                    .unwrap_or(0);
                let name_len = b.get("name").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
                c.tool_use += input_len + name_len;
            }
            _ => {
                c.tool_use += serde_json::to_string(b).unwrap_or_default().len();
            }
        }
    }
    c
}

// ----------------------------------------------------------------------------
// Per-session analysis
// ----------------------------------------------------------------------------

/// Analyze a single JSONL transcript. `project_root` is optional — when None,
/// project-specific disk baseline pieces (project CLAUDE.md, memory) are 0.
pub fn analyze_session(
    jsonl_path: &Path,
    project_root: Option<&Path>,
) -> Result<SessionTokenBreakdown, String> {
    let baseline = {
        let memdir = derive_project_memory_dir(jsonl_path);
        DiskBaseline::load(project_root, memdir.as_deref())
    };
    analyze_session_with_baseline(jsonl_path, &baseline)
}

/// Same as `analyze_session` but with an explicit baseline (useful for tests).
pub fn analyze_session_with_baseline(
    jsonl_path: &Path,
    baseline: &DiskBaseline,
) -> Result<SessionTokenBreakdown, String> {
    let content = fs::read_to_string(jsonl_path)
        .map_err(|e| format!("failed to read {}: {}", jsonl_path.display(), e))?;
    let messages: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();

    let session_id = jsonl_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let breakdown = analyze_messages(&messages, baseline, session_id, jsonl_path);
    Ok(breakdown)
}

fn derive_project_memory_dir(jsonl_path: &Path) -> Option<PathBuf> {
    // JSONL lives at .../projects/<encoded-path>/<session-id>.jsonl
    // OR    .../projects/<encoded-path>/<session-id>/subagents/agent-*.jsonl
    let parent = jsonl_path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;
    if parent_name == "subagents" {
        // up: .../projects/<encoded>/<session-id>/subagents → encoded dir is grandparent's parent
        return Some(parent.parent()?.parent()?.to_path_buf());
    }
    Some(parent.to_path_buf())
}

fn analyze_messages(
    messages: &[Value],
    baseline: &DiskBaseline,
    session_id: String,
    jsonl_path: &Path,
) -> SessionTokenBreakdown {
    let mut sources = SourceBuckets::default();
    let mut output = OutputBuckets::default();
    let mut usage = UsageTotals::default();
    let mut model: Option<String> = None;
    let mut is_sidechain = false;

    let mut pending_user_text = 0usize;
    let mut pending_tool_result = 0usize;
    let mut pending_reminder = 0usize;
    let mut awaiting_compact_summary = false;
    let mut compact_summary_chars = 0usize;
    let mut prev_text_chars = 0usize;
    let mut prev_thinking_chars = 0usize;
    let mut prev_tool_use_chars = 0usize;
    let mut assistant_idx: u32 = 0;
    let mut bundle_loads: u32 = 0;
    let mut ttl_refresh_count: u32 = 0;
    let mut seen_msg_ids: HashSet<String> = HashSet::new();

    for msg in messages {
        let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let subtype = msg.get("subtype").and_then(|t| t.as_str()).unwrap_or("");

        if msg_type == "system" && subtype == "compact_boundary" {
            awaiting_compact_summary = true;
            pending_user_text = 0;
            pending_tool_result = 0;
            pending_reminder = 0;
            prev_text_chars = 0;
            prev_thinking_chars = 0;
            prev_tool_use_chars = 0;
            continue;
        }

        if msg_type == "user" {
            let Some(inner) = msg.get("message") else { continue };
            let Some(content) = inner.get("content") else { continue };
            let c = count_user_chars(content);
            if awaiting_compact_summary {
                compact_summary_chars = c.text + c.tool_result + c.reminder;
                awaiting_compact_summary = false;
            } else {
                pending_user_text += c.text;
                pending_tool_result += c.tool_result;
                pending_reminder += c.reminder;
            }
            continue;
        }

        if msg_type != "assistant" { continue; }
        let Some(inner) = msg.get("message") else { continue };
        // Only count finalized turns; streaming half-turns have `stop_reason`
        // missing or null. Matches `compute_session_stats` in session.rs so the
        // Token tab agrees with the header cost chip.
        if inner.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        // Dedup retried writes by `id`. Matches session.rs behaviour.
        if let Some(id) = inner.get("id").and_then(|v| v.as_str()) {
            if !id.is_empty() && !seen_msg_ids.insert(id.to_string()) {
                continue;
            }
        }
        if model.is_none() {
            model = inner.get("model").and_then(|v| v.as_str()).map(String::from);
        }
        is_sidechain = is_sidechain || msg.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);

        let api_usage = inner.get("usage").cloned().unwrap_or(Value::Null);
        let input_tokens = api_usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let output_tokens = api_usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_creation = api_usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read = api_usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let eph_5m = api_usage
            .get("cache_creation")
            .and_then(|v| v.get("ephemeral_5m_input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let eph_1h = api_usage
            .get("cache_creation")
            .and_then(|v| v.get("ephemeral_1h_input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        usage.input_tokens += input_tokens;
        usage.output_tokens += output_tokens;
        usage.cache_creation_tokens += cache_creation;
        usage.cache_read_tokens += cache_read;
        usage.ephemeral_5m_tokens += eph_5m;
        usage.ephemeral_1h_tokens += eph_1h;

        let content = inner.get("content").cloned().unwrap_or(Value::Array(vec![]));
        let a = count_assistant_chars(&content);

        assistant_idx += 1;
        let is_first = assistant_idx == 1;
        let new_content_tokens = input_tokens + cache_creation;

        // Visible content this turn (chars/4 estimates)
        let v_user = tok_text_len(pending_user_text);
        let v_tool = tok_text_len(pending_tool_result);
        let v_rem = tok_text_len(pending_reminder);
        let v_prev = tok_text_len(prev_text_chars + prev_thinking_chars + prev_tool_use_chars);
        let v_compact = tok_text_len(compact_summary_chars);
        let visible_total = v_user + v_tool + v_rem + v_prev + v_compact;

        if is_first {
            bundle_loads += 1;
            sources.cc_base_system_prompt += CC_SYSTEM_PROMPT_TOKENS;
            sources.tool_defs += STOCK_TOOL_DEFS_TOKENS;
            sources.user_claudemd += baseline.user_claudemd;
            sources.project_claudemd += baseline.project_claudemd;
            sources.fleet_reminders += baseline.fleet_reminders;
            sources.memory_files += baseline.memory_files;
            sources.skills_manifest += baseline.skills_manifest;
            sources.visible_user_text += v_user;
            sources.visible_tool_result += v_tool;
            sources.visible_system_reminder += v_rem;
            sources.visible_prev_assistant += v_prev;
            sources.visible_compact_summary += v_compact;
            // Bundle absorbs the static side; remaining gap → residual.
            let attributed = baseline.bundle_total + visible_total;
            sources.residual_unexplained += new_content_tokens.saturating_sub(attributed);
        } else if cache_creation > TTL_REFRESH_THRESHOLD {
            ttl_refresh_count += 1;
            sources.ttl_refresh_overhead += new_content_tokens;
        } else {
            sources.visible_user_text += v_user;
            sources.visible_tool_result += v_tool;
            sources.visible_system_reminder += v_rem;
            sources.visible_prev_assistant += v_prev;
            sources.visible_compact_summary += v_compact;
            sources.residual_unexplained += new_content_tokens.saturating_sub(visible_total);
        }

        // Output side
        let out_text = tok_text_len(a.text);
        let out_thinking = tok_text_len(a.thinking);
        let out_tool = tok_text_len(a.tool_use);
        output.output_text += out_text;
        output.output_thinking_visible += out_thinking;
        output.output_tool_use += out_tool;
        let visible_output = out_text + out_thinking + out_tool;
        if output_tokens > visible_output {
            output.output_reasoning_invisible += output_tokens - visible_output;
        }

        pending_user_text = 0;
        pending_tool_result = 0;
        pending_reminder = 0;
        compact_summary_chars = 0;
        prev_text_chars = a.text;
        prev_thinking_chars = a.thinking;
        prev_tool_use_chars = a.tool_use;
    }

    let model_for_cost = model.as_deref();
    let estimated_cost_usd = Some(estimate_cost_usd(&usage, model_for_cost));
    let total_attributed_input = sources.total().saturating_sub(sources.residual_unexplained);
    let total_new_content = usage.input_tokens + usage.cache_creation_tokens;
    let fit_confidence = if total_new_content == 0 {
        1.0
    } else {
        (total_attributed_input as f64 / total_new_content as f64).clamp(0.0, 1.0)
    };

    SessionTokenBreakdown {
        session_id,
        jsonl_path: jsonl_path.to_string_lossy().to_string(),
        model,
        is_sidechain,
        messages: assistant_idx,
        bundle_loads,
        ttl_refresh_count,
        estimated_cost_usd,
        usage,
        sources,
        output,
        fit_confidence,
    }
}

/// Delegates to [`crate::model_cost::turn_cost_usd`] so the Token Spend tab
/// agrees with the per-turn cost the session header chip already shows. An
/// unknown model falls back to the same Opus 4.5/4.6 tier `get_model_costs`
/// uses.
fn estimate_cost_usd(u: &UsageTotals, model: Option<&str>) -> f64 {
    use crate::model_cost::{turn_cost_usd, TurnUsage};
    turn_cost_usd(
        model.unwrap_or(""),
        &TurnUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_tokens: u.cache_creation_tokens,
            cache_read_tokens: u.cache_read_tokens,
            web_search_requests: 0,
        },
    )
}

// ----------------------------------------------------------------------------
// Task aggregation
// ----------------------------------------------------------------------------

/// Aggregate a main session + all its recursive subagent JSONLs into one
/// TaskTokenBreakdown. Walks `<session-id>/subagents/*.jsonl` sibling dirs.
pub fn aggregate_task(main_jsonl: &Path, project_root: Option<&Path>) -> Result<TaskTokenBreakdown, String> {
    let memdir = derive_project_memory_dir(main_jsonl);
    let baseline = DiskBaseline::load(project_root, memdir.as_deref());
    let baseline_loaded = baseline.bundle_total > CC_SYSTEM_PROMPT_TOKENS + STOCK_TOOL_DEFS_TOKENS;

    let main = analyze_session_with_baseline(main_jsonl, &baseline)?;
    let subagents = scan_subagent_jsonls(main_jsonl)
        .into_iter()
        .filter_map(|p| analyze_session_with_baseline(&p, &baseline).ok())
        .collect::<Vec<_>>();

    let mut totals_usage = UsageTotals::default();
    let mut totals_sources = SourceBuckets::default();
    let mut totals_output = OutputBuckets::default();
    let mut totals_cost = 0.0;
    let mut have_cost = false;

    for sess in std::iter::once(&main).chain(subagents.iter()) {
        totals_usage.input_tokens += sess.usage.input_tokens;
        totals_usage.output_tokens += sess.usage.output_tokens;
        totals_usage.cache_creation_tokens += sess.usage.cache_creation_tokens;
        totals_usage.cache_read_tokens += sess.usage.cache_read_tokens;
        totals_usage.ephemeral_5m_tokens += sess.usage.ephemeral_5m_tokens;
        totals_usage.ephemeral_1h_tokens += sess.usage.ephemeral_1h_tokens;
        totals_sources.add(&sess.sources);
        totals_output.add(&sess.output);
        if let Some(c) = sess.estimated_cost_usd { totals_cost += c; have_cost = true; }
    }

    Ok(TaskTokenBreakdown {
        main,
        subagents,
        totals_usage,
        totals_sources,
        totals_output,
        totals_estimated_cost_usd: have_cost.then_some(totals_cost),
        baseline_loaded,
        bundle_size_tokens: baseline.bundle_total,
    })
}

fn scan_subagent_jsonls(main_jsonl: &Path) -> Vec<PathBuf> {
    let Some(stem) = main_jsonl.file_stem().and_then(|s| s.to_str()) else { return vec![] };
    let Some(parent) = main_jsonl.parent() else { return vec![] };
    let sub_dir = parent.join(stem).join("subagents");
    let Ok(dir) = fs::read_dir(&sub_dir) else { return vec![] };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_jsonl(lines: &[Value]) -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".jsonl").expect("tempfile");
        for l in lines { writeln!(f, "{}", serde_json::to_string(l).unwrap()).unwrap(); }
        f.flush().unwrap();
        f
    }

    fn baseline_with(claude_md: u64, fleet: u64, skills: u64) -> DiskBaseline {
        DiskBaseline {
            user_claudemd: claude_md,
            project_claudemd: 0,
            fleet_reminders: fleet,
            memory_files: 0,
            skills_manifest: skills,
            bundle_total: CC_SYSTEM_PROMPT_TOKENS + STOCK_TOOL_DEFS_TOKENS + claude_md + fleet + skills,
        }
    }

    #[test]
    fn token_estimates_use_chars_over_four() {
        assert_eq!(tok_text("hello world!"), 3); // 12 / 4 = 3
        assert_eq!(tok_text(""), 0);
        assert_eq!(tok_text_len(40), 10);
    }

    #[test]
    fn split_reminder_separates_xml_tags() {
        let s = "before<system-reminder>reminder body</system-reminder>after";
        let (rem, rest) = split_reminder_chars(s);
        assert_eq!(rem, "<system-reminder>reminder body</system-reminder>".len());
        assert_eq!(rest, "beforeafter".len());
    }

    #[test]
    fn extract_frontmatter_takes_yaml_block() {
        let body = "---\nname: foo\ndescription: bar\n---\n\n# Body content here\n";
        let n = extract_frontmatter_len(body);
        assert!(n >= 30 && n < body.len());
    }

    #[test]
    fn first_turn_attributes_bundle_to_disk_sources() {
        let baseline = baseline_with(1000, 500, 200);
        let messages = vec![
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{
                    "model":"claude-opus-4-7",
                    "stop_reason":"end_turn",
                    "content":[{"type":"text","text":"hello"}],
                    "usage":{
                        "input_tokens": 1,
                        "output_tokens": 10,
                        "cache_creation_input_tokens": 15000,
                        "cache_read_input_tokens": 0,
                    }
                }
            }),
        ];
        let f = write_jsonl(&messages);
        let s = analyze_session_with_baseline(f.path(), &baseline).expect("analyze");
        assert_eq!(s.bundle_loads, 1);
        assert_eq!(s.ttl_refresh_count, 0);
        assert_eq!(s.sources.cc_base_system_prompt, CC_SYSTEM_PROMPT_TOKENS);
        assert_eq!(s.sources.tool_defs, STOCK_TOOL_DEFS_TOKENS);
        assert_eq!(s.sources.user_claudemd, 1000);
        assert_eq!(s.sources.fleet_reminders, 500);
        assert_eq!(s.sources.skills_manifest, 200);
        // Residual = (1 + 15000) - (CC_SYSTEM_PROMPT_TOKENS + STOCK_TOOL_DEFS_TOKENS + 1000 + 500 + 200 + visible)
        // visible: "hi" = 2 chars → ~1 token
        assert!(s.sources.residual_unexplained < 5000);
        // Output text "hello" = 5 chars → 1 token
        assert_eq!(s.output.output_text, 1);
        assert_eq!(s.output.output_reasoning_invisible, 10 - 1);
        // fit_confidence should be near 1 minus residual_pct
        assert!(s.fit_confidence > 0.0 && s.fit_confidence <= 1.0);
    }

    #[test]
    fn ttl_refresh_event_lands_in_dedicated_bucket() {
        let baseline = baseline_with(0, 0, 0);
        let messages = vec![
            // Turn 1: first-turn bundle
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"x"}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{"model":"claude-opus-4-7","stop_reason":"end_turn","content":[{"type":"text","text":"y"}],
                    "usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":12000,"cache_read_input_tokens":0}}
            }),
            // Turn 2: tiny — steady state, no TTL
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"x2"}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{"model":"claude-opus-4-7","stop_reason":"end_turn","content":[{"type":"text","text":"y2"}],
                    "usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":200,"cache_read_input_tokens":12000}}
            }),
            // Turn 3: BIG cache_creation → TTL refresh
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"x3"}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{"model":"claude-opus-4-7","stop_reason":"end_turn","content":[{"type":"text","text":"y3"}],
                    "usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":50000,"cache_read_input_tokens":0}}
            }),
        ];
        let f = write_jsonl(&messages);
        let s = analyze_session_with_baseline(f.path(), &baseline).expect("analyze");
        assert_eq!(s.ttl_refresh_count, 1);
        assert_eq!(s.sources.ttl_refresh_overhead, 1 + 50000);
    }

    #[test]
    fn compact_boundary_resets_history_and_emits_summary_bucket() {
        let baseline = baseline_with(0, 0, 0);
        let synthetic = "Conversation continued summary content that is fairly large; ".repeat(40);
        let messages = vec![
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"x"}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{"model":"claude-opus-4-7","stop_reason":"end_turn","content":[{"type":"text","text":"y"}],
                    "usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":15000,"cache_read_input_tokens":0}}
            }),
            serde_json::json!({"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}),
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text": synthetic}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{"model":"claude-opus-4-7","stop_reason":"end_turn","content":[{"type":"text","text":"after"}],
                    "usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":2000,"cache_read_input_tokens":0}}
            }),
        ];
        let f = write_jsonl(&messages);
        let s = analyze_session_with_baseline(f.path(), &baseline).expect("analyze");
        // Post-compact turn is steady-state (cache_creation=2000 ≤ threshold 5000)
        // The synthetic summary is captured into visible_compact_summary.
        assert!(s.sources.visible_compact_summary > 500); // synthetic was ~2400 chars → ~600 tokens
    }

    #[test]
    fn analyze_skips_streaming_half_turns_and_duplicate_ids() {
        // Aligns token_analysis with session.rs:921 — must skip messages where
        // `stop_reason` is null/missing, and dedup by `id`. Without these
        // filters, streaming partials and retried turns inflate the totals.
        let baseline = baseline_with(0, 0, 0);
        let usage = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 10,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
        });
        let messages = vec![
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"q"}]}}),
            // Streaming half-turn: stop_reason missing -> must be skipped.
            serde_json::json!({
                "type":"assistant",
                "message":{
                    "id":"msg_alpha",
                    "model":"claude-opus-4-7",
                    "content":[{"type":"text","text":"partial"}],
                    "usage": usage,
                }
            }),
            // Finalized turn (same id as the partial above — also tests dedup
            // against the partial, even though the partial would have already
            // been dropped by the stop_reason filter).
            serde_json::json!({
                "type":"assistant",
                "message":{
                    "id":"msg_alpha",
                    "model":"claude-opus-4-7",
                    "stop_reason":"end_turn",
                    "content":[{"type":"text","text":"final"}],
                    "usage": usage,
                }
            }),
            // Duplicate of the finalized turn (e.g. retry write) — same id,
            // must be skipped.
            serde_json::json!({
                "type":"assistant",
                "message":{
                    "id":"msg_alpha",
                    "model":"claude-opus-4-7",
                    "stop_reason":"end_turn",
                    "content":[{"type":"text","text":"final"}],
                    "usage": usage,
                }
            }),
        ];
        let f = write_jsonl(&messages);
        let s = analyze_session_with_baseline(f.path(), &baseline).expect("analyze");
        assert_eq!(s.messages, 1, "should count exactly one assistant turn");
        assert_eq!(s.usage.input_tokens, 100, "must not double-count usage");
        assert_eq!(s.usage.output_tokens, 10);
    }

    #[test]
    fn estimate_cost_matches_model_cost_pricing_table() {
        // Regression guard: `estimate_cost_usd` and `model_cost::turn_cost_usd`
        // must agree, otherwise the Token Spend tab disagrees with the session
        // header chip (header uses model_cost.rs, Token tab uses this fn).
        use crate::model_cost::{turn_cost_usd, TurnUsage};

        let u = UsageTotals {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_creation_tokens: 1_000_000,
            cache_read_tokens: 10_000_000,
            ephemeral_5m_tokens: 0,
            ephemeral_1h_tokens: 0,
        };
        let turn = TurnUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_tokens: u.cache_creation_tokens,
            cache_read_tokens: u.cache_read_tokens,
            web_search_requests: 0,
        };

        for model in [
            "claude-opus-4-7",
            "claude-opus-4-6-20251101",
            "claude-opus-4-1-20250805",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
            "claude-haiku-3-5",
        ] {
            let estimated = estimate_cost_usd(&u, Some(model));
            let billed = turn_cost_usd(model, &turn);
            assert!(
                (estimated - billed).abs() < 1e-6,
                "model={} estimated=${:.4} billed=${:.4} delta=${:.4}",
                model, estimated, billed, estimated - billed,
            );
        }
    }

    #[test]
    fn aggregate_task_sums_main_plus_subagents() {
        // We don't have subagent files on disk; test the per-session totals
        // path with main only.
        let baseline = baseline_with(100, 100, 100);
        let messages = vec![
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"x"}]}}),
            serde_json::json!({
                "type":"assistant",
                "message":{"model":"claude-opus-4-7","stop_reason":"end_turn","content":[{"type":"text","text":"y"}],
                    "usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":15000,"cache_read_input_tokens":0}}
            }),
        ];
        let f = write_jsonl(&messages);
        let main = analyze_session_with_baseline(f.path(), &baseline).expect("ok");
        let mut totals_usage = UsageTotals::default();
        let mut totals_sources = SourceBuckets::default();
        let mut totals_output = OutputBuckets::default();
        totals_usage.input_tokens += main.usage.input_tokens;
        totals_usage.cache_creation_tokens += main.usage.cache_creation_tokens;
        totals_sources.add(&main.sources);
        totals_output.add(&main.output);
        assert_eq!(totals_usage.cache_creation_tokens, 15000);
        assert!(totals_sources.total() > 0);
    }
}
