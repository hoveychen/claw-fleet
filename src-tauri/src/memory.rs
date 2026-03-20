//! Memory scanning — reads Claude Code auto-memory files from
//! `~/.claude/projects/<project>/memory/` and traces their edit history
//! by scanning session JSONL files for Write/Edit tool calls.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::get_claude_dir;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMemory {
    /// Display name of the workspace (last path component).
    pub workspace_name: String,
    /// Full workspace path (decoded from directory name).
    pub workspace_path: String,
    /// The encoded directory name under ~/.claude/projects/.
    pub project_key: String,
    /// Whether a CLAUDE.md exists in the workspace root.
    pub has_claude_md: bool,
    /// Memory files found in the memory/ subdirectory.
    pub files: Vec<MemoryFile>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFile {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHistoryEntry {
    /// The session ID that made the change.
    pub session_id: String,
    /// Workspace name of the session.
    pub workspace_name: String,
    /// Timestamp of the tool call (ISO 8601).
    pub timestamp: String,
    /// Tool used: "Write" or "Edit".
    pub tool: String,
    /// For Write: full new content.  For Edit: old_string and new_string.
    pub detail: MemoryEditDetail,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum MemoryEditDetail {
    #[serde(rename = "write", rename_all = "camelCase")]
    Write { content: String },
    #[serde(rename = "edit", rename_all = "camelCase")]
    Edit {
        old_string: String,
        new_string: String,
    },
}

// ── Scan all workspace memories ──────────────────────────────────────────────

pub fn scan_all_memories() -> Vec<WorkspaceMemory> {
    let Some(claude_dir) = get_claude_dir() else {
        return vec![];
    };
    let projects_dir = claude_dir.join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return vec![];
    };

    let mut results = Vec::new();

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let project_key = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        let memory_dir = dir.join("memory");
        if !memory_dir.is_dir() {
            continue; // skip workspaces without memory
        }

        let workspace_path = decode_project_key(&project_key);
        let workspace_name = workspace_path
            .split('/')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or(&workspace_path)
            .to_string();

        // Check for CLAUDE.md
        let has_claude_md = Path::new(&workspace_path).join("CLAUDE.md").exists()
            || Path::new(&workspace_path)
                .join(".claude")
                .join("CLAUDE.md")
                .exists();

        // Scan memory files
        let mut files = Vec::new();
        if let Ok(mem_entries) = fs::read_dir(&memory_dir) {
            for mem_entry in mem_entries.flatten() {
                let mem_path = mem_entry.path();
                if !mem_path.is_file() {
                    continue;
                }
                if let Some(metadata) = fs::metadata(&mem_path).ok() {
                    let modified_ms = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);

                    files.push(MemoryFile {
                        name: mem_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .to_string(),
                        path: mem_path.to_string_lossy().to_string(),
                        size_bytes: metadata.len(),
                        modified_ms,
                    });
                }
            }
        }

        // Sort: MEMORY.md first, then alphabetically
        files.sort_by(|a, b| {
            let a_is_index = a.name == "MEMORY.md";
            let b_is_index = b.name == "MEMORY.md";
            b_is_index.cmp(&a_is_index).then(a.name.cmp(&b.name))
        });

        results.push(WorkspaceMemory {
            workspace_name,
            workspace_path,
            project_key,
            has_claude_md,
            files,
        });
    }

    // Sort by workspace name
    results.sort_by(|a, b| a.workspace_name.cmp(&b.workspace_name));
    results
}

// ── Read a memory file's content ─────────────────────────────────────────────

pub fn read_memory_file(path: &str) -> Result<String, String> {
    // Safety: only allow reading from ~/.claude/projects/*/memory/
    let claude_dir = get_claude_dir().ok_or("cannot determine home dir")?;
    let projects_dir = claude_dir.join("projects");
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let projects_canonical = fs::canonicalize(&projects_dir).map_err(|e| e.to_string())?;

    if !canonical.starts_with(&projects_canonical) {
        return Err("path is outside ~/.claude/projects/".into());
    }

    fs::read_to_string(path).map_err(|e| e.to_string())
}

// ── Trace history of a memory file ───────────────────────────────────────────

pub fn trace_memory_history(memory_path: &str) -> Vec<MemoryHistoryEntry> {
    let Some(claude_dir) = get_claude_dir() else {
        return vec![];
    };

    // Determine which project directory this memory belongs to
    let memory_pb = PathBuf::from(memory_path);
    let projects_dir = claude_dir.join("projects");

    // Extract the project key from the memory path
    // e.g. ~/.claude/projects/-Users-foo-bar/memory/MEMORY.md → -Users-foo-bar
    let project_key = memory_pb
        .parent()  // memory/
        .and_then(|p| p.parent()) // project dir
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    if project_key.is_empty() {
        return vec![];
    }

    let project_dir = projects_dir.join(&project_key);
    let workspace_name = decode_project_key(&project_key)
        .split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or_default()
        .to_string();

    // Scan all JSONL files in this project directory
    let mut history = Vec::new();

    let Ok(entries) = fs::read_dir(&project_dir) else {
        return history;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        if let Ok(content) = fs::read_to_string(&path) {
            scan_jsonl_for_memory_edits(
                &content,
                memory_path,
                &session_id,
                &workspace_name,
                &mut history,
            );
        }

        // Also scan subagent directories
        let session_dir = project_dir.join(&session_id);
        let subagents_dir = session_dir.join("subagents");
        if subagents_dir.is_dir() {
            if let Ok(agent_entries) = fs::read_dir(&subagents_dir) {
                for agent_entry in agent_entries.flatten() {
                    let agent_path = agent_entry.path();
                    if agent_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let agent_id = agent_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    if let Ok(content) = fs::read_to_string(&agent_path) {
                        scan_jsonl_for_memory_edits(
                            &content,
                            memory_path,
                            &agent_id,
                            &workspace_name,
                            &mut history,
                        );
                    }
                }
            }
        }
    }

    // Sort by timestamp ascending
    history.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    history
}

fn scan_jsonl_for_memory_edits(
    content: &str,
    target_memory_path: &str,
    session_id: &str,
    workspace_name: &str,
    history: &mut Vec<MemoryHistoryEntry>,
) {
    // We need the filename to match loosely (both absolute path and relative references)
    let target_name = Path::new(target_memory_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let mut last_timestamp = String::new();

    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Track timestamps
        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
            last_timestamp = ts.to_string();
        }

        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }

        let Some(blocks) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };

        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }

            let tool_name = block.get("name").and_then(|n| n.as_str()).unwrap_or_default();
            let input = block.get("input");

            match tool_name {
                "Write" => {
                    let file_path = input
                        .and_then(|i| i.get("file_path"))
                        .and_then(|p| p.as_str())
                        .unwrap_or_default();

                    if path_matches_memory(file_path, target_memory_path, target_name) {
                        let content_val = input
                            .and_then(|i| i.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_string();

                        history.push(MemoryHistoryEntry {
                            session_id: session_id.to_string(),
                            workspace_name: workspace_name.to_string(),
                            timestamp: last_timestamp.clone(),
                            tool: "Write".to_string(),
                            detail: MemoryEditDetail::Write { content: content_val },
                        });
                    }
                }
                "Edit" => {
                    let file_path = input
                        .and_then(|i| i.get("file_path"))
                        .and_then(|p| p.as_str())
                        .unwrap_or_default();

                    if path_matches_memory(file_path, target_memory_path, target_name) {
                        let old_string = input
                            .and_then(|i| i.get("old_string"))
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let new_string = input
                            .and_then(|i| i.get("new_string"))
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string();

                        history.push(MemoryHistoryEntry {
                            session_id: session_id.to_string(),
                            workspace_name: workspace_name.to_string(),
                            timestamp: last_timestamp.clone(),
                            tool: "Edit".to_string(),
                            detail: MemoryEditDetail::Edit {
                                old_string,
                                new_string,
                            },
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

/// Check if a file_path from a tool call matches the target memory file.
fn path_matches_memory(file_path: &str, target_path: &str, target_name: &str) -> bool {
    // Exact match
    if file_path == target_path {
        return true;
    }
    // Check if the path ends with /memory/<target_name>
    if file_path.ends_with(&format!("/memory/{}", target_name)) {
        return true;
    }
    false
}

fn decode_project_key(encoded: &str) -> String {
    let stripped = encoded.trim_start_matches('-');
    format!("/{}", stripped.replace('-', "/"))
}
