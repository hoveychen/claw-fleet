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
}

/// One declared phase, parsed from the script's `meta.phases`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPhase {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
    result: Option<String>,
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
                    }
                });
                agent.status = WorkflowAgentStatus::Done;
                if agent.agent_id.is_empty() {
                    if let Some(id) = entry.agent_id {
                        agent.agent_id = id;
                    }
                }
                agent.result = entry.result;
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

/// Read the first single- or double-quoted string literal in `s`, honoring
/// backslash escapes. Returns the unescaped contents. Iterates over `char`s
/// (not bytes) so multibyte UTF-8 (e.g. Chinese workflow names) survives intact.
fn first_quoted(s: &str) -> Option<String> {
    let mut chars = s.chars();
    // Skip until we hit an opening quote.
    let quote = loop {
        match chars.next() {
            Some(c) if c == '\'' || c == '"' => break c,
            Some(_) => continue,
            None => return None,
        }
    };
    let mut out = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            match c {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
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
        let agents = parse_journal(&journal_body);

        let (name, description, phases) = find_script_for_run(session_dir, &run_id)
            .map(|s| parse_script_meta(&s))
            .unwrap_or((None, None, Vec::new()));

        trees.push(WorkflowTree {
            run_id,
            name,
            description,
            phases,
            agents,
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
}
