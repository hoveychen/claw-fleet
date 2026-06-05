//! Claude Code **Workflow** visualization source.
//!
//! Claude Code's Workflow tool runs a declarative JS script in the background,
//! fanning out to several "workflow-subagent" agents. Unlike ordinary Task
//! subagents (which land in `<session-dir>/subagents/agent-*.jsonl`), workflow
//! runs land in their **own** directory tree:
//!
//! ```text
//! <projects>/<encoded-path>/<session-id>.jsonl          # the parent session transcript (file)
//! <projects>/<encoded-path>/<session-id>/               # sibling dir, same stem
//!   subagents/workflows/wf_<run-id>/
//!     journal.jsonl                                       # progress: one line per fan-out agent
//!     agent-<id>.jsonl                                    # each fan-out agent's full transcript
//!     agent-<id>.meta.json                                # {"agentType":"workflow-subagent"}
//!   workflows/scripts/<name>-wf_<run-id>.js               # the script (phases live here)
//! ```
//!
//! `journal.jsonl` is the only reliable progress source. Each line is one of:
//!
//! ```json
//! {"type":"started","key":"v2:<hash>","agentId":"<id>"}
//! {"type":"result","key":"v2:<hash>","agentId":"<id>","result":"<final text>"}
//! ```
//!
//! A `started` with no matching `result` (paired by `key`) means that agent is
//! still running; a `started`+`result` pair means it finished.
//!
//! Phases are NOT in the event stream — they're declared in the script's
//! `export const meta = { name, phases: [{ title, detail? }] }`. We best-effort
//! parse them from the `<name>-wf_<run-id>.js` file.
//!
//! There is no `task-notification` top-level event anywhere; completion only
//! surfaces back to the parent as a `tool_result` whose first line carries
//! `Task ID:` / `Transcript dir:` / `Run ID:`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Status of a single fan-out workflow agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowAgentStatus {
    /// `started` seen in journal, no matching `result` yet.
    Running,
    /// `started` + `result` paired by `key`.
    Done,
}

/// A single fan-out agent inside a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowAgent {
    /// Opaque agent id from the journal (`agentId`).
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// The journal pairing key (`v2:<hash>`); stable across started/result.
    pub key: String,
    pub status: WorkflowAgentStatus,
    /// Final result text, present only once the agent is `done`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Runtime `agentType` from the sibling `agent-<id>.meta.json`
    /// (e.g. `"Explore"`, `"workflow-subagent"`). Populated during discovery,
    /// not from the journal (which carries no agentType). Used to route agents
    /// back to their script call-site during DAG binding.
    #[serde(rename = "agentType", skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// First user-message text from the agent's `agent-<id>.jsonl` transcript,
    /// loaded during discovery. This is the binding signal used to route the
    /// agent to its script call-site (prefix-match against
    /// [`WorkflowNode`]-derived prompt fingerprints). Internal only — never
    /// serialized to the frontend (would bloat the payload with N full prompts).
    #[serde(skip)]
    pub prompt_text: Option<String>,
}

/// One declared phase, parsed from the script's `meta.phases`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPhase {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// How a DAG node's agent(s) were orchestrated in the script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowNodeKind {
    /// A bare `agent(...)` call (awaited sequentially).
    Single,
    /// An `agent(...)` inside a `parallel([...])` fan-out.
    Parallel,
    /// An `agent(...)` inside a `pipeline(items, ...stages)` stage.
    Pipeline,
}

/// Rolled-up live status of a DAG node, derived from its bound agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowNodeStatus {
    /// No runtime agent bound yet (declared but not started).
    Pending,
    /// At least one bound agent is still running.
    Running,
    /// At least one agent bound and all bound agents are done.
    Done,
}

/// A node in the reconstructed orchestration DAG. One node per `agent(...)`
/// **call-site** in the script (a fan-out call-site like
/// `parallel(arr.map(() => agent(...)))` is a single node that binds many
/// runtime agents).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    /// Stable id (`n0`, `n1`, ...), referenced by [`WorkflowEdge`].
    pub id: String,
    /// Human label: the script's `label:` opt when present, else derived from
    /// the enclosing phase, else `agent#N`.
    pub label: String,
    /// Enclosing phase title (`phase('X')` or the agent's `phase:` opt).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub kind: WorkflowNodeKind,
    pub status: WorkflowNodeStatus,
    /// Runtime agent ids bound to this call-site (0..N).
    #[serde(rename = "agentIds")]
    pub agent_ids: Vec<String>,
    /// True when the agent→node binding for this node is heuristic rather than
    /// exact (dynamic fan-out / order fallback). The UI flags these as
    /// approximate so the binding limitation is honest.
    pub approximate: bool,
}

/// A directed dependency edge between two [`WorkflowNode`]s (`from` → `to`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
}

/// A full workflow run discovered on disk for one parent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTree {
    /// The `wf_<run-id>` directory name, e.g. `wf_c3ab5242-718`.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// `meta.name` from the script, if found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `meta.description` from the script, if found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared phases (script `meta.phases`), best-effort.
    pub phases: Vec<WorkflowPhase>,
    /// Fan-out agents, in first-seen journal order.
    pub agents: Vec<WorkflowAgent>,
    /// Reconstructed orchestration DAG nodes (one per `agent(...)` call-site),
    /// in script order. Empty when the script could not be parsed.
    pub nodes: Vec<WorkflowNode>,
    /// Directed edges between DAG nodes (pipeline chains + phase progression).
    pub edges: Vec<WorkflowEdge>,
    /// Absolute path to the `wf_<run-id>` transcript dir.
    #[serde(rename = "transcriptDir")]
    pub transcript_dir: String,
}

impl WorkflowTree {
    /// Convenience: number of agents that have completed.
    pub fn done_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|a| a.status == WorkflowAgentStatus::Done)
            .count()
    }
    /// True while at least one agent is still running.
    pub fn is_running(&self) -> bool {
        self.agents
            .iter()
            .any(|a| a.status == WorkflowAgentStatus::Running)
    }
}

// ── Journal parsing ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct JournalLine {
    #[serde(rename = "type")]
    kind: String,
    key: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    /// `result` is a string for plain agents but a JSON **object** for agents
    /// that return structured output. Accept either (a plain `Option<String>`
    /// would fail to parse object results, dropping the whole line → the agent
    /// would never flip to done).
    result: Option<serde_json::Value>,
}

/// Flatten a journal `result` value to display text: plain strings pass through
/// unquoted; objects/arrays are JSON-stringified.
fn result_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Parse a `journal.jsonl` body into the ordered agent list.
///
/// Agents are keyed by `key` (stable across started/result). `started`
/// establishes order + agentId; a later `result` with the same `key` flips the
/// agent to `done` and attaches its result text. Lines that don't parse are
/// silently skipped (same tolerance as the rest of the codebase).
pub fn parse_journal(body: &str) -> Vec<WorkflowAgent> {
    // Preserve first-seen order while allowing in-place status upgrades.
    let mut order: Vec<String> = Vec::new();
    let mut by_key: BTreeMap<String, WorkflowAgent> = BTreeMap::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<JournalLine>(line) else {
            continue;
        };
        // A pairing key is required to track an agent; skip malformed lines.
        let Some(key) = entry.key.clone() else {
            continue;
        };
        match entry.kind.as_str() {
            "started" => {
                if !by_key.contains_key(&key) {
                    order.push(key.clone());
                    by_key.insert(
                        key.clone(),
                        WorkflowAgent {
                            agent_id: entry.agent_id.unwrap_or_default(),
                            key,
                            status: WorkflowAgentStatus::Running,
                            result: None,
                            agent_type: None,
                            prompt_text: None,
                        },
                    );
                }
            }
            "result" => {
                let agent = by_key.entry(key.clone()).or_insert_with(|| {
                    // result before started (shouldn't happen, but be safe)
                    order.push(key.clone());
                    WorkflowAgent {
                        agent_id: entry.agent_id.clone().unwrap_or_default(),
                        key: key.clone(),
                        status: WorkflowAgentStatus::Running,
                        result: None,
                        agent_type: None,
                        prompt_text: None,
                    }
                });
                agent.status = WorkflowAgentStatus::Done;
                if agent.agent_id.is_empty() {
                    if let Some(id) = entry.agent_id {
                        agent.agent_id = id;
                    }
                }
                agent.result = entry.result.map(result_to_string);
            }
            _ => {}
        }
    }

    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

// ── Script `meta` parsing ───────────────────────────────────────────────────

/// Best-effort extraction of `meta.name`, `meta.description`, and
/// `meta.phases[].{title,detail}` from a workflow script body.
///
/// The script is JS (`export const meta = { ... }`), not JSON, so we do a small
/// tolerant scan rather than pulling in a JS parser. Returns
/// `(name, description, phases)`.
pub fn parse_script_meta(body: &str) -> (Option<String>, Option<String>, Vec<WorkflowPhase>) {
    let name = extract_string_field(body, "name");
    let description = extract_string_field(body, "description");
    let phases = extract_phases(body);
    (name, description, phases)
}

/// Find `name: 'value'` / `name: "value"` at top level of the meta object.
/// Only matches the FIRST occurrence, which in practice is the meta field.
fn extract_string_field(body: &str, field: &str) -> Option<String> {
    // Look for `<field>:` then the next quoted string on the same logical span.
    let needle = format!("{field}:");
    let mut search_from = 0usize;
    while let Some(rel) = body[search_from..].find(&needle) {
        let idx = search_from + rel;
        // Ensure it's a key (preceded by whitespace / { / , ), not a substring
        // of a longer identifier like `displayName:`.
        let ok_boundary = body[..idx]
            .chars()
            .last()
            .map(|c| c.is_whitespace() || c == '{' || c == ',' || c == '(')
            .unwrap_or(true);
        if ok_boundary {
            let after = &body[idx + needle.len()..];
            if let Some(v) = first_quoted(after) {
                return Some(v);
            }
        }
        search_from = idx + needle.len();
    }
    None
}

/// Read the first single-, double-, or back-quoted string literal in `s`,
/// honoring backslash escapes. Returns the unescaped contents. Iterates over
/// `char`s (not bytes) so multibyte UTF-8 (e.g. Chinese workflow names) survives
/// intact. For backtick template literals, the scan stops at the first `${`
/// interpolation and returns the static prefix (e.g. `` `probe:${k}` `` →
/// `"probe:"`), which is good enough for a node label hint.
fn first_quoted(s: &str) -> Option<String> {
    let mut chars = s.chars().peekable();
    // Skip until we hit an opening quote.
    let quote = loop {
        match chars.next() {
            Some(c) if c == '\'' || c == '"' || c == '`' => break c,
            Some(_) => continue,
            None => return None,
        }
    };
    let mut out = String::new();
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if escaped {
            match c {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if quote == '`' && c == '$' && chars.peek() == Some(&'{') {
            // template interpolation begins — return the static prefix.
            return Some(out);
        } else if c == quote {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    Some(out) // unterminated; return what we have
}

/// Extract `phases: [ { title: 'x', detail: 'y' }, ... ]`.
fn extract_phases(body: &str) -> Vec<WorkflowPhase> {
    let Some(rel) = body.find("phases:") else {
        return Vec::new();
    };
    let after = &body[rel + "phases:".len()..];
    let Some(open) = after.find('[') else {
        return Vec::new();
    };
    // Find the matching ']' for this '['.
    let arr_start = open + 1;
    let mut depth = 1i32;
    let mut end = arr_start;
    for (i, ch) in after[arr_start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = arr_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Vec::new();
    }
    let arr = &after[arr_start..end];

    // Each phase is a `{ ... }` object; iterate over top-level braces.
    let mut phases = Vec::new();
    let bytes = arr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] as char == '{' {
            // find matching '}'
            let obj_start = i + 1;
            let mut d = 1i32;
            let mut k = obj_start;
            while k < bytes.len() {
                match bytes[k] as char {
                    '{' => d += 1,
                    '}' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            let obj = &arr[obj_start..k.min(arr.len())];
            if let Some(title) = extract_string_field(obj, "title") {
                let detail = extract_string_field(obj, "detail");
                phases.push(WorkflowPhase { title, detail });
            }
            i = k + 1;
        } else {
            i += 1;
        }
    }
    phases
}

// ── Script structure parsing → DAG ──────────────────────────────────────────

/// One `agent(...)` call-site discovered in the script, in source order.
#[derive(Debug, Clone)]
struct ScriptStep {
    phase: Option<String>,
    kind: WorkflowNodeKind,
    agent_type: Option<String>,
    label: Option<String>,
    /// Id of the enclosing `pipeline(...)` call, if any (for stage chaining).
    pipeline_group: Option<usize>,
    /// Leading **static** text of this call-site's prompt argument: either the
    /// concatenated string literals at the head of an inline prompt
    /// (`"a" + "b" + dynamicVar`), or the static head of a prompt-builder
    /// function (`const P = (x) => "## Header " + x`). Used to bind runtime
    /// agents to call-sites by prefix-matching their first user message — the
    /// only on-disk signal that survives, since the journal records no
    /// call-site → agent mapping. `None` when the prompt begins with a bare
    /// variable (no static text to fingerprint).
    prompt_fingerprint: Option<String>,
}

/// Skip a JS string literal starting at `b[i]` (an opening quote). Returns the
/// index just past the closing quote. Honors `\` escapes; for backticks it does
/// a naive skip to the next backtick (template interpolations are rare in the
/// positions we scan).
fn skip_string(b: &[u8], mut i: usize) -> usize {
    let q = b[i];
    i += 1;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Skip a `//` or `/* */` comment starting at `b[i]` (a `/`). Returns the index
/// just past the comment, or `i` unchanged if `b[i..]` isn't a comment.
fn skip_comment(b: &[u8], i: usize) -> usize {
    if i + 1 >= b.len() || b[i] != b'/' {
        return i;
    }
    match b[i + 1] {
        b'/' => {
            let mut j = i + 2;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            j
        }
        b'*' => {
            let mut j = i + 2;
            while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
                j += 1;
            }
            (j + 2).min(b.len())
        }
        _ => i,
    }
}

/// The identifier (call name) ending just before byte index `open` (an `(`).
/// Skips whitespace, then collects trailing `[A-Za-z0-9_$]`. Empty for an
/// anonymous `(` (e.g. an arrow-fn param list).
fn ident_before(b: &[u8], open: usize) -> String {
    let mut j = open;
    while j > 0 && (b[j - 1] as char).is_whitespace() {
        j -= 1;
    }
    let end = j;
    while j > 0 {
        let c = b[j - 1] as char;
        if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            j -= 1;
        } else {
            break;
        }
    }
    String::from_utf8_lossy(&b[j..end]).into_owned()
}

/// Given the index of an opening `(`, return the index of its matching `)`,
/// skipping nested parens, strings and comments. Returns `b.len()` if unmatched.
fn matching_paren(b: &[u8], open: usize) -> usize {
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        let c = b[i] as char;
        if c == '/' {
            let j = skip_comment(b, i);
            if j != i {
                i = j;
                continue;
            }
        }
        if c == '\'' || c == '"' || c == '`' {
            i = skip_string(b, i);
            continue;
        }
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
        i += 1;
    }
    b.len()
}

/// Within `span` (an `agent(...)` arg list, prompt-first), return the first
/// top-level `{ ... }` options object, skipping the prompt string literal. The
/// returned slice excludes the outer braces.
fn find_opts_object(span: &str) -> Option<&str> {
    let b = span.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c == '/' {
            let j = skip_comment(b, i);
            if j != i {
                i = j;
                continue;
            }
        }
        if c == '\'' || c == '"' || c == '`' {
            i = skip_string(b, i);
            continue;
        }
        if c == '{' {
            // find matching close brace, skipping strings/comments/nesting
            let mut depth = 0i32;
            let mut j = i;
            while j < b.len() {
                let d = b[j] as char;
                if d == '/' {
                    let k = skip_comment(b, j);
                    if k != j {
                        j = k;
                        continue;
                    }
                }
                if d == '\'' || d == '"' || d == '`' {
                    j = skip_string(b, j);
                    continue;
                }
                if d == '{' {
                    depth += 1;
                } else if d == '}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&span[i + 1..j]);
                    }
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Is `c` a byte that can appear in a JS identifier?
fn is_ident_byte(c: u8) -> bool {
    (c as char).is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// True when the keyword `kw` sits at byte `i` on identifier boundaries.
fn matches_kw(b: &[u8], i: usize, kw: &str) -> bool {
    let k = kw.as_bytes();
    if i + k.len() > b.len() || &b[i..i + k.len()] != k {
        return false;
    }
    let before_ok = i == 0 || !is_ident_byte(b[i - 1]);
    let after_ok = i + k.len() >= b.len() || !is_ident_byte(b[i + k.len()]);
    before_ok && after_ok
}

/// Advance past any run of whitespace and `//` / `/* */` comments from `i`.
fn skip_ws_comments(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let j = skip_comment(b, i);
        if j != i {
            i = j;
            continue;
        }
        break;
    }
    i
}

/// Decode a string literal whose opening quote is at byte `start` of `s`.
/// Returns `(decoded contents, byte index just past the closing quote)`.
/// Honors `\n`/`\t`/`\<char>` escapes and is UTF-8 safe (Chinese prompt text
/// survives). For backtick template literals, decoding stops at the first
/// `${` interpolation (the static prefix is what we want) but the scan still
/// runs to the closing backtick so the returned end index is correct.
fn decode_string_at(s: &str, start: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if start >= b.len() {
        return None;
    }
    let quote = b[start];
    if quote != b'\'' && quote != b'"' && quote != b'`' {
        return None;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut i = start + 1;
    let mut stop = false; // hit a template interpolation — keep the static prefix
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && i + 1 < b.len() {
            if !stop {
                match b[i + 1] {
                    b'n' => buf.push(b'\n'),
                    b't' => buf.push(b'\t'),
                    other => buf.push(other),
                }
            }
            i += 2;
            continue;
        }
        if c == quote {
            return Some((String::from_utf8_lossy(&buf).into_owned(), i + 1));
        }
        if quote == b'`' && c == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
            stop = true;
        }
        if !stop {
            buf.push(c);
        }
        i += 1;
    }
    Some((String::from_utf8_lossy(&buf).into_owned(), b.len()))
}

/// Read the leading run of string literals joined by `+` at the head of
/// expression `s` (skipping whitespace/comments). Returns the concatenated
/// static text, capturing the static prefix of a prompt built like
/// `"a" + "b" + dynamicVar + ...`. Returns `None` when `s` does not begin with
/// a string literal (e.g. a bare identifier, a builder call, or a variable).
fn leading_static_concat(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = skip_ws_comments(b, 0);
    if i >= b.len() {
        return None;
    }
    let c = b[i];
    if c != b'\'' && c != b'"' && c != b'`' {
        return None;
    }
    let mut acc = String::new();
    loop {
        let (text, end) = decode_string_at(s, i)?;
        acc.push_str(&text);
        i = skip_ws_comments(b, end);
        if i < b.len() && b[i] == b'+' {
            let next = skip_ws_comments(b, i + 1);
            if next < b.len() && (b[next] == b'\'' || b[next] == b'"' || b[next] == b'`') {
                i = next;
                continue; // another literal in the static concat chain
            }
        }
        break; // `+ <dynamic>` or end of expression — static prefix ends here
    }
    if acc.is_empty() {
        None
    } else {
        Some(acc)
    }
}

/// Find the first top-level `=>` arrow at or after byte `start`, skipping over
/// a leading `(params)` list, strings, and comments. Returns `None` if the RHS
/// is a plain value expression (begins with a string literal at depth 0) rather
/// than an arrow function, or if no arrow is found before the expression ends.
fn find_arrow(b: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = start;
    while i < b.len() {
        let j = skip_comment(b, i);
        if j != i {
            i = j;
            continue;
        }
        let c = b[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            if depth == 0 {
                return None; // RHS is a value, not an arrow fn
            }
            i = skip_string(b, i);
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            b'=' if depth == 0 && i + 1 < b.len() && b[i + 1] == b'>' => return Some(i),
            b';' | b',' if depth == 0 => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Scan the script for prompt-builder definitions
/// (`const NAME = (params) => "static" + ...` or `const NAME = "static" + ...`)
/// and map each builder name to the static head text of its returned prompt.
/// Only expression-bodied arrows / direct string consts are recognised — enough
/// for the documented prompt-builder pattern. Block-bodied functions are skipped.
fn extract_prompt_builders(body: &str) -> BTreeMap<String, String> {
    let b = body.as_bytes();
    let n = b.len();
    let mut map = BTreeMap::new();
    let mut i = 0;
    while i < n {
        let c = b[i];
        if c == b'/' {
            let j = skip_comment(b, i);
            if j != i {
                i = j;
                continue;
            }
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            i = skip_string(b, i);
            continue;
        }
        let kwlen = if matches_kw(b, i, "const") {
            5
        } else if matches_kw(b, i, "let") {
            3
        } else {
            0
        };
        if kwlen > 0 {
            let mut j = skip_ws_comments(b, i + kwlen);
            let id_start = j;
            while j < n && is_ident_byte(b[j]) {
                j += 1;
            }
            if j > id_start {
                let name = String::from_utf8_lossy(&b[id_start..j]).into_owned();
                let k = skip_ws_comments(b, j);
                // a single `=` (not `==` / `=>`)
                if k < n && b[k] == b'=' && (k + 1 >= n || (b[k + 1] != b'=' && b[k + 1] != b'>'))
                {
                    let rhs = skip_ws_comments(b, k + 1);
                    let fp = match find_arrow(b, rhs) {
                        Some(arrow) => {
                            let after = skip_ws_comments(b, arrow + 2);
                            leading_static_concat(&body[after..])
                        }
                        None => leading_static_concat(&body[rhs..]),
                    };
                    if let Some(fp) = fp {
                        map.insert(name, fp);
                    }
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    map
}

/// Within an `agent(...)` argument span (excludes the outer parens), return the
/// first argument substring — everything up to the first top-level comma.
fn first_arg_of(span: &str) -> &str {
    let b = span.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let j = skip_comment(b, i);
        if j != i {
            i = j;
            continue;
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            i = skip_string(b, i);
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => return &span[..i],
            _ => {}
        }
        i += 1;
    }
    span
}

/// Compute a call-site's prompt fingerprint from its first argument: an inline
/// string-concat prompt yields its static head directly; a builder call
/// `IDENT(...)` resolves to the builder's static head via `builders`.
fn fingerprint_for_call(first_arg: &str, builders: &BTreeMap<String, String>) -> Option<String> {
    if let Some(fp) = leading_static_concat(first_arg) {
        return Some(fp);
    }
    let t = first_arg.trim_start();
    let tb = t.as_bytes();
    let mut k = 0;
    while k < tb.len() && is_ident_byte(tb[k]) {
        k += 1;
    }
    if k > 0 {
        let rest = skip_ws_comments(tb, k);
        if rest < tb.len() && tb[rest] == b'(' {
            return builders.get(&t[..k]).cloned();
        }
    }
    None
}

/// Parse the script body into the ordered list of `agent(...)` call-sites, each
/// tagged with its enclosing phase, orchestration kind (single / parallel /
/// pipeline), declared `agentType`/`label` opts, pipeline group, and the static
/// head of its prompt (for agent→call-site binding). This is a tolerant lexical
/// scan, not a real JS parse — robust enough for the documented workflow
/// patterns (`phase()`, `parallel([...])`, `pipeline(items, ...)`).
fn parse_script_steps(body: &str) -> Vec<ScriptStep> {
    let b = body.as_bytes();
    let n = b.len();
    let builders = extract_prompt_builders(body);

    struct Frame {
        name: String,
        depth: i32,
        pipeline_id: Option<usize>,
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut depth = 0i32;
    let mut pipeline_counter = 0usize;
    let mut current_phase: Option<String> = None;
    let mut steps: Vec<ScriptStep> = Vec::new();

    let mut i = 0;
    while i < n {
        let c = b[i] as char;
        if c == '/' {
            let j = skip_comment(b, i);
            if j != i {
                i = j;
                continue;
            }
        }
        if c == '\'' || c == '"' || c == '`' {
            i = skip_string(b, i);
            continue;
        }
        if c == '(' {
            let name = ident_before(b, i);
            depth += 1;

            if name == "phase" {
                if let Some(p) = first_quoted(&body[i + 1..]) {
                    current_phase = Some(p);
                }
            } else if name == "agent" {
                // Determine kind + pipeline group from the *current* stack
                // (the agent frame isn't pushed yet).
                let mut kind = WorkflowNodeKind::Single;
                let mut pipeline_group = None;
                for f in stack.iter().rev() {
                    if f.name == "pipeline" {
                        kind = WorkflowNodeKind::Pipeline;
                        pipeline_group = f.pipeline_id;
                        break;
                    } else if f.name == "parallel" {
                        kind = WorkflowNodeKind::Parallel;
                        // a parallel may itself sit inside a pipeline stage;
                        // keep scanning for a pipeline group for chaining.
                        for g in stack.iter().rev() {
                            if g.name == "pipeline" {
                                pipeline_group = g.pipeline_id;
                                break;
                            }
                        }
                        break;
                    }
                }

                let close = matching_paren(b, i);
                let span = &body[i + 1..close.min(n)];
                let opts = find_opts_object(span);
                let agent_type = opts.and_then(|o| extract_string_field(o, "agentType"));
                let label = opts.and_then(|o| extract_string_field(o, "label"));
                let phase_opt = opts.and_then(|o| extract_string_field(o, "phase"));
                let prompt_fingerprint = fingerprint_for_call(first_arg_of(span), &builders);

                steps.push(ScriptStep {
                    phase: phase_opt.or_else(|| current_phase.clone()),
                    kind,
                    agent_type,
                    label,
                    pipeline_group,
                    prompt_fingerprint,
                });
            }

            let pipeline_id = if name == "pipeline" {
                let id = pipeline_counter;
                pipeline_counter += 1;
                Some(id)
            } else {
                None
            };
            stack.push(Frame {
                name,
                depth,
                pipeline_id,
            });
            i += 1;
            continue;
        }
        if c == ')' {
            if let Some(f) = stack.last() {
                if f.depth == depth {
                    stack.pop();
                }
            }
            depth -= 1;
            i += 1;
            continue;
        }
        i += 1;
    }

    steps
}

/// The leftover sink for unbindable agents: the last fan-out (parallel /
/// pipeline) call-site if there is one, else the last step. Shared by both
/// binding strategies so dynamic fan-out beyond what we could route lands
/// somewhere honest (flagged `approximate`).
fn leftover_target(steps: &[ScriptStep]) -> usize {
    steps
        .iter()
        .rposition(|s| matches!(s.kind, WorkflowNodeKind::Parallel | WorkflowNodeKind::Pipeline))
        .unwrap_or(steps.len() - 1)
}

/// Bind agents to call-sites by prefix-matching each agent's first prompt
/// against the call-sites' static prompt fingerprints. Returns `None` (so the
/// caller falls back to the heuristic) when there's nothing to match on — no
/// agent carries `prompt_text`, or no step has a usable fingerprint. When a
/// match is found it is **exact** (not approximate): the agent ran that exact
/// prompt head, so the binding is ground truth, not a guess. Agents that match
/// nothing become leftover on the last fan-out node (approximate).
fn fingerprint_binding(
    steps: &[ScriptStep],
    agents: &[WorkflowAgent],
) -> Option<(Vec<Vec<String>>, Vec<bool>)> {
    let has_prompts = agents.iter().any(|a| a.prompt_text.is_some());
    // (step index, fingerprint) for steps with a fingerprint long enough to be
    // discriminating — guards against a 1-2 char head matching everything.
    let mut fps: Vec<(usize, &str)> = steps
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.prompt_fingerprint.as_deref().map(|f| (i, f)))
        .filter(|(_, f)| f.trim().len() >= 4)
        .collect();
    if !has_prompts || fps.is_empty() {
        return None;
    }
    // Longest fingerprint first, so a more specific head wins when one
    // fingerprint is a prefix of another.
    fps.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    let mut bound: Vec<Vec<String>> = vec![Vec::new(); steps.len()];
    let mut approx: Vec<bool> = vec![false; steps.len()];
    let mut leftover: Vec<String> = Vec::new();
    for a in agents {
        let hit = a
            .prompt_text
            .as_deref()
            .and_then(|pt| fps.iter().find(|(_, fp)| pt.starts_with(fp)).map(|(i, _)| *i));
        match hit {
            Some(si) => bound[si].push(a.agent_id.clone()),
            None => leftover.push(a.agent_id.clone()),
        }
    }
    if !leftover.is_empty() {
        let target = leftover_target(steps);
        bound[target].extend(leftover);
        approx[target] = true; // the leftover portion is a guess, not ground truth
    }
    Some((bound, approx))
}

/// Bind agents to call-sites by the legacy `agentType`/order heuristic:
/// `agentType`-anchored runs for typed fan-out call-sites, one agent per single
/// call-site, in journal start order. Used as a fallback when fingerprint
/// binding has nothing to work with. Bindings that fall back to order (dynamic
/// fan-out, count mismatch, untyped fan-out) flag the node `approximate`.
fn heuristic_binding(
    steps: &[ScriptStep],
    agents: &[WorkflowAgent],
) -> (Vec<Vec<String>>, Vec<bool>) {
    let mut bound: Vec<Vec<String>> = vec![Vec::new(); steps.len()];
    let mut approx: Vec<bool> = vec![false; steps.len()];

    let mut ai = 0usize; // journal-agent cursor
    for (ni, step) in steps.iter().enumerate() {
        match step.kind {
            WorkflowNodeKind::Single => {
                if ai < agents.len() {
                    bound[ni].push(agents[ai].agent_id.clone());
                    ai += 1;
                }
            }
            WorkflowNodeKind::Parallel | WorkflowNodeKind::Pipeline => {
                if let Some(at) = &step.agent_type {
                    // consume the run of consecutive agents of this declared type
                    let start = ai;
                    while ai < agents.len()
                        && agents[ai].agent_type.as_deref() == Some(at.as_str())
                    {
                        bound[ni].push(agents[ai].agent_id.clone());
                        ai += 1;
                    }
                    // a typed fan-out that captured nothing is uncertain
                    if ai == start {
                        approx[ni] = true;
                    }
                } else {
                    // no type signal → can't know the fan-out width; take one and
                    // flag approximate.
                    if ai < agents.len() {
                        bound[ni].push(agents[ai].agent_id.clone());
                        ai += 1;
                    }
                    approx[ni] = true;
                }
            }
        }
    }

    // Leftover agents (dynamic fan-out beyond what we could route): attach to
    // the last fan-out node if there is one, else the last node. Flag approximate.
    if ai < agents.len() {
        let target = leftover_target(steps);
        while ai < agents.len() {
            bound[target].push(agents[ai].agent_id.clone());
            ai += 1;
        }
        approx[target] = true;
    }

    (bound, approx)
}

/// Build the DAG (nodes + edges) from parsed script steps and the runtime
/// journal agents. Agents are bound to call-site nodes by prompt fingerprint
/// when possible (ground truth), else by the `agentType`/order heuristic.
/// Heuristic / leftover bindings flag the node `approximate = true`.
fn build_dag(steps: &[ScriptStep], agents: &[WorkflowAgent]) -> (Vec<WorkflowNode>, Vec<WorkflowEdge>) {
    if steps.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // ── Binding pass ──────────────────────────────────────────────────────
    let (mut bound, approx) =
        fingerprint_binding(steps, agents).unwrap_or_else(|| heuristic_binding(steps, agents));

    // ── Node construction ─────────────────────────────────────────────────
    let mut nodes: Vec<WorkflowNode> = Vec::with_capacity(steps.len());
    let mut per_phase_idx: BTreeMap<String, usize> = BTreeMap::new();
    for (ni, step) in steps.iter().enumerate() {
        let agent_ids = std::mem::take(&mut bound[ni]);
        let status = if agent_ids.is_empty() {
            WorkflowNodeStatus::Pending
        } else {
            let any_running = agent_ids.iter().any(|id| {
                agents
                    .iter()
                    .find(|a| &a.agent_id == id)
                    .map(|a| a.status == WorkflowAgentStatus::Running)
                    .unwrap_or(false)
            });
            if any_running {
                WorkflowNodeStatus::Running
            } else {
                WorkflowNodeStatus::Done
            }
        };
        // A label's static head can be a trivial 1-char fragment when the
        // script builds it from a dominantly-dynamic concat (e.g.
        // `label: "v" + v + ":" + claim` → "v"). That's uninformative; fall back
        // to the phase-derived label in that case.
        let label = step
            .label
            .clone()
            .filter(|s| s.trim().chars().count() >= 2)
            .unwrap_or_else(|| {
            match &step.phase {
                Some(p) => {
                    let pk = p.clone();
                    let c = per_phase_idx.entry(pk).or_insert(0);
                    let lbl = if *c == 0 {
                        p.clone()
                    } else {
                        format!("{p} #{}", *c + 1)
                    };
                    *c += 1;
                    lbl
                }
                None => format!("agent #{}", ni + 1),
            }
        });
        nodes.push(WorkflowNode {
            id: format!("n{ni}"),
            label,
            phase: step.phase.clone(),
            kind: step.kind,
            status,
            agent_ids,
            approximate: approx[ni],
        });
    }

    // ── Edge construction ─────────────────────────────────────────────────
    use std::collections::HashSet;
    let mut edges: Vec<WorkflowEdge> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut add = |from: usize, to: usize, edges: &mut Vec<WorkflowEdge>| {
        if from != to && seen.insert((from, to)) {
            edges.push(WorkflowEdge {
                from: format!("n{from}"),
                to: format!("n{to}"),
            });
        }
    };

    // Pipeline-stage chains: consecutive nodes sharing a pipeline group.
    {
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (ni, step) in steps.iter().enumerate() {
            if let Some(g) = step.pipeline_group {
                groups.entry(g).or_default().push(ni);
            }
        }
        for members in groups.values() {
            for w in members.windows(2) {
                add(w[0], w[1], &mut edges);
            }
        }
    }

    // Phase progression: connect consecutive phase groups (last → first).
    let has_phases = steps.iter().any(|s| s.phase.is_some());
    if has_phases {
        // ordered distinct phases by first appearance
        let mut order: Vec<String> = Vec::new();
        let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (ni, step) in steps.iter().enumerate() {
            let key = step.phase.clone().unwrap_or_default();
            if !members.contains_key(&key) {
                order.push(key.clone());
            }
            members.entry(key).or_default().push(ni);
        }
        for w in order.windows(2) {
            let prev = &members[&w[0]];
            let next = &members[&w[1]];
            if let (Some(&last), Some(&first)) = (prev.last(), next.first()) {
                add(last, first, &mut edges);
            }
        }
    } else {
        // No phase markers: treat nodes as a sequential await chain.
        for w in (0..steps.len()).collect::<Vec<_>>().windows(2) {
            add(w[0], w[1], &mut edges);
        }
    }

    (nodes, edges)
}

// ── Disk discovery ──────────────────────────────────────────────────────────

/// Given a parent session's `.jsonl` transcript path, return the sibling
/// session directory (`<stem>/`) that holds `subagents/` and `workflows/`.
///
/// e.g. `.../projects/<enc>/<sid>.jsonl` → `.../projects/<enc>/<sid>/`.
fn session_dir_for_jsonl(session_jsonl: &Path) -> Option<PathBuf> {
    let stem = session_jsonl.file_stem()?.to_str()?.to_string();
    let parent = session_jsonl.parent()?;
    Some(parent.join(stem))
}

/// Locate the workflow script for a given `wf_<run-id>` under
/// `<session-dir>/workflows/scripts/*-<run-id>.js`.
fn find_script_for_run(session_dir: &Path, run_id: &str) -> Option<String> {
    let scripts_dir = session_dir.join("workflows").join("scripts");
    let entries = fs::read_dir(&scripts_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        // script files are named `<name>-wf_<run-id>`; match the run id suffix.
        if stem.ends_with(run_id) {
            return fs::read_to_string(&p).ok();
        }
    }
    None
}

/// Fallback script resolver for `scriptPath`-invoked runs.
///
/// When a Workflow is launched with `{scriptPath}` (or a `name`) instead of an
/// inline `{script}`, the SDK does NOT persist a copy under
/// `workflows/scripts/`, so [`find_script_for_run`] finds nothing and the DAG
/// comes back empty ("无法解析编排结构"). The script's real location is still
/// recorded in the parent transcript: every Workflow tool_result prints a
/// `Transcript dir: …/<run-id>` line and a `Script file: <path>` line. We match
/// the tool_result to this run by its transcript dir and read that file.
fn script_path_from_transcript(session_dir: &Path, run_id: &str) -> Option<String> {
    let dir_name = session_dir.file_name()?.to_str()?;
    let jsonl = session_dir.parent()?.join(format!("{dir_name}.jsonl"));
    let body = fs::read_to_string(&jsonl).ok()?;
    for line in body.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| v.get("content"));
        let Some(blocks) = content.and_then(|c| c.as_array()) else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            let text = match b.get("content") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Array(items)) => items
                    .iter()
                    .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => continue,
            };
            if let Some(path) = match_script_file_line(&text, run_id) {
                if let Ok(script) = fs::read_to_string(&path) {
                    return Some(script);
                }
            }
        }
    }
    None
}

/// Pull the `Script file: <path>` value out of a Workflow tool_result, but only
/// when its `Transcript dir:` line resolves to `run_id` (the dir basename).
fn match_script_file_line(text: &str, run_id: &str) -> Option<String> {
    let mut matches_run = false;
    let mut script: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Transcript dir:") {
            if rest.trim().trim_end_matches('/').rsplit('/').next() == Some(run_id) {
                matches_run = true;
            }
        } else if let Some(rest) = l.strip_prefix("Script file:") {
            script = Some(rest.trim().to_string());
        }
    }
    if matches_run {
        script
    } else {
        None
    }
}

/// Read the first user-message text from an agent's `agent-<id>.jsonl`
/// transcript. This is the prompt the workflow passed to the agent, used to
/// bind the agent back to its script call-site. Scans only the first few lines
/// (the prompt is line 1 in practice) and reads one line at a time so a large
/// multi-turn transcript isn't slurped whole. Handles both string and
/// text-block `content` shapes. `None` when the file is missing or no user
/// message is found near the top.
fn read_agent_first_prompt(wf_dir: &Path, agent_id: &str) -> Option<String> {
    if agent_id.is_empty() {
        return None;
    }
    let path = wf_dir.join(format!("agent-{agent_id}.jsonl"));
    let file = fs::File::open(&path).ok()?;
    let mut reader = BufReader::new(file);
    for _ in 0..8 {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let content = v.get("message").and_then(|m| m.get("content"));
        let text = match content {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => continue,
        };
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// Discover all workflow runs for a session, given its `.jsonl` transcript path.
///
/// Returns one [`WorkflowTree`] per `wf_*` directory found under
/// `<session-dir>/subagents/workflows/`. Empty when the session has no
/// workflow runs (the common case).
pub fn discover_workflow_trees(session_jsonl: &Path) -> Vec<WorkflowTree> {
    let Some(session_dir) = session_dir_for_jsonl(session_jsonl) else {
        return Vec::new();
    };
    discover_workflow_trees_in_dir(&session_dir)
}

/// Core discovery against an explicit session directory (testable without
/// touching `~/.claude`).
pub fn discover_workflow_trees_in_dir(session_dir: &Path) -> Vec<WorkflowTree> {
    let wf_root = session_dir.join("subagents").join("workflows");
    let Ok(entries) = fs::read_dir(&wf_root) else {
        return Vec::new();
    };

    let mut trees = Vec::new();
    for entry in entries.flatten() {
        let wf_dir = entry.path();
        if !wf_dir.is_dir() {
            continue;
        }
        let run_id = wf_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if run_id.is_empty() {
            continue;
        }

        let journal_body = fs::read_to_string(wf_dir.join("journal.jsonl")).unwrap_or_default();
        let mut agents = parse_journal(&journal_body);

        // Enrich each agent with its runtime `agentType` from the sibling
        // `agent-<id>.meta.json` (the journal itself carries no agentType) and
        // its first prompt text from the `agent-<id>.jsonl` transcript (the
        // binding signal — the journal carries no call-site mapping either).
        for a in &mut agents {
            if a.agent_id.is_empty() {
                continue;
            }
            let meta_path = wf_dir.join(format!("agent-{}.meta.json", a.agent_id));
            if let Ok(body) = fs::read_to_string(&meta_path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(t) = v.get("agentType").and_then(|x| x.as_str()) {
                        a.agent_type = Some(t.to_string());
                    }
                }
            }
            a.prompt_text = read_agent_first_prompt(&wf_dir, &a.agent_id);
        }

        let script_body = find_script_for_run(session_dir, &run_id)
            .or_else(|| script_path_from_transcript(session_dir, &run_id));
        let (name, description, phases) = script_body
            .as_deref()
            .map(parse_script_meta)
            .unwrap_or((None, None, Vec::new()));
        let steps = script_body
            .as_deref()
            .map(parse_script_steps)
            .unwrap_or_default();
        let (nodes, edges) = build_dag(&steps, &agents);

        trees.push(WorkflowTree {
            run_id,
            name,
            description,
            phases,
            agents,
            nodes,
            edges,
            transcript_dir: wf_dir.to_string_lossy().to_string(),
        });
    }

    // Stable order by run_id so the UI doesn't reshuffle between polls.
    trees.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    trees
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_journal_pairs_started_and_result_by_key() {
        let body = r#"
{"type":"started","key":"v2:aaa","agentId":"agentA"}
{"type":"started","key":"v2:bbb","agentId":"agentB"}
{"type":"result","key":"v2:bbb","agentId":"agentB","result":"B done"}
"#;
        let agents = parse_journal(body);
        assert_eq!(agents.len(), 2);
        // first-seen order preserved
        assert_eq!(agents[0].agent_id, "agentA");
        assert_eq!(agents[0].status, WorkflowAgentStatus::Running);
        assert_eq!(agents[0].result, None);
        assert_eq!(agents[1].agent_id, "agentB");
        assert_eq!(agents[1].status, WorkflowAgentStatus::Done);
        assert_eq!(agents[1].result.as_deref(), Some("B done"));
    }

    #[test]
    fn parse_journal_skips_malformed_and_keyless_lines() {
        let body = "not json\n{\"type\":\"started\"}\n{\"type\":\"started\",\"key\":\"v2:x\",\"agentId\":\"a\"}\n";
        let agents = parse_journal(body);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].key, "v2:x");
    }

    #[test]
    fn parse_journal_handles_object_result() {
        // Real workflows using structured output write `result` as a JSON
        // object, not a string. The agent must still flip to done and the
        // result must be captured (stringified), not dropped.
        let body = r#"
{"type":"started","key":"v2:k","agentId":"a"}
{"type":"result","key":"v2:k","agentId":"a","result":{"question":"Q","answer":"A"}}
"#;
        let agents = parse_journal(body);
        assert_eq!(agents.len(), 1);
        assert_eq!(
            agents[0].status,
            WorkflowAgentStatus::Done,
            "object result must still flip the agent to done"
        );
        let r = agents[0].result.as_deref().unwrap_or("");
        assert!(
            r.contains("question") && r.contains("answer"),
            "object result should be captured (stringified), got: {r:?}"
        );
    }

    #[test]
    fn parse_journal_string_result_stays_unquoted() {
        // A plain string result must be stored as-is, not JSON-quoted.
        let body = "{\"type\":\"started\",\"key\":\"v2:k\",\"agentId\":\"a\"}\n{\"type\":\"result\",\"key\":\"v2:k\",\"agentId\":\"a\",\"result\":\"plain text\"}\n";
        let agents = parse_journal(body);
        assert_eq!(agents[0].result.as_deref(), Some("plain text"));
    }

    #[test]
    fn done_count_and_is_running() {
        let body = r#"
{"type":"started","key":"v2:a","agentId":"a"}
{"type":"started","key":"v2:b","agentId":"b"}
{"type":"result","key":"v2:b","agentId":"b","result":"ok"}
"#;
        let agents = parse_journal(body);
        let tree = WorkflowTree {
            run_id: "wf_x".into(),
            name: None,
            description: None,
            phases: vec![],
            agents,
            nodes: vec![],
            edges: vec![],
            transcript_dir: "/tmp/wf_x".into(),
        };
        assert_eq!(tree.done_count(), 1);
        assert!(tree.is_running());
    }

    #[test]
    fn parse_script_meta_extracts_name_description_and_phases() {
        let script = r#"
export const meta = {
  name: 'fleet-workflow-viz-probe',
  description: 'Probe how Claude Code workflows surface progress',
  phases: [
    { title: 'Probe' },
    { title: 'Synthesize', detail: 'merge findings' },
  ],
}

phase('Probe')
"#;
        let (name, desc, phases) = parse_script_meta(script);
        assert_eq!(name.as_deref(), Some("fleet-workflow-viz-probe"));
        assert_eq!(
            desc.as_deref(),
            Some("Probe how Claude Code workflows surface progress")
        );
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].title, "Probe");
        assert_eq!(phases[0].detail, None);
        assert_eq!(phases[1].title, "Synthesize");
        assert_eq!(phases[1].detail.as_deref(), Some("merge findings"));
    }

    #[test]
    fn parse_script_meta_handles_double_quotes_and_chinese() {
        let script = r#"export const meta = { name: "调研", phases: [ { title: "理解", detail: "并行调研" } ] }"#;
        let (name, _desc, phases) = parse_script_meta(script);
        assert_eq!(name.as_deref(), Some("调研"));
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].title, "理解");
        assert_eq!(phases[0].detail.as_deref(), Some("并行调研"));
    }

    #[test]
    fn parse_script_meta_empty_when_no_meta() {
        let (name, desc, phases) = parse_script_meta("const x = 1;\nphase('a')\n");
        assert!(name.is_none());
        assert!(desc.is_none());
        assert!(phases.is_empty());
    }

    #[test]
    fn discover_reads_journal_and_script_from_disk() {
        let tmp = std::env::temp_dir().join(format!("wfviz-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let session_dir = tmp.join("sid123");
        let wf_dir = session_dir
            .join("subagents")
            .join("workflows")
            .join("wf_run-abc");
        fs::create_dir_all(&wf_dir).unwrap();
        fs::write(
            wf_dir.join("journal.jsonl"),
            "{\"type\":\"started\",\"key\":\"v2:k1\",\"agentId\":\"a1\"}\n{\"type\":\"result\",\"key\":\"v2:k1\",\"agentId\":\"a1\",\"result\":\"done\"}\n",
        )
        .unwrap();

        let scripts_dir = session_dir.join("workflows").join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(
            scripts_dir.join("my-flow-wf_run-abc.js"),
            "export const meta = { name: 'my-flow', phases: [ { title: 'One' } ] }\n",
        )
        .unwrap();

        let trees = discover_workflow_trees_in_dir(&session_dir);
        assert_eq!(trees.len(), 1);
        let t = &trees[0];
        assert_eq!(t.run_id, "wf_run-abc");
        assert_eq!(t.name.as_deref(), Some("my-flow"));
        assert_eq!(t.phases.len(), 1);
        assert_eq!(t.phases[0].title, "One");
        assert_eq!(t.agents.len(), 1);
        assert_eq!(t.agents[0].status, WorkflowAgentStatus::Done);
        assert_eq!(t.done_count(), 1);
        assert!(!t.is_running());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_falls_back_to_script_file_from_transcript() {
        // Repro for "无法解析编排结构": a `scriptPath`-invoked run leaves NO
        // persisted copy under `workflows/scripts/`. The only pointer to the
        // script is the `Script file: <path>` line the Workflow tool_result
        // records in the parent transcript. Without a fallback, phases+nodes
        // come back empty and the UI drops to the flat agent-list fallback.
        let tmp = std::env::temp_dir().join(format!("wfviz-spath-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let session_dir = tmp.join("sid-spath");
        let wf_dir = session_dir
            .join("subagents")
            .join("workflows")
            .join("wf_spath-1");
        fs::create_dir_all(&wf_dir).unwrap();
        fs::write(
            wf_dir.join("journal.jsonl"),
            "{\"type\":\"started\",\"key\":\"v2:k\",\"agentId\":\"a1\"}\n",
        )
        .unwrap();

        // The actual script lives at an arbitrary external path (mimicking the
        // real `/tmp/wofdec/*.js` case) — NOT under workflows/scripts/.
        let script_path = tmp.join("my_external_flow.js");
        fs::write(&script_path, CANONICAL).unwrap();

        // Parent transcript: one Workflow tool_result pointing at that script.
        let parent_jsonl = session_dir.parent().unwrap().join("sid-spath.jsonl");
        let result_text = format!(
            "Workflow launched in background. Task ID: t1\nTranscript dir: {}\nScript file: {}\n(Edit this file with Write/Edit and re-invoke …)",
            wf_dir.to_string_lossy(),
            script_path.to_string_lossy(),
        );
        let line = serde_json::json!({
            "type": "user",
            "message": { "content": [ { "type": "tool_result", "content": result_text } ] }
        })
        .to_string();
        fs::write(&parent_jsonl, format!("{line}\n")).unwrap();

        let trees = discover_workflow_trees_in_dir(&session_dir);
        assert_eq!(trees.len(), 1);
        let t = &trees[0];
        assert_eq!(t.name.as_deref(), Some("x"), "name from external script");
        assert_eq!(t.phases.len(), 2, "phases parse from external script");
        assert!(!t.nodes.is_empty(), "DAG nodes parse from external script");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_agent_first_prompt_string_and_block_content() {
        let tmp = std::env::temp_dir().join(format!("wf-prompt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // string content (the common shape on disk)
        fs::write(
            tmp.join("agent-aaa.jsonl"),
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"## Web Searcher: protocol-layer\\n\\nbody\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":\"...\"}}\n",
        )
        .unwrap();
        assert_eq!(
            read_agent_first_prompt(&tmp, "aaa").as_deref(),
            Some("## Web Searcher: protocol-layer\n\nbody")
        );

        // text-block array content
        fs::write(
            tmp.join("agent-bbb.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"## Source Extractor\\n\\nx\"}]}}\n",
        )
        .unwrap();
        assert_eq!(
            read_agent_first_prompt(&tmp, "bbb").as_deref(),
            Some("## Source Extractor\n\nx")
        );

        // missing file / empty id
        assert_eq!(read_agent_first_prompt(&tmp, "missing"), None);
        assert_eq!(read_agent_first_prompt(&tmp, ""), None);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_returns_empty_when_no_workflows() {
        let tmp = std::env::temp_dir().join(format!("wfviz-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let session_dir = tmp.join("sid");
        fs::create_dir_all(&session_dir).unwrap();
        assert!(discover_workflow_trees_in_dir(&session_dir).is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_dir_for_jsonl_strips_extension() {
        let p = Path::new("/x/projects/enc/abc123.jsonl");
        let d = session_dir_for_jsonl(p).unwrap();
        assert_eq!(d, Path::new("/x/projects/enc/abc123"));
    }

    fn mk_agent(id: &str, at: &str, st: WorkflowAgentStatus) -> WorkflowAgent {
        WorkflowAgent {
            agent_id: id.into(),
            key: format!("v2:{id}"),
            status: st,
            result: None,
            agent_type: Some(at.into()),
            prompt_text: None,
        }
    }

    /// The canonical probe pattern: a `parallel(arr.map(() => agent(...)))`
    /// fan-out followed by a single synthesize agent — the exact shape of the
    /// real wf_c3ab5242-718 sample.
    const CANONICAL: &str = r#"
export const meta = { name: 'x', phases: [ { title: 'Probe' }, { title: 'Synthesize' } ] }
phase('Probe')
const findings = await parallel(
  ASPECTS.map((a) => () =>
    agent(a.prompt, { label: `probe:${a.key}`, phase: 'Probe', agentType: 'Explore' })
  )
)
phase('Synthesize')
const synthesis = await agent(
  `synth ${findings[0]} and ${findings[1]}`,
  { label: 'synthesize', phase: 'Synthesize' }
)
"#;

    #[test]
    fn parse_script_steps_canonical_parallel_then_single() {
        let steps = parse_script_steps(CANONICAL);
        assert_eq!(steps.len(), 2, "two agent() call-sites");

        assert_eq!(steps[0].kind, WorkflowNodeKind::Parallel);
        assert_eq!(steps[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(steps[0].label.as_deref(), Some("probe:"));
        assert_eq!(steps[0].phase.as_deref(), Some("Probe"));

        assert_eq!(steps[1].kind, WorkflowNodeKind::Single);
        assert_eq!(steps[1].agent_type, None);
        assert_eq!(steps[1].label.as_deref(), Some("synthesize"));
        assert_eq!(steps[1].phase.as_deref(), Some("Synthesize"));
    }

    #[test]
    fn canonical_fingerprints_inline_and_variable() {
        // synthesize uses a backtick prompt `synth ${...}` → static head "synth ";
        // the parallel probe uses a bare `a.prompt` variable → no fingerprint.
        let steps = parse_script_steps(CANONICAL);
        assert_eq!(steps[0].prompt_fingerprint, None, "variable prompt has no static head");
        assert_eq!(steps[1].prompt_fingerprint.as_deref(), Some("synth "));
    }

    #[test]
    fn fingerprint_inline_string_concat_prompt() {
        // scope-style inline prompt: leading literal(s) then `+ dynamicVar`.
        let script = r###"
const QUESTION = "x"
const s = await agent(
  "Decompose this question.\n\n## Q\n" + QUESTION + "\n\nStructured output.",
  { label: "scope" }
)
"###;
        let steps = parse_script_steps(script);
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].prompt_fingerprint.as_deref(),
            Some("Decompose this question.\n\n## Q\n"),
            "static head stops at the first dynamic `+ QUESTION`"
        );
    }

    #[test]
    fn fingerprint_resolves_builder_function() {
        // The exact deep-research shape: prompt built by an arrow-fn builder,
        // called inside a parallel fan-out. The fingerprint must come from the
        // builder's static head, not the bare `SEARCH_PROMPT(angle)` call expr.
        let script = r###"
const SEARCH_PROMPT = (angle) =>
  "## Web Searcher: " + angle.label + "\n\nResearch question: " + QUESTION
phase("Search")
const r = await pipeline(angles,
  angle => agent(SEARCH_PROMPT(angle), { label: "search:" + angle.label, phase: "Search" })
)
"###;
        let builders = extract_prompt_builders(script);
        assert_eq!(builders.get("SEARCH_PROMPT").map(String::as_str), Some("## Web Searcher: "));
        let steps = parse_script_steps(script);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].prompt_fingerprint.as_deref(), Some("## Web Searcher: "));
        assert_eq!(steps[0].kind, WorkflowNodeKind::Pipeline);
    }

    #[test]
    fn extract_prompt_builders_skips_block_body_and_non_string() {
        // Arrow with a string body is captured; a numeric const and a
        // block-bodied arrow are not mistaken for prompt builders.
        let script = r###"
const HEADER = (v) => "## Voter " + v
const MAX = 12
const calc = (a) => { return a + 1 }
"###;
        let b = extract_prompt_builders(script);
        assert_eq!(b.get("HEADER").map(String::as_str), Some("## Voter "));
        assert!(!b.contains_key("MAX"));
        assert!(!b.contains_key("calc"));
    }

    #[test]
    fn build_dag_binds_by_agenttype_run() {
        let steps = parse_script_steps(CANONICAL);
        let agents = vec![
            mk_agent("a0", "Explore", WorkflowAgentStatus::Done),
            mk_agent("a1", "Explore", WorkflowAgentStatus::Done),
            mk_agent("a2", "workflow-subagent", WorkflowAgentStatus::Done),
        ];
        let (nodes, edges) = build_dag(&steps, &agents);

        assert_eq!(nodes.len(), 2);
        // the two Explore agents route to the typed parallel node...
        assert_eq!(nodes[0].agent_ids, vec!["a0", "a1"]);
        assert_eq!(nodes[0].kind, WorkflowNodeKind::Parallel);
        assert_eq!(nodes[0].status, WorkflowNodeStatus::Done);
        assert!(!nodes[0].approximate, "typed fan-out is an exact binding");
        // ...and the default-typed agent routes to the single synthesize node.
        assert_eq!(nodes[1].agent_ids, vec!["a2"]);
        assert_eq!(nodes[1].kind, WorkflowNodeKind::Single);

        // phase progression edge Probe → Synthesize
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, "n0");
        assert_eq!(edges[0].to, "n1");
    }

    #[test]
    fn build_dag_running_status_and_pending() {
        let steps = parse_script_steps(CANONICAL);
        // only the two probes started; synthesize not yet
        let agents = vec![
            mk_agent("a0", "Explore", WorkflowAgentStatus::Running),
            mk_agent("a1", "Explore", WorkflowAgentStatus::Done),
        ];
        let (nodes, _edges) = build_dag(&steps, &agents);
        assert_eq!(nodes[0].status, WorkflowNodeStatus::Running);
        assert_eq!(nodes[1].status, WorkflowNodeStatus::Pending);
        assert!(nodes[1].agent_ids.is_empty());
    }

    #[test]
    fn parse_and_build_pipeline_chains_stages() {
        let script = r#"
phase('Work')
const r = await pipeline(items,
  d => agent('stage one', { agentType: 'Explore' }),
  prev => agent('stage two', { label: 'verify' })
)
"#;
        let steps = parse_script_steps(script);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].kind, WorkflowNodeKind::Pipeline);
        assert_eq!(steps[1].kind, WorkflowNodeKind::Pipeline);
        assert_eq!(steps[0].pipeline_group, steps[1].pipeline_group);
        assert!(steps[0].pipeline_group.is_some());

        let agents = vec![mk_agent("x", "Explore", WorkflowAgentStatus::Done)];
        let (_nodes, edges) = build_dag(&steps, &agents);
        assert!(
            edges.iter().any(|e| e.from == "n0" && e.to == "n1"),
            "pipeline stages chain n0 → n1"
        );
    }

    fn mk_agent_pt(id: &str, prompt: &str) -> WorkflowAgent {
        WorkflowAgent {
            agent_id: id.into(),
            key: format!("v2:{id}"),
            status: WorkflowAgentStatus::Done,
            result: None,
            agent_type: Some("workflow-subagent".into()),
            prompt_text: Some(prompt.into()),
        }
    }

    /// The deep-research shape that broke before this fix: 5 call-sites, every
    /// runtime agent the default `workflow-subagent` type (no agentType signal),
    /// agents interleaved in journal order (pipeline has no barrier). The old
    /// heuristic bound ×1 to each fan-out node and dumped the rest on the last
    /// one. Fingerprint binding must recover the true fan-out widths.
    const DEEP_RESEARCH: &str = r###"
const SEARCH_PROMPT = (a) => "## Web Searcher: " + a.label
const FETCH_PROMPT = (s, a) => "## Source Extractor\n\n" + s.url
const VERIFY_PROMPT = (c, v) => "## Adversarial Claim Verifier (voter " + (v + 1) + ")"
phase("Scope")
const scope = await agent("Decompose this research question.\n\n## Q\n" + Q, { label: "scope" })
phase("Search")
const sr = await pipeline(scope.angles,
  angle => agent(SEARCH_PROMPT(angle), { label: "search:" + angle.label, phase: "Search" }),
  res => parallel(novel.map(source => () => agent(FETCH_PROMPT(source, res.angle), { label: "fetch", phase: "Fetch" })))
)
phase("Verify")
const voted = await parallel(claims.map(c => () => parallel(votes.map(v => () => agent(VERIFY_PROMPT(c, v), { label: "verify", phase: "Verify" })))))
phase("Synthesize")
const report = await agent("## Synthesis: research report\n\n" + Q, { label: "synthesize" })
"###;

    #[test]
    fn build_dag_fingerprint_recovers_fanout_widths() {
        let steps = parse_script_steps(DEEP_RESEARCH);
        assert_eq!(steps.len(), 5, "scope/search/fetch/verify/synthesize");

        // Agents interleaved (not grouped by call-site) — order must not matter.
        let agents = vec![
            mk_agent_pt("scope0", "Decompose this research question.\n\n## Q\nwhat is X?"),
            mk_agent_pt("search0", "## Web Searcher: protocol-layer\n\n..."),
            mk_agent_pt("fetch0", "## Source Extractor\n\nhttps://a.example\n..."),
            mk_agent_pt("verify0", "## Adversarial Claim Verifier (voter 1)\n\n..."),
            mk_agent_pt("search1", "## Web Searcher: product-practice\n\n..."),
            mk_agent_pt("fetch1", "## Source Extractor\n\nhttps://b.example\n..."),
            mk_agent_pt("fetch2", "## Source Extractor\n\nhttps://c.example\n..."),
            mk_agent_pt("verify1", "## Adversarial Claim Verifier (voter 2)\n\n..."),
            mk_agent_pt("verify2", "## Adversarial Claim Verifier (voter 3)\n\n..."),
            mk_agent_pt("verify3", "## Adversarial Claim Verifier (voter 1)\n\n..."),
            mk_agent_pt("synth0", "## Synthesis: research report\n\nFinal answer."),
        ];

        let (nodes, _edges) = build_dag(&steps, &agents);
        assert_eq!(nodes.len(), 5);
        // scope ×1, search ×2, fetch ×3, verify ×4, synthesize ×1 — true widths.
        assert_eq!(nodes[0].agent_ids, vec!["scope0"]);
        assert_eq!(nodes[1].agent_ids, vec!["search0", "search1"]);
        assert_eq!(nodes[2].agent_ids, vec!["fetch0", "fetch1", "fetch2"]);
        assert_eq!(nodes[3].agent_ids, vec!["verify0", "verify1", "verify2", "verify3"]);
        assert_eq!(nodes[4].agent_ids, vec!["synth0"]);
        // Every binding is ground truth (a real prompt match) — nothing approximate.
        assert!(nodes.iter().all(|n| !n.approximate), "fingerprint binding is exact");
    }

    #[test]
    fn fingerprint_binding_skipped_without_prompts_falls_back() {
        // Same script, but agents carry no prompt_text (the unit-fixture case):
        // fingerprint binding returns None and the legacy heuristic runs, which
        // for this untyped shape collapses fan-out to ×1 + a leftover dump.
        let steps = parse_script_steps(DEEP_RESEARCH);
        assert!(fingerprint_binding(&steps, &[]).is_none(), "no agents → no fingerprint binding");
        let agents: Vec<WorkflowAgent> = (0..6)
            .map(|i| mk_agent(&format!("a{i}"), "workflow-subagent", WorkflowAgentStatus::Done))
            .collect();
        assert!(
            fingerprint_binding(&steps, &agents).is_none(),
            "agents without prompt_text → fall back to heuristic"
        );
        // build_dag still produces a DAG via the heuristic (no panic, 5 nodes).
        let (nodes, _) = build_dag(&steps, &agents);
        assert_eq!(nodes.len(), 5);
    }

    #[test]
    fn node_label_falls_back_to_phase_for_trivial_static_label() {
        // A label built from a dominantly-dynamic concat (`"v" + v + ":"`) has a
        // 1-char static head ("v") — uninformative, so the node must be labeled
        // by its phase ("Verify") instead. A normal label like "search:" stays.
        let script = r###"
phase("Search")
const s = await pipeline(items, x => agent("## S " + x, { label: "search:" + x, phase: "Search" }))
phase("Verify")
const v = await parallel(items.map(c => () => parallel(vs.map(n => () => agent("## V " + n, { label: "v" + n + ":" + c.t, phase: "Verify" })))))
"###;
        let steps = parse_script_steps(script);
        let agents = vec![mk_agent_pt("s0", "## S a"), mk_agent_pt("v0", "## V 0")];
        let (nodes, _) = build_dag(&steps, &agents);
        let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert!(labels.contains(&"search:"), "informative label kept: {labels:?}");
        assert!(labels.contains(&"Verify"), "trivial 'v' label → phase: {labels:?}");
        assert!(!labels.contains(&"v"), "must not label a node 'v': {labels:?}");
    }

    #[test]
    fn build_dag_empty_when_no_steps() {
        let (nodes, edges) = build_dag(&[], &[]);
        assert!(nodes.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn discover_builds_dag_with_meta_agenttype() {
        let tmp = std::env::temp_dir().join(format!("wfdag-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let session_dir = tmp.join("sid");
        let wf_dir = session_dir
            .join("subagents")
            .join("workflows")
            .join("wf_abc-1");
        fs::create_dir_all(&wf_dir).unwrap();
        fs::write(
            wf_dir.join("journal.jsonl"),
            "{\"type\":\"started\",\"key\":\"v2:k0\",\"agentId\":\"id0\"}\n\
             {\"type\":\"result\",\"key\":\"v2:k0\",\"agentId\":\"id0\",\"result\":\"r0\"}\n\
             {\"type\":\"started\",\"key\":\"v2:k1\",\"agentId\":\"id1\"}\n\
             {\"type\":\"result\",\"key\":\"v2:k1\",\"agentId\":\"id1\",\"result\":\"r1\"}\n\
             {\"type\":\"started\",\"key\":\"v2:k2\",\"agentId\":\"id2\"}\n",
        )
        .unwrap();
        // meta.json: two Explore probes + one default synthesize
        fs::write(wf_dir.join("agent-id0.meta.json"), "{\"agentType\":\"Explore\"}").unwrap();
        fs::write(wf_dir.join("agent-id1.meta.json"), "{\"agentType\":\"Explore\"}").unwrap();
        fs::write(
            wf_dir.join("agent-id2.meta.json"),
            "{\"agentType\":\"workflow-subagent\"}",
        )
        .unwrap();

        let scripts_dir = session_dir.join("workflows").join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(scripts_dir.join("flow-wf_abc-1.js"), CANONICAL).unwrap();

        let trees = discover_workflow_trees_in_dir(&session_dir);
        assert_eq!(trees.len(), 1);
        let t = &trees[0];
        // agentType enriched from meta.json
        assert_eq!(t.agents[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(t.agents[2].agent_type.as_deref(), Some("workflow-subagent"));
        // DAG built: 2 nodes, probe node done (both probes finished), synth running
        assert_eq!(t.nodes.len(), 2);
        assert_eq!(t.nodes[0].agent_ids, vec!["id0", "id1"]);
        assert_eq!(t.nodes[0].status, WorkflowNodeStatus::Done);
        assert_eq!(t.nodes[1].agent_ids, vec!["id2"]);
        assert_eq!(t.nodes[1].status, WorkflowNodeStatus::Running);
        assert_eq!(t.edges.len(), 1);

        let _ = fs::remove_dir_all(&tmp);
    }
}
