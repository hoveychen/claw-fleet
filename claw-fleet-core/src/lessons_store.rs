//! Managed store for daily-report lessons the user adds to their global Claude
//! guidance.
//!
//! Install strategy mirrors [`crate::model_guidance`] / `wiki_guidance`:
//!   1. Each added lesson lives as its own sentinel-wrapped block inside one
//!      managed file, `~/.claude/fleet-lessons.md`.
//!   2. A single sentinel-wrapped `@~/.claude/fleet-lessons.md` import is
//!      injected into `~/.claude/CLAUDE.md`.
//!
//! Unlike the guidance modules (one static file, one sentinel), the lessons
//! file holds **many** per-lesson blocks so individual lessons can be listed and
//! removed. The old behaviour appended a raw `# Lesson (…)` block straight into
//! `CLAUDE.md` body with no way to enumerate or undo it;
//! [`migrate_legacy_lessons`] lifts those raw blocks into the managed file.
//!
//! All core logic is pure `&str -> String` so it is unit-tested without touching
//! the real `~/.claude`; the public `add`/`remove`/`list`/`migrate` functions are
//! thin file-IO wrappers.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::daily_report::Lesson;

/// Sentinel around the `@import` line inside `~/.claude/CLAUDE.md`.
const IMPORT_BEGIN: &str = "<!-- fleet:lessons:begin -->";
const IMPORT_END: &str = "<!-- fleet:lessons:end -->";

/// Header written once at the top of `fleet-lessons.md`. Lives outside every
/// per-lesson sentinel, so the parser ignores it.
const FILE_HEADER: &str = "# Fleet Lessons (managed by Claw Fleet — do not edit)\n\n\
Lessons you added from the Fleet daily-report. Manage them from the desktop \
Memory panel — hand edits may be overwritten.\n";

/// A lesson currently recorded in the managed file, carrying the stable `id`
/// used to remove it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ManagedLesson {
    /// Stable identifier (derived from session + content); the sentinel key.
    pub id: String,
    pub content: String,
    pub reason: String,
    pub workspace_name: String,
    pub session_id: String,
}

// ── Paths ────────────────────────────────────────────────────────────────────

fn claude_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".claude"))
}

fn lessons_file_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("fleet-lessons.md"))
}

fn claude_md_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("CLAUDE.md"))
}

// ── Pure helpers (unit-tested) ───────────────────────────────────────────────

/// Deterministic id for a lesson. Used to key its sentinel block and to dedupe
/// re-adds of the same lesson. Removal always matches the id stored in the file,
/// so cross-version hash instability is irrelevant.
fn lesson_id(lesson: &Lesson) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lesson.session_id.hash(&mut h);
    lesson.content.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn block_begin(id: &str) -> String {
    format!("<!-- fleet:lesson:{id}:begin -->")
}

fn block_end(id: &str) -> String {
    format!("<!-- fleet:lesson:{id}:end -->")
}

/// Render one per-lesson sentinel block. The inner body reuses the exact
/// `# Lesson (…)` / `**Why:**` shape the legacy appender produced, so parsing is
/// symmetric with migration.
fn render_block(id: &str, lesson: &Lesson) -> String {
    format!(
        "{begin}\n# Lesson (from {ws}, session {sid})\n{content}\n\n**Why:** {reason}\n{end}\n",
        begin = block_begin(id),
        end = block_end(id),
        ws = lesson.workspace_name,
        sid = lesson.session_id,
        content = lesson.content,
        reason = lesson.reason,
    )
}

/// Remove the block for `id` (begin..end inclusive) from the managed file body.
/// No-op if absent; idempotent.
fn strip_block(content: &str, id: &str) -> String {
    let begin = block_begin(id);
    let end = block_end(id);
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == begin {
            in_block = true;
            continue;
        }
        if trimmed == end {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push_str(line);
        }
    }
    out
}

/// Whether any per-lesson block remains in the managed file body.
fn has_any_block(content: &str) -> bool {
    content
        .lines()
        .any(|l| l.trim().starts_with("<!-- fleet:lesson:") && l.trim().ends_with(":begin -->"))
}

/// Parse every per-lesson block into a [`ManagedLesson`] (the `list` logic).
fn parse_blocks(content: &str) -> Vec<ManagedLesson> {
    let mut out = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(id) = trimmed
            .strip_prefix("<!-- fleet:lesson:")
            .and_then(|r| r.strip_suffix(":begin -->"))
        {
            let end_marker = block_end(id);
            let mut body: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != end_marker {
                body.push(lines[i]);
                i += 1;
            }
            // i now points at the end marker (or EOF); skip it.
            if let Some(ml) = parse_block_body(id, &body) {
                out.push(ml);
            }
        }
        i += 1;
    }
    out
}

/// Parse the header (`workspace_name`, `session_id`) out of a `# Lesson (…)`
/// line. `None` if the line isn't a lesson header.
fn parse_header(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let inner = t.strip_prefix("# Lesson (from ")?.strip_suffix(")")?;
    // Session ids never contain ", session ", so rsplit is unambiguous even if a
    // workspace name (unlikely) contained a comma.
    let (ws, sid) = inner.rsplit_once(", session ")?;
    Some((ws.to_string(), sid.to_string()))
}

/// Turn a block's inner lines into a [`ManagedLesson`]. Inner shape:
/// `# Lesson (from ws, session sid)` / content… / blank / `**Why:** reason…`.
fn parse_block_body(id: &str, body: &[&str]) -> Option<ManagedLesson> {
    let (ws, sid) = parse_header(body.first()?)?;
    let rest = &body[1..];
    let why_idx = rest
        .iter()
        .position(|l| l.trim_start().starts_with("**Why:**"));
    let (content_lines, reason_lines): (&[&str], &[&str]) = match why_idx {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, &[]),
    };
    let content = content_lines.join("\n").trim().to_string();
    if content.is_empty() {
        return None;
    }
    let reason = reason_lines
        .join("\n")
        .trim()
        .strip_prefix("**Why:**")
        .unwrap_or("")
        .trim()
        .to_string();
    Some(ManagedLesson {
        id: id.to_string(),
        content,
        reason,
        workspace_name: ws,
        session_id: sid,
    })
}

/// Strip the `fleet:lessons` import sentinel block from `CLAUDE.md` body.
fn strip_import(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == IMPORT_BEGIN {
            in_block = true;
            continue;
        }
        if trimmed == IMPORT_END {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push_str(line);
        }
    }
    out
}

/// Inject (or re-inject) the `fleet:lessons` import sentinel pointing at
/// `import_path` into `CLAUDE.md` body. Idempotent.
fn inject_import(content: &str, import_path: &str) -> String {
    let stripped = strip_import(content);
    let block = format!("{IMPORT_BEGIN}\n@{import_path}\n{IMPORT_END}\n");
    if stripped.trim().is_empty() {
        block
    } else if stripped.ends_with('\n') {
        format!("{stripped}\n{block}")
    } else {
        format!("{stripped}\n\n{block}")
    }
}

/// Collapse runs of 3+ newlines to a single blank line — keeps CLAUDE.md tidy
/// after raw blocks are lifted out during migration.
fn collapse_blank_runs(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut newline_run = 0;
    for ch in content.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(ch);
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }
    out
}

/// Extract raw legacy `# Lesson (…)` blocks that sit **outside** any Fleet
/// sentinel in `CLAUDE.md`, returning the cleaned body plus the parsed lessons.
/// Pure; [`migrate_legacy_lessons`] does the file IO around it.
fn extract_raw_lessons(content: &str) -> (String, Vec<Lesson>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut remaining = String::with_capacity(content.len());
    let mut lessons: Vec<Lesson> = Vec::new();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        // Track Fleet sentinel nesting so we never touch wrapped content.
        if t.starts_with("<!-- fleet:") && t.ends_with(":begin -->") {
            depth += 1;
            remaining.push_str(line);
            remaining.push('\n');
            i += 1;
            continue;
        }
        if t.starts_with("<!-- fleet:") && t.ends_with(":end -->") {
            depth = depth.saturating_sub(1);
            remaining.push_str(line);
            remaining.push('\n');
            i += 1;
            continue;
        }
        if depth == 0 {
            if let Some((ws, sid)) = parse_header(line) {
                // A raw block is: one content paragraph, then optional
                // `**Why:**` / `**How to apply:**` paragraphs. Anything after
                // that — e.g. the user's own hand-written rules sitting right
                // below the last lesson — is NOT part of the block. (A looser
                // "swallow until the next lesson/sentinel/@import" boundary
                // once migrated 293 lines of user rules into a lesson body.)
                let is_boundary = |l2: &str| {
                    parse_header(l2).is_some()
                        || l2.trim().starts_with("<!-- fleet:")
                        || l2.starts_with('@')
                };
                let mut body: Vec<&str> = Vec::new();
                i += 1;
                // Content paragraph: up to the first blank line.
                while i < lines.len() {
                    let l2 = lines[i];
                    if l2.trim().is_empty() || is_boundary(l2) {
                        break;
                    }
                    body.push(l2);
                    i += 1;
                }
                // Trailing annotation paragraphs, each separated by blanks.
                loop {
                    let mut j = i;
                    while j < lines.len() && lines[j].trim().is_empty() {
                        j += 1;
                    }
                    let Some(&next) = lines.get(j) else { break };
                    let nt = next.trim_start();
                    if !(nt.starts_with("**Why:**") || nt.starts_with("**How to apply:**")) {
                        break;
                    }
                    while i < j {
                        body.push(lines[i]);
                        i += 1;
                    }
                    while i < lines.len() {
                        let l2 = lines[i];
                        if l2.trim().is_empty() || is_boundary(l2) {
                            break;
                        }
                        body.push(l2);
                        i += 1;
                    }
                }
                if let Some(lesson) = parse_raw_body(ws, sid, &body) {
                    lessons.push(lesson);
                }
                // Drop the raw block from `remaining`.
                continue;
            }
        }
        remaining.push_str(line);
        remaining.push('\n');
        i += 1;
    }
    (collapse_blank_runs(&remaining), lessons)
}

fn parse_raw_body(ws: String, sid: String, body: &[&str]) -> Option<Lesson> {
    let why_idx = body
        .iter()
        .position(|l| l.trim_start().starts_with("**Why:**"));
    let (content_lines, reason_lines): (&[&str], &[&str]) = match why_idx {
        Some(idx) => (&body[..idx], &body[idx..]),
        None => (body, &[]),
    };
    let content = content_lines.join("\n").trim().to_string();
    if content.is_empty() {
        return None;
    }
    let reason = reason_lines
        .join("\n")
        .trim()
        .strip_prefix("**Why:**")
        .unwrap_or("")
        .trim()
        .to_string();
    Some(Lesson {
        content,
        reason,
        workspace_name: ws,
        session_id: sid,
    })
}

// ── Public file-IO API ───────────────────────────────────────────────────────

/// Add a lesson to the managed file and ensure the CLAUDE.md import is present.
/// Re-adding the same lesson (same id) replaces its block. Returns the id.
pub fn add_lesson(lesson: &Lesson) -> Result<String, String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    let id = lesson_id(lesson);
    let path = lessons_file_path().ok_or("cannot determine home dir")?;

    let existing = fs::read_to_string(&path).unwrap_or_default();
    let stripped = strip_block(&existing, &id);
    let base = if stripped.trim().is_empty() {
        FILE_HEADER.to_string()
    } else {
        stripped
    };
    let mut new_content = base;
    if !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    if !new_content.ends_with("\n\n") {
        new_content.push('\n');
    }
    new_content.push_str(&render_block(&id, lesson));
    fs::write(&path, new_content).map_err(|e| format!("write fleet-lessons.md: {e}"))?;

    ensure_import_installed()?;
    Ok(id)
}

/// Remove the lesson with `id`. If it was the last one, delete the managed file
/// and strip the CLAUDE.md import. Idempotent.
pub fn remove_lesson(id: &str) -> Result<(), String> {
    let path = lessons_file_path().ok_or("cannot determine home dir")?;
    let existing = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(()), // file gone → nothing to remove
    };
    let stripped = strip_block(&existing, id);
    if has_any_block(&stripped) {
        fs::write(&path, stripped).map_err(|e| format!("write fleet-lessons.md: {e}"))?;
    } else {
        // No lessons left — remove the file and its import entirely.
        let _ = fs::remove_file(&path);
        remove_import()?;
    }
    Ok(())
}

/// List all lessons currently in the managed file.
///
/// Runs the one-time legacy migration first (idempotent, no-op once done) so the
/// raw `# Lesson (…)` blocks the old appender left in CLAUDE.md body get folded
/// into the managed file the first time anything lists — the report card and the
/// Memory panel both list on mount. Safe from recursion: migration calls
/// [`add_lesson`], which never lists.
pub fn list_lessons() -> Vec<ManagedLesson> {
    let _ = migrate_legacy_lessons();
    let Some(path) = lessons_file_path() else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_blocks(&content)
}

/// Ensure the `@fleet-lessons.md` import sentinel is present in CLAUDE.md.
fn ensure_import_installed() -> Result<(), String> {
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let path = lessons_file_path().ok_or("cannot determine home dir")?;
    let existing = fs::read_to_string(&claude_md).unwrap_or_default();
    let new_content = inject_import(&existing, &path.display().to_string());
    if new_content != existing {
        fs::write(&claude_md, new_content).map_err(|e| format!("write CLAUDE.md: {e}"))?;
    }
    Ok(())
}

/// Strip the `@fleet-lessons.md` import sentinel from CLAUDE.md.
fn remove_import() -> Result<(), String> {
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    if let Ok(existing) = fs::read_to_string(&claude_md) {
        let stripped = strip_import(&existing);
        if stripped != existing {
            fs::write(&claude_md, stripped).map_err(|e| format!("write CLAUDE.md: {e}"))?;
        }
    }
    Ok(())
}

/// One-time migration: lift raw `# Lesson (…)` blocks the old appender left in
/// `~/.claude/CLAUDE.md` into the managed file. Returns how many were migrated.
/// Idempotent — a second run finds nothing to move.
pub fn migrate_legacy_lessons() -> Result<usize, String> {
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let content = match fs::read_to_string(&claude_md) {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };
    let (remaining, lessons) = extract_raw_lessons(&content);
    if lessons.is_empty() {
        return Ok(0);
    }
    // Clean CLAUDE.md first, then re-home each lesson (add_lesson re-adds the
    // import at the end).
    fs::write(&claude_md, remaining).map_err(|e| format!("write CLAUDE.md: {e}"))?;
    let mut n = 0;
    for l in &lessons {
        add_lesson(l)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(content: &str, reason: &str, ws: &str, sid: &str) -> Lesson {
        Lesson {
            content: content.to_string(),
            reason: reason.to_string(),
            workspace_name: ws.to_string(),
            session_id: sid.to_string(),
        }
    }

    #[test]
    fn import_markers_unique_vs_other_fleet_blocks() {
        assert!(IMPORT_BEGIN.contains("fleet:lessons"));
        assert_ne!(IMPORT_BEGIN, "<!-- fleet:model-guidance:begin -->");
        assert_ne!(IMPORT_BEGIN, "<!-- fleet:wiki-guidance:begin -->");
        assert_ne!(IMPORT_BEGIN, "<!-- fleet:interaction-mode:begin -->");
        assert_ne!(IMPORT_BEGIN, "<!-- fleet:prd-discipline:begin -->");
    }

    #[test]
    fn render_then_parse_round_trips() {
        let l = mk("Always verify.", "Got bitten once.", "claude-fleet", "abc-123");
        let id = lesson_id(&l);
        let file = format!("{FILE_HEADER}\n\n{}", render_block(&id, &l));
        let parsed = parse_blocks(&file);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, id);
        assert_eq!(parsed[0].content, "Always verify.");
        assert_eq!(parsed[0].reason, "Got bitten once.");
        assert_eq!(parsed[0].workspace_name, "claude-fleet");
        assert_eq!(parsed[0].session_id, "abc-123");
    }

    #[test]
    fn parse_handles_multi_paragraph_content() {
        let l = mk("Line one.\n\nLine two.", "Because reasons.", "ws", "sid");
        let id = lesson_id(&l);
        let parsed = parse_blocks(&render_block(&id, &l));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].content, "Line one.\n\nLine two.");
        assert_eq!(parsed[0].reason, "Because reasons.");
    }

    #[test]
    fn strip_block_removes_only_target() {
        let a = mk("A content", "A why", "w", "s1");
        let b = mk("B content", "B why", "w", "s2");
        let ida = lesson_id(&a);
        let idb = lesson_id(&b);
        let file = format!("{}{}", render_block(&ida, &a), render_block(&idb, &b));
        let after = strip_block(&file, &ida);
        assert!(!after.contains("A content"));
        assert!(after.contains("B content"));
        // idempotent
        assert_eq!(strip_block(&after, &ida), after);
        // removing the last leaves no blocks
        let empty = strip_block(&after, &idb);
        assert!(!has_any_block(&empty));
    }

    #[test]
    fn add_same_id_replaces_not_duplicates() {
        // Simulate add_lesson's dedupe at the string level.
        let l = mk("Same content", "why1", "w", "s");
        let id = lesson_id(&l);
        let file = render_block(&id, &l);
        // second add with edited reason but same content+session → same id
        let l2 = mk("Same content", "why2", "w", "s");
        assert_eq!(lesson_id(&l2), id);
        let replaced = format!("{}{}", strip_block(&file, &id), render_block(&id, &l2));
        let parsed = parse_blocks(&replaced);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].reason, "why2");
    }

    #[test]
    fn inject_and_strip_import_round_trip() {
        let claude = "user rules here\n";
        let injected = inject_import(claude, "/home/x/.claude/fleet-lessons.md");
        assert!(injected.contains(IMPORT_BEGIN));
        assert!(injected.contains("@/home/x/.claude/fleet-lessons.md"));
        assert!(injected.starts_with("user rules here\n"));
        // idempotent: injecting again doesn't duplicate
        let twice = inject_import(&injected, "/home/x/.claude/fleet-lessons.md");
        assert_eq!(twice.matches(IMPORT_BEGIN).count(), 1);
        // strip removes the block and keeps the user's rules (a trailing blank
        // line separator may remain — harmless).
        let stripped = strip_import(&injected);
        assert!(!stripped.contains(IMPORT_BEGIN));
        assert!(!stripped.contains("fleet-lessons.md"));
        assert!(stripped.starts_with("user rules here\n"));
    }

    #[test]
    fn inject_into_empty_yields_just_block() {
        let injected = inject_import("", "/p/fleet-lessons.md");
        assert!(injected.starts_with(IMPORT_BEGIN));
        assert!(injected.trim_end().ends_with(IMPORT_END));
    }

    #[test]
    fn migrate_lifts_raw_blocks_and_preserves_sentinels() {
        // A CLAUDE.md with: prose, two raw lessons, a sentinel guidance block,
        // and a trailing @import — mirrors the real file layout.
        let claude = "\
Some global rules.\n\
\n\
# Lesson (from claude-fleet, session 2778efcb)\n\
恢复前必须先验证。\n\
\n\
**Why:** 直接回滚会重现原问题。\n\
\n\
# Lesson (from netferry, session 2026-04-07)\n\
subagent 结论要交叉验证。\n\
\n\
**Why:** 语义推断把包归错了类。\n\
\n\
<!-- fleet:model-guidance:begin -->\n\
@/home/x/.claude/fleet-model-guidance.md\n\
<!-- fleet:model-guidance:end -->\n\
@/home/x/.claude/fleet-interaction-mode.md\n";
        let (remaining, lessons) = extract_raw_lessons(claude);
        assert_eq!(lessons.len(), 2);
        assert_eq!(lessons[0].workspace_name, "claude-fleet");
        assert_eq!(lessons[0].session_id, "2778efcb");
        assert_eq!(lessons[0].content, "恢复前必须先验证。");
        assert_eq!(lessons[0].reason, "直接回滚会重现原问题。");
        assert_eq!(lessons[1].workspace_name, "netferry");
        assert_eq!(lessons[1].session_id, "2026-04-07");
        // Raw blocks gone, sentinel + imports preserved.
        assert!(!remaining.contains("# Lesson (from"));
        assert!(remaining.contains("Some global rules."));
        assert!(remaining.contains("<!-- fleet:model-guidance:begin -->"));
        assert!(remaining.contains("@/home/x/.claude/fleet-interaction-mode.md"));
        // Idempotent: re-running finds nothing.
        let (_, again) = extract_raw_lessons(&remaining);
        assert!(again.is_empty());
    }

    #[test]
    fn migrate_stops_at_prose_after_why_paragraph() {
        // Real-world layout: the last raw lesson block is followed by ordinary
        // hand-written rules (no sentinel, no `@`, no next lesson header). The
        // block must end after its **Why:** (+ optional **How to apply:**)
        // paragraph — the trailing prose belongs to the user, not the lesson.
        let claude = "\
# Lesson (from netferry, session 2026-04-07)\n\
subagent 结论要交叉验证。\n\
\n\
**Why:** 语义推断把包归错了类。\n\
\n\
**How to apply:** 关键结论自己 grep 确认。\n\
\n\
Before writing any new function, first search the codebase.\n\
\n\
**Why:** 重复造轮子是常见毛病。\n\
\n\
**How to apply:** 动手前跑一次 grep。\n";
        let (remaining, lessons) = extract_raw_lessons(claude);
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].content, "subagent 结论要交叉验证。");
        assert_eq!(
            lessons[0].reason,
            "语义推断把包归错了类。\n\n**How to apply:** 关键结论自己 grep 确认。"
        );
        // The user's hand-written rule (and its own Why/How lines) must survive.
        assert!(remaining.contains("Before writing any new function"));
        assert!(remaining.contains("重复造轮子是常见毛病"));
        assert!(remaining.contains("动手前跑一次 grep"));
        // …and must NOT have been swallowed into the lesson.
        assert!(!lessons[0].reason.contains("Before writing any new function"));
    }

    #[test]
    fn migrate_ignores_lesson_headers_inside_sentinels() {
        // A `# Lesson (…)` line inside a sentinel block (e.g. already-migrated
        // content that somehow lands in CLAUDE.md) must NOT be extracted.
        let claude = "\
<!-- fleet:lesson:deadbeef:begin -->\n\
# Lesson (from ws, session sid)\n\
content\n\
\n\
**Why:** why\n\
<!-- fleet:lesson:deadbeef:end -->\n";
        let (remaining, lessons) = extract_raw_lessons(claude);
        assert!(lessons.is_empty());
        assert_eq!(remaining, claude);
    }
}
