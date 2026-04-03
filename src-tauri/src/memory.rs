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
        let workspace_name = match workspace_path
            .split('/')
            .filter(|s| !s.is_empty())
            .last()
        {
            Some(name) => name.to_string(),
            None => continue, // skip degenerate keys that decode to "/"
        };

        // Check for CLAUDE.md — skip if workspace resolves into a TCC-protected
        // directory (e.g. ~/Downloads) to avoid triggering macOS permission dialogs.
        let ws_root = Path::new(&workspace_path);
        let has_claude_md = if crate::tcc::is_tcc_protected(ws_root) {
            false
        } else {
            ws_root.join("CLAUDE.md").exists()
                || ws_root.join(".claude").join("CLAUDE.md").exists()
        };

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

    // Scan global memory (~/.claude/memory/)
    let global_memory_dir = claude_dir.join("memory");
    let global_has_claude_md = claude_dir.join("CLAUDE.md").is_file();

    // Show global entry if it has memory files OR a CLAUDE.md
    if global_memory_dir.is_dir() || global_has_claude_md {
        let mut files = Vec::new();
        if global_memory_dir.is_dir() {
            if let Ok(mem_entries) = fs::read_dir(&global_memory_dir) {
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
        }
        if !files.is_empty() || global_has_claude_md {
            files.sort_by(|a, b| {
                let a_is_index = a.name == "MEMORY.md";
                let b_is_index = b.name == "MEMORY.md";
                b_is_index.cmp(&a_is_index).then(a.name.cmp(&b.name))
            });
            results.insert(
                0,
                WorkspaceMemory {
                    workspace_name: "(Global)".to_string(),
                    // Point to ~/.claude/ so read_claude_md finds ~/.claude/CLAUDE.md
                    workspace_path: claude_dir.to_string_lossy().to_string(),
                    project_key: "__global__".to_string(),
                    has_claude_md: global_has_claude_md,
                    files,
                },
            );
        }
    }

    // Sort project memories by workspace name (global stays at front)
    let global = if results.first().map_or(false, |r| r.project_key == "__global__") {
        Some(results.remove(0))
    } else {
        None
    };
    results.sort_by(|a, b| a.workspace_name.cmp(&b.workspace_name));
    if let Some(g) = global {
        results.insert(0, g);
    }
    results
}

// ── Read a memory file's content ─────────────────────────────────────────────

pub fn read_memory_file(path: &str) -> Result<String, String> {
    // Safety: only allow reading from ~/.claude/projects/*/memory/ or ~/.claude/memory/
    let claude_dir = get_claude_dir().ok_or("cannot determine home dir")?;
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;

    let projects_dir = claude_dir.join("projects");
    let global_memory_dir = claude_dir.join("memory");

    let allowed = if let Ok(p) = fs::canonicalize(&projects_dir) {
        canonical.starts_with(&p)
    } else {
        false
    } || if let Ok(g) = fs::canonicalize(&global_memory_dir) {
        canonical.starts_with(&g)
    } else {
        false
    };

    if !allowed {
        return Err("path is outside allowed memory directories".into());
    }

    fs::read_to_string(path).map_err(|e| e.to_string())
}

// ── Read CLAUDE.md from a workspace ─────────────────────────────────────────

pub fn read_claude_md(workspace_path: &str) -> Result<String, String> {
    let root = Path::new(workspace_path);
    // Skip TCC-protected paths to avoid macOS permission dialogs.
    if crate::tcc::is_tcc_protected(root) {
        return Err("CLAUDE.md not found (TCC-protected workspace)".into());
    }
    // Try <workspace>/CLAUDE.md first, then <workspace>/.claude/CLAUDE.md
    let candidates = [
        root.join("CLAUDE.md"),
        root.join(".claude").join("CLAUDE.md"),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return fs::read_to_string(candidate).map_err(|e| e.to_string());
        }
    }
    Err("CLAUDE.md not found".into())
}

// ── Promote a memory file into a CLAUDE.md ──────────────────────────────────

/// Move a memory file's content into a CLAUDE.md file.
///
/// `target` must be `"project"` or `"global"`.
///   - project → appends to `<workspace_path>/CLAUDE.md`
///   - global  → appends to `~/.claude/CLAUDE.md`
///
/// After appending, the memory file is deleted and MEMORY.md is updated.
pub fn promote_memory(
    memory_path: &str,
    target: &str,
    workspace_path: &str,
) -> Result<(), String> {
    let mem_pb = PathBuf::from(memory_path);
    if !mem_pb.is_file() {
        return Err("memory file not found".into());
    }

    // Read and strip frontmatter
    let raw = fs::read_to_string(&mem_pb).map_err(|e| e.to_string())?;
    let content = strip_frontmatter(&raw);
    if content.trim().is_empty() {
        return Err("memory file has no content after frontmatter".into());
    }

    // Determine target CLAUDE.md path
    let claude_md_path = match target {
        "project" => PathBuf::from(workspace_path).join("CLAUDE.md"),
        "global" => {
            let claude_dir = get_claude_dir().ok_or("cannot determine home dir")?;
            claude_dir.join("CLAUDE.md")
        }
        _ => return Err(format!("invalid target: {}", target)),
    };

    // Append to CLAUDE.md (create if it doesn't exist)
    let existing = if claude_md_path.is_file() {
        fs::read_to_string(&claude_md_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    let separator = if existing.is_empty() || existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };

    let new_content = format!("{}{}{}\n", existing, separator, content.trim());
    fs::write(&claude_md_path, new_content).map_err(|e| e.to_string())?;

    // Delete the memory file
    let mem_name = mem_pb
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    fs::remove_file(&mem_pb).map_err(|e| e.to_string())?;

    // Update MEMORY.md: remove the line referencing this file
    let memory_dir = mem_pb.parent().unwrap_or(Path::new(""));
    let memory_md = memory_dir.join("MEMORY.md");
    if memory_md.is_file() {
        if let Ok(index_content) = fs::read_to_string(&memory_md) {
            let updated: Vec<&str> = index_content
                .lines()
                .filter(|line| !line.contains(&format!("({})", mem_name)))
                .collect();
            let _ = fs::write(&memory_md, updated.join("\n") + "\n");
        }
    }

    Ok(())
}

/// Strip YAML frontmatter (--- ... ---) from the beginning of a markdown string.
fn strip_frontmatter(s: &str) -> &str {
    let trimmed = s.trim_start();
    if !trimmed.starts_with("---") {
        return s;
    }
    // Find the closing ---
    if let Some(end) = trimmed[3..].find("\n---") {
        let after = &trimmed[3 + end + 4..]; // skip past \n---
        // Skip the newline after closing ---
        if after.starts_with('\n') {
            &after[1..]
        } else {
            after
        }
    } else {
        s // no closing ---, return as-is
    }
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

/// Decode a Claude Code project key back to the original filesystem path.
///
/// The encoding replaces `/` with `-`, but directory names themselves may
/// contain `-`.  We greedily match the longest existing directory at each
/// level so that e.g. `-Users-hoveychen-workspace-claw-fleet` correctly
/// resolves to `/Users/hoveychen/workspace/claw-fleet` instead of the
/// incorrect `/Users/hoveychen/workspace/claude/fleet`.
fn decode_project_key(encoded: &str) -> String {
    let stripped = encoded.trim_start_matches('-');
    let parts: Vec<&str> = stripped.split('-').collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    crate::session::decode_workspace_path_with_parts(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── path_matches_memory tests ───────────────────────────────────────────

    #[test]
    fn path_match_exact() {
        assert!(path_matches_memory(
            "/Users/me/.claude/projects/foo/memory/MEMORY.md",
            "/Users/me/.claude/projects/foo/memory/MEMORY.md",
            "MEMORY.md",
        ));
    }

    #[test]
    fn path_match_suffix() {
        assert!(path_matches_memory(
            "/some/other/path/memory/feedback_testing.md",
            "/Users/me/.claude/projects/foo/memory/feedback_testing.md",
            "feedback_testing.md",
        ));
    }

    #[test]
    fn path_no_match() {
        assert!(!path_matches_memory(
            "/Users/me/.claude/projects/foo/memory/OTHER.md",
            "/Users/me/.claude/projects/foo/memory/MEMORY.md",
            "MEMORY.md",
        ));
    }

    #[test]
    fn path_no_match_similar_name() {
        // Shouldn't match a file with a similar but different name
        assert!(!path_matches_memory(
            "/memory/MEMORY.md.bak",
            "/some/memory/MEMORY.md",
            "MEMORY.md",
        ));
    }

    // ── scan_jsonl_for_memory_edits tests ───────────────────────────────────

    #[test]
    fn scan_finds_write_tool_call() {
        let jsonl = json!({
            "type": "assistant",
            "timestamp": "2026-03-01T10:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Write",
                    "input": {
                        "file_path": "/home/user/.claude/projects/foo/memory/MEMORY.md",
                        "content": "# Memory\n\nSome content."
                    }
                }]
            }
        }).to_string();

        let mut history = Vec::new();
        scan_jsonl_for_memory_edits(
            &jsonl,
            "/home/user/.claude/projects/foo/memory/MEMORY.md",
            "sess1",
            "my-project",
            &mut history,
        );

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool, "Write");
        assert_eq!(history[0].session_id, "sess1");
        assert_eq!(history[0].timestamp, "2026-03-01T10:00:00Z");
        match &history[0].detail {
            MemoryEditDetail::Write { content } => {
                assert!(content.contains("# Memory"));
            }
            _ => panic!("Expected Write detail"),
        }
    }

    #[test]
    fn scan_finds_edit_tool_call() {
        let jsonl = json!({
            "type": "assistant",
            "timestamp": "2026-03-01T11:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Edit",
                    "input": {
                        "file_path": "/home/user/.claude/projects/foo/memory/MEMORY.md",
                        "old_string": "old text",
                        "new_string": "new text"
                    }
                }]
            }
        }).to_string();

        let mut history = Vec::new();
        scan_jsonl_for_memory_edits(
            &jsonl,
            "/home/user/.claude/projects/foo/memory/MEMORY.md",
            "sess2",
            "my-project",
            &mut history,
        );

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].tool, "Edit");
        match &history[0].detail {
            MemoryEditDetail::Edit { old_string, new_string } => {
                assert_eq!(old_string, "old text");
                assert_eq!(new_string, "new text");
            }
            _ => panic!("Expected Edit detail"),
        }
    }

    #[test]
    fn scan_ignores_non_matching_paths() {
        let jsonl = json!({
            "type": "assistant",
            "timestamp": "2026-03-01T12:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Write",
                    "input": {
                        "file_path": "/home/user/some/other/file.rs",
                        "content": "fn main() {}"
                    }
                }]
            }
        }).to_string();

        let mut history = Vec::new();
        scan_jsonl_for_memory_edits(
            &jsonl,
            "/home/user/.claude/projects/foo/memory/MEMORY.md",
            "sess3",
            "project",
            &mut history,
        );

        assert!(history.is_empty());
    }

    #[test]
    fn scan_ignores_user_messages() {
        let jsonl = json!({
            "type": "user",
            "timestamp": "2026-03-01T12:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Write",
                    "input": {
                        "file_path": "/home/user/.claude/projects/foo/memory/MEMORY.md",
                        "content": "should not match"
                    }
                }]
            }
        }).to_string();

        let mut history = Vec::new();
        scan_jsonl_for_memory_edits(
            &jsonl,
            "/home/user/.claude/projects/foo/memory/MEMORY.md",
            "sess4",
            "project",
            &mut history,
        );

        assert!(history.is_empty());
    }

    #[test]
    fn scan_handles_multiple_lines_and_timestamp_tracking() {
        let line1 = json!({
            "type": "assistant",
            "timestamp": "2026-03-01T10:00:00Z",
            "message": { "content": [] }
        }).to_string();
        let line2 = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Write",
                    "input": {
                        "file_path": "/memory/MEMORY.md",
                        "content": "updated"
                    }
                }]
            }
        }).to_string();
        let content = format!("{}\n{}", line1, line2);

        let mut history = Vec::new();
        scan_jsonl_for_memory_edits(
            &content,
            "/some/path/memory/MEMORY.md",
            "sess5",
            "proj",
            &mut history,
        );

        assert_eq!(history.len(), 1);
        // Should carry forward the timestamp from line1
        assert_eq!(history[0].timestamp, "2026-03-01T10:00:00Z");
    }

    #[test]
    fn scan_handles_invalid_json_gracefully() {
        let content = "not valid json\n{also invalid}\n";
        let mut history = Vec::new();
        scan_jsonl_for_memory_edits(
            content,
            "/memory/MEMORY.md",
            "sess6",
            "proj",
            &mut history,
        );
        assert!(history.is_empty());
    }

    // ── decode_project_key tests ──────────────────────────────────────────

    #[test]
    fn decode_project_key_single_dash_returns_slash() {
        // A project directory named "-" decodes to "/" which causes a lone
        // "/" to appear as the workspace name in the Memory panel.
        let result = decode_project_key("-");
        // This demonstrates the bug: the decoded path is just "/"
        assert_eq!(result, "/");
    }

    #[test]
    fn decode_project_key_empty_string() {
        let result = decode_project_key("");
        // Empty string splits into [""] which also produces "/"
        assert_eq!(result, "/");
    }

    #[test]
    fn workspace_name_from_root_path_is_none() {
        // A workspace_path of "/" has no meaningful last component,
        // so scan_all_memories should skip it (continue).
        let workspace_path = "/";
        let workspace_name = workspace_path
            .split('/')
            .filter(|s| !s.is_empty())
            .last();
        assert!(workspace_name.is_none());
    }

    #[test]
    fn decode_project_key_normal_path() {
        // Normal project keys should decode correctly (filesystem-dependent,
        // but the naive fallback still produces a reasonable path).
        let result = decode_project_key("-tmp");
        // /tmp exists on macOS/Linux, so greedy matching finds it
        assert!(result.starts_with('/'));
        assert!(result.len() > 1);
    }
}
