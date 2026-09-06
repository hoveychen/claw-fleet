//! Session notes — an agent's private, incremental checkpoint store that
//! survives context compaction and handoff relays.
//!
//! Modelled on Codex's `notes` tool namespace (`ext/history-notes` in
//! openai/codex), minus the OpenAI backend: Codex proxies every call to
//! `alpha/notes/v2/*` and the content is encrypted end to end, so the client
//! never sees it. Fleet keeps the same *shape* (virtual paths, write / append /
//! read-by-line-range / list-by-prefix / literal-substring search, 1 MB per
//! file) but stores plain files under `~/.fleet/notes/<session_id>/`, which is
//! what lets the desktop and the `SessionStart(compact)` hook read them back.
//!
//! # Scope
//!
//! - **Writes** are limited to the calling session's own directory.
//! - **Reads** (read / list / search) cover the calling session plus every
//!   *predecessor* on its handoff chain ([`crate::handoff::chain_containing`]),
//!   so a relay successor inherits the notes the previous hop took without any
//!   copying. Successors are not visible to predecessors — a retired session
//!   woken late must not read notes written after it handed off.
//!
//! Paths are virtual: relative, `/`-separated, no empty / `.` / `..` components,
//! no leading slash. They are resolved under the session directory only after
//! validation, so a note path can never escape it.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// Per-file ceiling, matching Codex's documented 1,000,000 UTF-8 bytes.
pub const MAX_NOTE_FILE_BYTES: usize = 1_000_000;

/// A note file as listed back to the agent.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteFile {
    /// Virtual path within the owning session's notes directory.
    pub path: String,
    /// Session that owns (wrote) the file.
    pub session_id: String,
    pub bytes: u64,
    pub updated_ms: u64,
}

/// One matching line from [`search`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteMatch {
    pub path: String,
    pub session_id: String,
    /// 1-based line number.
    pub line: usize,
    pub text: String,
}

fn notes_root() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("notes"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Validation ───────────────────────────────────────────────────────────────

/// Reject session ids that could not be a directory name. Ids are uuids
/// (Claude) or uuid-shaped thread ids (Codex); anything with a separator or
/// dot-run is refused rather than sanitised, so a bad id cannot alias another
/// session's directory.
fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("session id is empty".to_string());
    }
    if session_id
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        return Err(format!("session id `{session_id}` is not a plain identifier"));
    }
    Ok(())
}

/// Validate a virtual note path and return its components.
///
/// Rules (aligned with Codex's `notes` description): relative, `/`-separated,
/// no empty / `.` / `..` components, no backslashes or NUL, and `~` is *not*
/// expanded (it is an ordinary character — but we still refuse it as a leading
/// component to keep the error obvious rather than silently literal).
pub fn validate_path(path: &str) -> Result<Vec<String>, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("note path is empty".to_string());
    }
    if path.starts_with('/') {
        return Err(format!("note path `{path}` must be relative (paths are virtual, not filesystem paths)"));
    }
    if path.contains('\\') || path.contains('\0') {
        return Err(format!("note path `{path}` contains an unsupported character"));
    }
    if path.starts_with('~') {
        return Err(format!("note path `{path}` must not start with `~` (no shell expansion)"));
    }
    let mut parts = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" => return Err(format!("note path `{path}` has an empty component")),
            "." | ".." => {
                return Err(format!("note path `{path}` must not contain `.` or `..` components"))
            }
            _ => parts.push(comp.to_string()),
        }
    }
    Ok(parts)
}

fn resolve(root: &Path, session_id: &str, path: &str) -> Result<PathBuf, String> {
    validate_session_id(session_id)?;
    let parts = validate_path(path)?;
    let mut p = root.join(session_id);
    for part in parts {
        p.push(part);
    }
    Ok(p)
}

// ── Scope ────────────────────────────────────────────────────────────────────

/// Sessions whose notes `session_id` may read: itself first, then its handoff
/// predecessors from the nearest hop back to the chain's origin.
pub fn readable_sessions(session_id: &str) -> Vec<String> {
    readable_sessions_with(session_id, crate::handoff::chain_containing(session_id).as_ref())
}

/// Pure core of [`readable_sessions`]: `chain` is the relay chain the session
/// sits on, if any.
pub fn readable_sessions_with(
    session_id: &str,
    chain: Option<&crate::handoff::HandoffChain>,
) -> Vec<String> {
    let mut out = vec![session_id.to_string()];
    if let Some(chain) = chain {
        let ids = chain.session_ids();
        if let Some(pos) = ids.iter().position(|s| s == session_id) {
            for id in ids[..pos].iter().rev() {
                if !out.contains(id) {
                    out.push(id.clone());
                }
            }
        }
    }
    out
}

// ── Write / append ───────────────────────────────────────────────────────────

/// Create or replace a note file in the calling session's directory.
pub fn write(session_id: &str, path: &str, text: &str) -> Result<NoteFile, String> {
    let root = notes_root().ok_or("cannot determine home dir")?;
    write_in(&root, session_id, path, text)
}

pub fn write_in(root: &Path, session_id: &str, path: &str, text: &str) -> Result<NoteFile, String> {
    if text.len() > MAX_NOTE_FILE_BYTES {
        return Err(format!(
            "note would be {} bytes; every file must stay at or below {MAX_NOTE_FILE_BYTES} bytes — split it into another file",
            text.len()
        ));
    }
    let full = resolve(root, session_id, path)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    // Write-then-rename so a reader (the compact hook, the desktop) never sees a
    // half-written file.
    let tmp = full.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &full).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {}: {e}", full.display())
    })?;
    stat(root, session_id, path, &full)
}

/// Append text exactly as provided to a note file, creating it if absent.
pub fn append(session_id: &str, path: &str, text: &str) -> Result<NoteFile, String> {
    let root = notes_root().ok_or("cannot determine home dir")?;
    append_in(&root, session_id, path, text)
}

pub fn append_in(root: &Path, session_id: &str, path: &str, text: &str) -> Result<NoteFile, String> {
    let full = resolve(root, session_id, path)?;
    let existing = fs::metadata(&full).map(|m| m.len() as usize).unwrap_or(0);
    if existing + text.len() > MAX_NOTE_FILE_BYTES {
        return Err(format!(
            "appending {} bytes to a {existing}-byte note would exceed {MAX_NOTE_FILE_BYTES} bytes — create another file",
            text.len()
        ));
    }
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&full)
        .map_err(|e| format!("open {}: {e}", full.display()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("append {}: {e}", full.display()))?;
    stat(root, session_id, path, &full)
}

fn stat(_root: &Path, session_id: &str, path: &str, full: &Path) -> Result<NoteFile, String> {
    let meta = fs::metadata(full).map_err(|e| format!("stat {}: {e}", full.display()))?;
    Ok(NoteFile {
        path: path.trim().to_string(),
        session_id: session_id.to_string(),
        bytes: meta.len(),
        updated_ms: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or_else(now_ms),
    })
}

// ── Read ─────────────────────────────────────────────────────────────────────

/// Read a note by virtual path. The calling session's own file wins; otherwise
/// the nearest predecessor on the handoff chain that has one.
///
/// `start_line` / `stop_line` are 1-based and inclusive; negative values count
/// back from the final line (`-1` = last line), matching Codex's semantics.
pub fn read(
    session_id: &str,
    path: &str,
    start_line: Option<i64>,
    stop_line: Option<i64>,
) -> Result<String, String> {
    let root = notes_root().ok_or("cannot determine home dir")?;
    read_in(&root, &readable_sessions(session_id), path, start_line, stop_line)
}

pub fn read_in(
    root: &Path,
    readable: &[String],
    path: &str,
    start_line: Option<i64>,
    stop_line: Option<i64>,
) -> Result<String, String> {
    for sid in readable {
        let full = resolve(root, sid, path)?;
        if full.is_file() {
            let text = fs::read_to_string(&full).map_err(|e| format!("read {}: {e}", full.display()))?;
            return Ok(slice_lines(&text, start_line, stop_line));
        }
    }
    Err(format!("no note at `{}`", path.trim()))
}

/// Apply a 1-based inclusive, negative-aware line range to `text`.
fn slice_lines(text: &str, start_line: Option<i64>, stop_line: Option<i64>) -> String {
    if start_line.is_none() && stop_line.is_none() {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len() as i64;
    let norm = |v: i64| -> i64 {
        if v < 0 {
            n + v + 1
        } else {
            v
        }
    };
    let start = start_line.map(norm).unwrap_or(1).max(1);
    let stop = stop_line.map(norm).unwrap_or(n).min(n);
    if n == 0 || start > stop {
        return String::new();
    }
    lines[(start - 1) as usize..stop as usize].join("\n")
}

// ── List ─────────────────────────────────────────────────────────────────────

/// List note files visible to `session_id` (own first, then predecessors),
/// optionally filtered by virtual-path prefix. Newest-updated first within a
/// session.
pub fn list(session_id: &str, prefix: Option<&str>) -> Result<Vec<NoteFile>, String> {
    let root = notes_root().ok_or("cannot determine home dir")?;
    list_in(&root, &readable_sessions(session_id), prefix)
}

pub fn list_in(root: &Path, readable: &[String], prefix: Option<&str>) -> Result<Vec<NoteFile>, String> {
    let prefix = prefix.map(str::trim).filter(|p| !p.is_empty());
    let mut out = Vec::new();
    for sid in readable {
        validate_session_id(sid)?;
        let dir = root.join(sid);
        if !dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        walk(&dir, &dir, &mut files);
        files.retain(|(rel, _)| prefix.map(|p| rel.starts_with(p)).unwrap_or(true));
        let mut entries: Vec<NoteFile> = files
            .into_iter()
            .filter_map(|(rel, full)| stat(root, sid, &rel, &full).ok())
            .collect();
        entries.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then_with(|| a.path.cmp(&b.path)));
        out.extend(entries);
    }
    Ok(out)
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(base, &p, out);
        } else if p.is_file() {
            // Skip in-flight temp files from `write_in`.
            if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.starts_with("tmp-")) {
                continue;
            }
            if let Ok(rel) = p.strip_prefix(base) {
                let rel = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((rel, p));
            }
        }
    }
}

// ── Search ───────────────────────────────────────────────────────────────────

/// Case-sensitive literal-substring search over note lines visible to
/// `session_id`. Files are visited own-session-first, newest-updated first.
pub fn search(
    session_id: &str,
    query: &str,
    path_prefix: Option<&str>,
    max_files: usize,
    max_matches_per_file: usize,
) -> Result<Vec<NoteMatch>, String> {
    let root = notes_root().ok_or("cannot determine home dir")?;
    search_in(
        &root,
        &readable_sessions(session_id),
        query,
        path_prefix,
        max_files,
        max_matches_per_file,
    )
}

pub fn search_in(
    root: &Path,
    readable: &[String],
    query: &str,
    path_prefix: Option<&str>,
    max_files: usize,
    max_matches_per_file: usize,
) -> Result<Vec<NoteMatch>, String> {
    if query.is_empty() {
        return Err("search query is empty".to_string());
    }
    let mut out = Vec::new();
    let mut files_hit = 0usize;
    for file in list_in(root, readable, path_prefix)? {
        if files_hit >= max_files.max(1) {
            break;
        }
        let full = resolve(root, &file.session_id, &file.path)?;
        let Ok(text) = fs::read_to_string(&full) else { continue };
        let mut matched = 0usize;
        for (i, line) in text.lines().enumerate() {
            if line.contains(query) {
                out.push(NoteMatch {
                    path: file.path.clone(),
                    session_id: file.session_id.clone(),
                    line: i + 1,
                    text: line.to_string(),
                });
                matched += 1;
                if matched >= max_matches_per_file.max(1) {
                    break;
                }
            }
        }
        if matched > 0 {
            files_hit += 1;
        }
    }
    Ok(out)
}

// ── Hint (post-compaction re-injection) ──────────────────────────────────────

/// Byte ceiling for the hint injected after a compaction — Codex's
/// `MAX_THREAD_HINT_BYTES`. A hint is a pointer back into the notes, not the
/// notes themselves.
pub const MAX_HINT_BYTES: usize = 4_000;

/// The text the `SessionStart(compact|resume|startup)` hook injects: a roster of
/// the note files visible to `session_id` plus the most recently updated one,
/// clipped to [`MAX_HINT_BYTES`]. `None` when there are no notes, so sessions
/// that never took any pay nothing (the hook stays silent).
pub fn render_hint(session_id: &str) -> Option<String> {
    let root = notes_root()?;
    render_hint_in(&root, session_id, &readable_sessions(session_id))
}

pub fn render_hint_in(root: &Path, session_id: &str, readable: &[String]) -> Option<String> {
    let files = list_in(root, readable, None).ok()?;
    if files.is_empty() {
        return None;
    }
    let mut out = String::from(
        "<fleet_notes>\nPrivate checkpoint notes from before this context window (own session \
         first, then handoff predecessors). Read the rest with fleet__notes read, and use \
         fleet__history search to recover details a compaction dropped. Internal bookkeeping — \
         do not narrate to the user.\n",
    );
    out.push_str(&format!("Files ({}):\n", files.len()));
    for f in &files {
        let owner = if f.session_id == session_id { "own" } else { "predecessor" };
        out.push_str(&format!("  {}  {} bytes  [{owner}]\n", f.path, f.bytes));
    }
    // Most recent write overall — a checkpoint written right before the
    // compaction is exactly what the next window needs first.
    if let Some(latest) = files.iter().max_by_key(|f| f.updated_ms) {
        if let Ok(full) = resolve(root, &latest.session_id, &latest.path) {
            if let Ok(text) = fs::read_to_string(&full) {
                out.push_str(&format!("--- {} (latest) ---\n", latest.path));
                let budget = MAX_HINT_BYTES.saturating_sub(out.len() + "</fleet_notes>\n".len() + 64);
                out.push_str(&clip_bytes(&text, budget));
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out.push_str("</fleet_notes>\n");
    Some(out)
}

/// Clip to `budget` bytes on a char boundary, marking the elision.
fn clip_bytes(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let mut end = budget.saturating_sub(24).min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [clipped; read the file for the rest]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{HandoffChain, HandoffLink};

    #[test]
    fn hint_is_none_without_notes_and_bounded_with_them() {
        let root = fresh_root("hint");
        let own = vec!["s1".to_string()];
        assert!(render_hint_in(&root, "s1", &own).is_none());

        write_in(&root, "s1", "old.md", "old stuff").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let big = "决策".repeat(5_000); // 30 KB of CJK, well over the ceiling
        write_in(&root, "s1", "checkpoint.md", &big).unwrap();

        let hint = render_hint_in(&root, "s1", &own).unwrap();
        assert!(hint.len() <= MAX_HINT_BYTES, "hint is {} bytes", hint.len());
        assert!(hint.starts_with("<fleet_notes>\n"));
        assert!(hint.ends_with("</fleet_notes>\n"));
        assert!(hint.contains("Files (2):"));
        assert!(hint.contains("old.md  9 bytes  [own]"));
        assert!(hint.contains("--- checkpoint.md (latest) ---"), "{hint}");
        assert!(hint.contains("[clipped;"));
        let _ = fs::remove_dir_all(&root);
    }

    fn fresh_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fleet-notes-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn chain(ids: &[&str]) -> HandoffChain {
        let links = ids
            .windows(2)
            .map(|w| HandoffLink {
                from_session_id: w[0].to_string(),
                to_session_id: w[1].to_string(),
                note: String::new(),
                plan_id: None,
                next_task: None,
                handed_at: 0,
            })
            .collect();
        HandoffChain {
            chain_id: "c".into(),
            workspace_path: "/ws".into(),
            plan_id: None,
            links,
        }
    }

    #[test]
    fn write_read_append_roundtrip() {
        let root = fresh_root("rw");
        let own = vec!["s1".to_string()];
        write_in(&root, "s1", "progress.md", "goal: x\n").unwrap();
        append_in(&root, "s1", "progress.md", "next: y\n").unwrap();
        assert_eq!(read_in(&root, &own, "progress.md", None, None).unwrap(), "goal: x\nnext: y\n");
        let listed = list_in(&root, &own, None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "progress.md");
        assert_eq!(listed[0].bytes, 16);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_replaces_and_nested_paths_work() {
        let root = fresh_root("nest");
        let own = vec!["s1".to_string()];
        write_in(&root, "s1", "a/b/c.md", "one").unwrap();
        write_in(&root, "s1", "a/b/c.md", "two").unwrap();
        assert_eq!(read_in(&root, &own, "a/b/c.md", None, None).unwrap(), "two");
        assert!(root.join("s1/a/b/c.md").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn path_traversal_and_bad_shapes_are_refused() {
        let root = fresh_root("trav");
        for bad in ["../x", "a/../x", "/etc/passwd", "", ".", "a//b", "a\\b", "~/x", "a/./b"] {
            assert!(write_in(&root, "s1", bad, "x").is_err(), "should refuse `{bad}`");
        }
        // Nothing escaped the root.
        assert!(!root.parent().unwrap().join("x").exists());
        // Session ids with separators are refused too.
        assert!(write_in(&root, "../s1", "a.md", "x").is_err());
        assert!(write_in(&root, "", "a.md", "x").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn size_ceiling_applies_to_write_and_append() {
        let root = fresh_root("size");
        let big = "x".repeat(MAX_NOTE_FILE_BYTES + 1);
        assert!(write_in(&root, "s1", "big.md", &big).is_err());
        let almost = "x".repeat(MAX_NOTE_FILE_BYTES - 1);
        write_in(&root, "s1", "big.md", &almost).unwrap();
        assert!(append_in(&root, "s1", "big.md", "yy").is_err());
        append_in(&root, "s1", "big.md", "y").unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn line_ranges_are_one_based_inclusive_and_negative_aware() {
        let text = "l1\nl2\nl3\nl4\n";
        assert_eq!(slice_lines(text, Some(2), Some(3)), "l2\nl3");
        assert_eq!(slice_lines(text, Some(-2), None), "l3\nl4");
        assert_eq!(slice_lines(text, None, Some(-3)), "l1\nl2");
        assert_eq!(slice_lines(text, Some(3), Some(2)), "");
        assert_eq!(slice_lines(text, Some(0), Some(99)), "l1\nl2\nl3\nl4");
        assert_eq!(slice_lines("", Some(1), Some(1)), "");
    }

    #[test]
    fn readable_scope_is_self_then_predecessors_never_successors() {
        let c = chain(&["a", "b", "c", "d"]);
        assert_eq!(readable_sessions_with("c", Some(&c)), vec!["c", "b", "a"]);
        assert_eq!(readable_sessions_with("a", Some(&c)), vec!["a"]);
        assert_eq!(readable_sessions_with("zzz", Some(&c)), vec!["zzz"]);
        assert_eq!(readable_sessions_with("solo", None), vec!["solo"]);
    }

    #[test]
    fn successor_reads_predecessor_notes_but_own_file_shadows() {
        let root = fresh_root("chain");
        write_in(&root, "a", "checkpoint.md", "from a").unwrap();
        write_in(&root, "a", "only-a.md", "a only").unwrap();
        write_in(&root, "b", "checkpoint.md", "from b").unwrap();
        let readable = readable_sessions_with("b", Some(&chain(&["a", "b"])));
        assert_eq!(read_in(&root, &readable, "checkpoint.md", None, None).unwrap(), "from b");
        assert_eq!(read_in(&root, &readable, "only-a.md", None, None).unwrap(), "a only");
        let paths: Vec<(String, String)> = list_in(&root, &readable, None)
            .unwrap()
            .into_iter()
            .map(|f| (f.session_id, f.path))
            .collect();
        // Own session first; the predecessor's files follow (newest-updated
        // first within a session, which is timing-dependent here — so only the
        // membership of that pair is pinned).
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], ("b".to_string(), "checkpoint.md".to_string()));
        let mut rest = paths[1..].to_vec();
        rest.sort();
        assert_eq!(
            rest,
            vec![
                ("a".to_string(), "checkpoint.md".to_string()),
                ("a".to_string(), "only-a.md".to_string()),
            ]
        );
        // The predecessor cannot see the successor.
        let a_only = readable_sessions_with("a", Some(&chain(&["a", "b"])));
        assert_eq!(list_in(&root, &a_only, None).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_is_literal_case_sensitive_and_bounded() {
        let root = fresh_root("search");
        write_in(&root, "s1", "n1.md", "Alpha one\nalpha two\nAlpha three\n").unwrap();
        write_in(&root, "s1", "n2.md", "Alpha four\n").unwrap();
        write_in(&root, "s1", "other/n3.md", "nothing\n").unwrap();
        let own = vec!["s1".to_string()];
        let hits = search_in(&root, &own, "Alpha", None, 10, 10).unwrap();
        assert_eq!(hits.len(), 3, "{hits:?}");
        assert!(hits.iter().all(|h| h.text.contains("Alpha")));
        let capped = search_in(&root, &own, "Alpha", None, 10, 1).unwrap();
        assert_eq!(capped.len(), 2);
        let one_file = search_in(&root, &own, "Alpha", None, 1, 10).unwrap();
        assert_eq!(one_file.iter().map(|h| h.path.as_str()).collect::<std::collections::HashSet<_>>().len(), 1);
        let scoped = search_in(&root, &own, "nothing", Some("other/"), 10, 10).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].line, 1);
        assert!(search_in(&root, &own, "", None, 10, 10).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_note_is_an_error_not_empty() {
        let root = fresh_root("missing");
        let own = vec!["s1".to_string()];
        assert!(read_in(&root, &own, "nope.md", None, None).is_err());
        assert!(list_in(&root, &own, None).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
