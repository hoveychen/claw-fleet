//! TASKS.md PRD-plan parsing, shared by the `fleet prd-context` hook and the
//! desktop Backend (session task-progress display).
//!
//! The sentinel format is `<!-- fleet:prd:begin id="X" -->` … `<!-- fleet:prd:end
//! id="X" -->`. Parsing is deliberately silent-degrading: malformed input never
//! panics and never blocks the caller. For the hook we additionally surface
//! detected problems (see [`SentinelProblem`]) so the agent can repair a broken
//! file rather than have its plan silently vanish from the injection.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ── Block types ───────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub struct PrdBlock {
    pub id: Option<String>,
    pub body: String,
    /// Schema version from the begin sentinel's `v="..."` attribute: `2` for
    /// v2, `1` for legacy blocks with no `v` attribute.
    pub version: u8,
}

/// Attributes parsed off a sentinel comment (`<!-- fleet:prd:begin id="x" v="2" -->`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentinelAttrs {
    pub id: Option<String>,
    pub version: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedBlock {
    pub id: Option<String>,
    pub body: String,
    pub source: PathBuf,
    pub mtime: SystemTime,
}

// ── Display / Backend types ───────────────────────────────────────────────────

/// Compact per-session task progress shown on the session card. Aggregates
/// every *active* plan the agent currently sees (main checkout + sibling
/// worktrees), mirroring the hook injection so the card matches reality.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlanSummary {
    pub plan_count: u32,
    pub done: u32,
    pub total: u32,
    /// Display name of the "currently focused" plan — the most recently modified
    /// active block (its `**Plan:**` title, falling back to its sentinel id).
    /// `None` for a single legacy anonymous block with no title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_plan: Option<String>,
    /// First still-pending top-level task in the focused plan (e.g. `**P3** — …`).
    /// `None` when the focused plan has no pending task (shouldn't happen for an
    /// active plan, but kept optional for safety).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
}

/// One checkbox line in a plan, for the full task-list view.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub text: String,
    pub done: bool,
}

/// A full plan with all of its task items, for the detail panel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlanDetail {
    pub id: Option<String>,
    /// `None` when the plan lives in the main checkout's TASKS.md; otherwise a
    /// relative (or absolute fallback) path to the worktree source file.
    pub source: Option<String>,
    pub items: Vec<TaskItem>,
}

/// A structural problem detected while parsing a TASKS.md. Surfaced to the agent
/// as a warning by the hook — we never auto-rewrite the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentinelProblem {
    /// A `begin` sentinel with no matching `end` (dropped block).
    UnterminatedBegin { id: Option<String> },
    /// An `end` sentinel with no open `begin`.
    UnmatchedEnd { id: Option<String> },
    /// `begin` and `end` ids disagree (block discarded).
    IdMismatch {
        begin_id: Option<String>,
        end_id: Option<String>,
    },
    /// A line that looks like a sentinel but failed to parse (bad quotes, no
    /// `-->`, empty id, …).
    MalformedSentinelLine { line: String },
}

impl SentinelProblem {
    /// Human-readable, single-line description for the warning reminder.
    pub fn describe(&self) -> String {
        let fmt_id = |id: &Option<String>| {
            id.as_deref()
                .map(|s| format!("id=\"{s}\""))
                .unwrap_or_else(|| "(anonymous)".to_string())
        };
        match self {
            SentinelProblem::UnterminatedBegin { id } => {
                format!("`begin {}` 没有匹配的 `end`(块未闭合,已被丢弃)", fmt_id(id))
            }
            SentinelProblem::UnmatchedEnd { id } => {
                format!("`end {}` 没有对应的 `begin`", fmt_id(id))
            }
            SentinelProblem::IdMismatch { begin_id, end_id } => format!(
                "`begin {}` 与 `end {}` 的 id 不一致(块被丢弃)",
                fmt_id(begin_id),
                fmt_id(end_id)
            ),
            SentinelProblem::MalformedSentinelLine { line } => {
                format!("无法解析的 sentinel 行:`{line}`")
            }
        }
    }
}

// ── Git root / source discovery ───────────────────────────────────────────────

/// Walk up from `start` to locate the main checkout root.
///
/// `.git` as a directory → its parent is the main checkout root.
/// `.git` as a file (`gitdir: ...`) → we're in a linked worktree; follow the
/// pointer to the worktree's git dir, read `commondir` to find the common
/// `.git` directory, and return that directory's parent.
/// Returns `None` if `start` is not inside any git repository.
pub fn discover_main_checkout_root(start: &Path) -> Option<PathBuf> {
    let mut p = start.canonicalize().ok()?;
    loop {
        let git = p.join(".git");
        if let Ok(meta) = fs::metadata(&git) {
            if meta.is_dir() {
                return Some(p);
            }
            if meta.is_file() {
                let content = fs::read_to_string(&git).ok()?;
                let gitdir = content
                    .lines()
                    .find_map(|l| l.strip_prefix("gitdir:").map(|s| s.trim()))?;
                let worktree_gitdir = PathBuf::from(gitdir);
                let commondir_path = worktree_gitdir.join("commondir");
                let commondir = fs::read_to_string(&commondir_path).ok()?;
                let common_abs = worktree_gitdir.join(commondir.trim());
                let common = common_abs.canonicalize().ok()?;
                return common.parent().map(|x| x.to_path_buf());
            }
        }
        p = p.parent()?.to_path_buf();
    }
}

/// Enumerate candidate TASKS.md files. In a git repo: main checkout's
/// TASKS.md first, then each `<main>/.worktrees/*/TASKS.md` that exists,
/// sorted alphabetically by path so output ordering is stable across runs.
/// Outside any repo (no main_root): just `<cwd>/TASKS.md` if it exists.
pub fn collect_task_sources(cwd: &Path, main_root: Option<&Path>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    match main_root {
        Some(root) => {
            let main_tasks = root.join("TASKS.md");
            if main_tasks.exists() {
                paths.push(main_tasks);
            }
            let worktrees_dir = root.join(".worktrees");
            if let Ok(entries) = fs::read_dir(&worktrees_dir) {
                let mut wt: Vec<PathBuf> = entries
                    .flatten()
                    .filter_map(|e| {
                        let p = e.path().join("TASKS.md");
                        if p.exists() {
                            Some(p)
                        } else {
                            None
                        }
                    })
                    .collect();
                wt.sort();
                paths.extend(wt);
            }
        }
        None => {
            let p = cwd.join("TASKS.md");
            if p.exists() {
                paths.push(p);
            }
        }
    }
    paths
}

// ── Block extraction ──────────────────────────────────────────────────────────

/// Scan a TASKS.md body for every active-plan region. A block opens on a
/// line containing `<!-- fleet:prd:begin ... -->` and closes on a matching
/// `<!-- fleet:prd:end ... -->` whose `id` agrees with the opener (both
/// missing counts as agreement, preserving the legacy unmarked form).
/// Mismatched or unterminated blocks are dropped silently — the hook
/// re-runs on every prompt, so a partially-edited file self-heals.
pub fn extract_prd_blocks(content: &str) -> Vec<PrdBlock> {
    extract_prd_blocks_with_problems(content).0
}

/// Like [`extract_prd_blocks`] but also returns every structural problem
/// detected. The returned `Vec<PrdBlock>` is byte-for-byte identical to what
/// [`extract_prd_blocks`] yields — problem detection is additive and never
/// changes which blocks are kept.
pub fn extract_prd_blocks_with_problems(content: &str) -> (Vec<PrdBlock>, Vec<SentinelProblem>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    // (id, version, body)
    let mut current: Option<(Option<String>, u8, String)> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(attrs) = parse_sentinel(trimmed, "begin") {
            // A second `begin` while one is already open means the previous
            // block was never closed — drop it and start fresh.
            if let Some((open_id, _, _)) = current.take() {
                problems.push(SentinelProblem::UnterminatedBegin { id: open_id });
            }
            current = Some((attrs.id, attrs.version, String::new()));
            continue;
        }
        if let Some(attrs) = parse_sentinel(trimmed, "end") {
            let id = attrs.id;
            match current.take() {
                Some((open_id, version, body)) => {
                    if open_id == id {
                        out.push(PrdBlock { id: open_id, body, version });
                    } else {
                        // id mismatch → discard the dangling block; do not start
                        // a new one from the `end` line.
                        problems.push(SentinelProblem::IdMismatch {
                            begin_id: open_id,
                            end_id: id,
                        });
                    }
                }
                None => {
                    problems.push(SentinelProblem::UnmatchedEnd { id });
                }
            }
            continue;
        }
        // A line that looks like a sentinel but parsed as neither begin nor end
        // is malformed (bad quotes, missing `-->`, empty id, …).
        if looks_like_sentinel(trimmed) {
            problems.push(SentinelProblem::MalformedSentinelLine {
                line: trimmed.to_string(),
            });
        }
        if let Some((_, _, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some((open_id, _, _)) = current.take() {
        problems.push(SentinelProblem::UnterminatedBegin { id: open_id });
    }
    (out, problems)
}

/// True when a line begins with a PRD sentinel prefix. Only consulted after
/// both `parse_sentinel_line(.., "begin")` and `(.., "end")` returned `None`,
/// so a match here means a genuinely malformed sentinel.
fn looks_like_sentinel(trimmed: &str) -> bool {
    trimmed.starts_with("<!-- fleet:prd:begin") || trimmed.starts_with("<!-- fleet:prd:end")
}

/// Back-compat wrapper: parse a sentinel and return just its `id` slot.
/// `Some(None)` = legacy anonymous, `Some(Some(id))` = id present, `None` =
/// not a sentinel of this kind / malformed / empty id.
pub fn parse_sentinel_line(line: &str, kind: &str) -> Option<Option<String>> {
    parse_sentinel(line, kind).map(|a| a.id)
}

/// Parse a sentinel comment line into its attributes. `kind` is "begin" or
/// "end". Handles both v1 (`id="x"` only, or bare anonymous) and v2
/// (`id="x" v="2"`, attributes in any order). Unknown attributes are tolerated
/// (ignored) so future versions stay forward-compatible. Returns `None` when
/// the line isn't a sentinel of this kind, is malformed (bad quotes, no
/// `-->`), or carries an empty `id`.
pub fn parse_sentinel(line: &str, kind: &str) -> Option<SentinelAttrs> {
    let prefix = format!("<!-- fleet:prd:{kind}");
    let rest = line.strip_prefix(&prefix)?;
    let inner = rest.strip_suffix("-->")?.trim();
    let attrs = parse_attr_map(inner)?;
    let id = attrs.iter().find(|(k, _)| k == "id").map(|(_, v)| v.clone());
    if let Some(idv) = &id {
        if idv.is_empty() {
            return None;
        }
    }
    let version = match attrs.iter().find(|(k, _)| k == "v").map(|(_, v)| v.as_str()) {
        Some("2") => 2,
        _ => 1,
    };
    Some(SentinelAttrs { id, version })
}

/// Parse the inner text of a sentinel into ordered `key="value"` pairs.
/// Empty inner → `Some(vec![])` (anonymous sentinel). Returns `None` if the
/// text isn't cleanly a sequence of `key="value"` attributes (e.g. an
/// unterminated quote or a bare token), which the caller treats as malformed.
fn parse_attr_map(inner: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut s = inner.trim_start();
    while !s.is_empty() {
        let eq = s.find('=')?;
        let key = s[..eq].trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        let after = s[eq + 1..].trim_start();
        let after = after.strip_prefix('"')?;
        let close = after.find('"')?;
        out.push((key.to_string(), after[..close].to_string()));
        s = after[close + 1..].trim_start();
    }
    Some(out)
}

/// Migrate a TASKS.md body from v1 to v2: add `v="2"` to every `begin`
/// sentinel that has an `id` but no version yet. Pure and idempotent — running
/// it twice is a no-op. Anonymous (id-less) legacy blocks are left untouched
/// (v2 requires a stable id; the parser still reads them as v1). All other
/// content is preserved byte-for-byte.
pub fn migrate_v1_to_v2(content: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        match parse_sentinel(trimmed, "begin") {
            Some(attrs) if attrs.version == 1 && attrs.id.is_some() => {
                if let Some(idx) = line.rfind("-->") {
                    let before = line[..idx].trim_end();
                    lines.push(format!("{before} v=\"2\" -->"));
                    continue;
                }
                lines.push(line.to_string());
            }
            _ => lines.push(line.to_string()),
        }
    }
    let mut out = lines.join("\n");
    // Preserve a trailing newline if the original had one.
    if content.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ── Mutation helpers (used by the `fleet plan` subcommands) ───────────────────

/// Re-attach a trailing newline iff the source had one — keeps mutations from
/// silently adding/stripping the final newline.
fn join_preserving_eol(lines: Vec<String>, original: &str) -> String {
    let mut s = lines.join("\n");
    if original.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Flip a top-level checkbox in-place: line `"- [ ]…"`/`"- [x]…"` → the
/// requested state, preserving everything after the 5-char marker.
fn set_line_checkbox(line: &str, done: bool) -> String {
    let marker = if done { "- [x]" } else { "- [ ]" };
    format!("{marker}{}", &line[5..])
}

/// Return the body of the plan block with `plan_id`, if present.
pub fn plan_body(content: &str, plan_id: &str) -> Option<String> {
    extract_prd_blocks(content)
        .into_iter()
        .find(|b| b.id.as_deref() == Some(plan_id))
        .map(|b| b.body)
}

/// Set the `[ ]`/`[x]` state of the top-level task matching `p_label` (matched
/// as the bold token `**<label>**`) inside plan `plan_id`. Errors when the plan
/// or the task isn't found.
pub fn set_checkbox(
    content: &str,
    plan_id: &str,
    p_label: &str,
    done: bool,
) -> Result<String, String> {
    let needle = format!("**{p_label}**");
    let mut in_target = false;
    let mut seen_plan = false;
    let mut found = false;
    let mut out: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(attrs) = parse_sentinel(trimmed, "begin") {
            in_target = attrs.id.as_deref() == Some(plan_id);
            if in_target {
                seen_plan = true;
            }
            out.push(line.to_string());
            continue;
        }
        if parse_sentinel(trimmed, "end").is_some() {
            in_target = false;
            out.push(line.to_string());
            continue;
        }
        if in_target && !found && top_level_checkbox(line).is_some() && line.contains(&needle) {
            out.push(set_line_checkbox(line, done));
            found = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !seen_plan {
        return Err(format!("plan '{plan_id}' not found"));
    }
    if !found {
        return Err(format!("task '{p_label}' not found in plan '{plan_id}'"));
    }
    Ok(join_preserving_eol(out, content))
}

/// Append a new pending task `- [ ] **<p_label>** — <text>` just before the
/// end sentinel of `plan_id`. Errors when the plan isn't found.
pub fn add_task(
    content: &str,
    plan_id: &str,
    p_label: &str,
    text: &str,
) -> Result<String, String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut seen_plan = false;
    let mut inserted = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(attrs) = parse_sentinel(trimmed, "begin") {
            in_target = attrs.id.as_deref() == Some(plan_id);
            if in_target {
                seen_plan = true;
            }
            out.push(line.to_string());
            continue;
        }
        if parse_sentinel(trimmed, "end").is_some() {
            if in_target && !inserted {
                out.push(format!("- [ ] **{p_label}** — {text}"));
                inserted = true;
            }
            in_target = false;
            out.push(line.to_string());
            continue;
        }
        out.push(line.to_string());
    }
    if !seen_plan {
        return Err(format!("plan '{plan_id}' not found"));
    }
    Ok(join_preserving_eol(out, content))
}

/// Append a fresh empty v2 plan block. Errors when `plan_id` already exists.
pub fn create_plan(content: &str, plan_id: &str, title: &str) -> Result<String, String> {
    if extract_prd_blocks(content)
        .iter()
        .any(|b| b.id.as_deref() == Some(plan_id))
    {
        return Err(format!("plan '{plan_id}' already exists"));
    }
    let block = format!(
        "<!-- fleet:prd:begin id=\"{plan_id}\" v=\"2\" -->\n\n**Plan:** {title}\n\n<!-- fleet:prd:end id=\"{plan_id}\" -->\n"
    );
    let mut out = content.to_string();
    if out.is_empty() {
        out.push_str("# TASKS\n\n");
    } else {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&block);
    Ok(out)
}

// ── Active-plan filtering ──────────────────────────────────────────────────────

/// Drop completed plans, keep active ones whole. A plan is "complete" when
/// no top-level checkbox is still `[ ]`; in that case we return `None` so the
/// block falls out of the injection. Otherwise we return the body verbatim
/// (trimmed), because some plans use the block as a living PRD document.
pub fn distill_active_block(body: &str) -> Option<String> {
    let has_pending = body.lines().any(is_pending_task_line);
    if !has_pending {
        return None;
    }
    Some(body.trim().to_string())
}

/// True iff a line is a top-level pending P-task (`- [ ]`).
pub fn is_pending_task_line(line: &str) -> bool {
    top_level_checkbox(line) == Some(false)
}

/// True iff a line is a top-level completed P-task (`- [x]` / `- [X]`).
pub fn is_done_task_line(line: &str) -> bool {
    top_level_checkbox(line) == Some(true)
}

/// `Some(true)` for `- [x]`, `Some(false)` for `- [ ]`, `None` for anything
/// else (including indented sub-items). Top-level only: a checkbox line must
/// start at column 0 so nested detail bullets don't inflate the count.
fn top_level_checkbox(line: &str) -> Option<bool> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let rest = line.strip_prefix("- ")?;
    if rest.starts_with("[ ]") {
        Some(false)
    } else if rest.starts_with("[x]") || rest.starts_with("[X]") {
        Some(true)
    } else {
        None
    }
}

/// Count `(done, total)` top-level checkbox tasks in a plan body.
pub fn count_tasks(body: &str) -> (u32, u32) {
    let mut done = 0u32;
    let mut total = 0u32;
    for line in body.lines() {
        match top_level_checkbox(line) {
            Some(true) => {
                done += 1;
                total += 1;
            }
            Some(false) => total += 1,
            None => {}
        }
    }
    (done, total)
}

/// Parse the top-level checkbox lines of a plan body into [`TaskItem`]s,
/// preserving order.
pub fn parse_task_items(body: &str) -> Vec<TaskItem> {
    let mut items = Vec::new();
    for line in body.lines() {
        if let Some(done) = top_level_checkbox(line) {
            items.push(TaskItem {
                text: checkbox_text(line),
                done,
            });
        }
    }
    items
}

/// Extract a plan's display name from its `**Plan:** …` line, if present.
/// Trims the `**Plan:**` marker and surrounding whitespace.
pub fn extract_plan_name(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        // Accept `**Plan:** xxx` and the bare `Plan: xxx` fallback.
        let Some(rest) = t
            .strip_prefix("**Plan:**")
            .or_else(|| t.strip_prefix("Plan:"))
        else {
            continue;
        };
        let name = rest.trim().trim_end_matches("**").trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// First still-pending top-level task in a plan body (the `- [ ]` text), or
/// `None` when every top-level task is done.
pub fn first_pending_task(body: &str) -> Option<String> {
    body.lines()
        .find(|l| is_pending_task_line(l))
        .map(|l| checkbox_text(l))
}

/// Strip the `- [ ] ` / `- [x] ` prefix and return the trimmed task text.
fn checkbox_text(line: &str) -> String {
    let rest = line.trim_start().strip_prefix("- ").unwrap_or(line);
    let rest = rest
        .strip_prefix("[ ]")
        .or_else(|| rest.strip_prefix("[x]"))
        .or_else(|| rest.strip_prefix("[X]"))
        .unwrap_or(rest);
    rest.trim().to_string()
}

// ── Source loading / dedup ────────────────────────────────────────────────────

/// Read every source, extract its blocks, and collect any problems. When
/// `active_only` is true, fully-completed plans are dropped (the hook +
/// card-summary behaviour); when false, every plan is kept (the detail view).
/// Sources we can't read are silently skipped — the caller re-runs later.
pub fn collect_from_sources(
    sources: &[PathBuf],
    active_only: bool,
) -> (Vec<SourcedBlock>, Vec<(PathBuf, SentinelProblem)>) {
    let mut all: Vec<SourcedBlock> = Vec::new();
    let mut problems: Vec<(PathBuf, SentinelProblem)> = Vec::new();
    for path in sources {
        let Ok(body) = fs::read_to_string(path) else {
            continue;
        };
        let mtime = fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        let (blocks, probs) = extract_prd_blocks_with_problems(&body);
        for p in probs {
            problems.push((path.clone(), p));
        }
        for b in blocks {
            let body = if active_only {
                match distill_active_block(&b.body) {
                    Some(d) => d,
                    None => continue,
                }
            } else {
                b.body.trim().to_string()
            };
            all.push(SourcedBlock {
                id: b.id,
                body,
                source: path.clone(),
                mtime,
            });
        }
    }
    (all, problems)
}

/// For each source path, read its body and mtime, then extract every active
/// PRD block. Thin wrapper over [`collect_from_sources`] that discards
/// problems — kept for the hook's existing render path.
pub fn load_sourced_blocks(sources: &[PathBuf]) -> Vec<SourcedBlock> {
    collect_from_sources(sources, true).0
}

/// Dedup by `id`: when the same `Some(id)` appears in multiple sources, keep
/// the one whose source file mtime is most recent (ties resolve to first
/// seen). `None` ids (legacy anonymous blocks) are kept independently.
pub fn dedup_blocks_keep_latest_mtime(blocks: Vec<SourcedBlock>) -> Vec<SourcedBlock> {
    let mut chosen: HashMap<String, usize> = HashMap::new();
    for (i, b) in blocks.iter().enumerate() {
        if let Some(id) = &b.id {
            match chosen.get(id) {
                Some(&j) => {
                    if blocks[i].mtime > blocks[j].mtime {
                        chosen.insert(id.clone(), i);
                    }
                }
                None => {
                    chosen.insert(id.clone(), i);
                }
            }
        }
    }
    blocks
        .into_iter()
        .enumerate()
        .filter(|(i, b)| match &b.id {
            None => true,
            Some(id) => chosen.get(id) == Some(i),
        })
        .map(|(_, b)| b)
        .collect()
}

// ── Rendering (hook) ──────────────────────────────────────────────────────────

/// Render the deduped blocks for the hook injection.
pub fn render_with_sources(blocks: &[SourcedBlock], main_root: Option<&Path>) -> String {
    if blocks.len() == 1 && blocks[0].id.is_none() {
        return blocks[0].body.trim().to_string();
    }
    let primary: Option<PathBuf> = main_root.map(|r| r.join("TASKS.md"));
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n---\n\n");
        }
        let label = b.id.as_deref().unwrap_or("(anonymous)");
        let tag = display_source(&b.source, main_root, primary.as_deref());
        match tag {
            Some(s) => out.push_str(&format!("## Plan: {label} — source: {s}\n\n")),
            None => out.push_str(&format!("## Plan: {label}\n\n")),
        }
        out.push_str(b.body.trim());
    }
    out
}

/// `None` when `source` is the main TASKS.md (the implicit norm — no
/// annotation needed). Otherwise a relative path under `main_root` when
/// possible, else the absolute path as a fallback.
pub fn display_source(
    source: &Path,
    main_root: Option<&Path>,
    primary: Option<&Path>,
) -> Option<String> {
    if primary.map_or(false, |p| source == p) {
        return None;
    }
    // Normalize to forward slashes so the rendered context is platform-stable.
    let to_string = |p: &Path| p.display().to_string().replace('\\', "/");
    if let Some(root) = main_root {
        if let Ok(rel) = source.strip_prefix(root) {
            return Some(to_string(rel));
        }
    }
    Some(to_string(source))
}

/// Format detected sentinel problems into a warning block for the hook
/// injection. Returns `None` when there are no problems. We never auto-rewrite
/// TASKS.md — this nudges the agent to repair it so a broken block doesn't
/// silently vanish from the macro plan.
pub fn render_problem_warning(problems: &[(PathBuf, SentinelProblem)]) -> Option<String> {
    if problems.is_empty() {
        return None;
    }
    let mut out = String::from(
        "⚠️ 检测到 TASKS.md 可能损坏(受影响的块已被跳过,未自动改写文件)。\
请检查并修正以下 sentinel,否则相关计划不会被注入:",
    );
    for (path, prob) in problems {
        let p = path.display().to_string().replace('\\', "/");
        out.push_str(&format!("\n  - {p}: {}", prob.describe()));
    }
    Some(out)
}

// ── High-level workspace queries (Backend) ────────────────────────────────────

/// Aggregate active-plan progress for the workspace containing `cwd`. Scans
/// the same source set the hook injects (main checkout + sibling worktrees).
/// `None` when no TASKS.md exists or no plan is active.
pub fn summarize_workspace_tasks(cwd: &Path) -> Option<TaskPlanSummary> {
    let main_root = discover_main_checkout_root(cwd);
    let sources = collect_task_sources(cwd, main_root.as_deref());
    if sources.is_empty() {
        return None;
    }
    let (raw, _) = collect_from_sources(&sources, true);
    let deduped = dedup_blocks_keep_latest_mtime(raw);
    if deduped.is_empty() {
        return None;
    }
    let mut done = 0u32;
    let mut total = 0u32;
    for b in &deduped {
        let (d, t) = count_tasks(&b.body);
        done += d;
        total += t;
    }
    // "Currently focused" plan = the most recently modified active block. When
    // a session works one of several plans across worktrees, the file it just
    // ticked a checkbox in has the newest mtime. NB: plans sharing one TASKS.md
    // have identical mtime, so within a file this falls back to the first active
    // plan in scan order (reduce keeps the earlier block on an mtime tie).
    let focused = deduped
        .iter()
        .reduce(|a, b| if b.mtime > a.mtime { b } else { a });
    let (current_plan, current_task) = match focused {
        Some(b) => (
            extract_plan_name(&b.body).or_else(|| b.id.clone()),
            first_pending_task(&b.body),
        ),
        None => (None, None),
    };
    Some(TaskPlanSummary {
        plan_count: deduped.len() as u32,
        done,
        total,
        current_plan,
        current_task,
    })
}

/// List every plan (active *and* completed) with all of its task items, for the
/// detail view. Scans the same source set as the hook.
pub fn list_workspace_task_plans(cwd: &Path) -> Vec<TaskPlanDetail> {
    let main_root = discover_main_checkout_root(cwd);
    let sources = collect_task_sources(cwd, main_root.as_deref());
    if sources.is_empty() {
        return Vec::new();
    }
    let (raw, _) = collect_from_sources(&sources, false);
    let deduped = dedup_blocks_keep_latest_mtime(raw);
    let primary: Option<PathBuf> = main_root.as_ref().map(|r| r.join("TASKS.md"));
    deduped
        .into_iter()
        .map(|b| TaskPlanDetail {
            source: display_source(&b.source, main_root.as_deref(), primary.as_deref()),
            id: b.id,
            items: parse_task_items(&b.body),
        })
        .collect()
}

/// Among the workspace's TASKS.md sources (main + worktrees), the file that
/// contains plan `plan_id`. When several do, the most recently modified wins —
/// the same dedup rule the hook uses. `None` if no source has the plan.
pub fn find_plan_source(cwd: &Path, plan_id: &str) -> Option<PathBuf> {
    let main_root = discover_main_checkout_root(cwd);
    let sources = collect_task_sources(cwd, main_root.as_deref());
    let mut hits: Vec<(PathBuf, SystemTime)> = Vec::new();
    for p in sources {
        let Ok(body) = fs::read_to_string(&p) else {
            continue;
        };
        if extract_prd_blocks(&body)
            .iter()
            .any(|b| b.id.as_deref() == Some(plan_id))
        {
            let m = fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            hits.push((p, m));
        }
    }
    hits.into_iter().max_by_key(|(_, m)| *m).map(|(p, _)| p)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn extract_returns_empty_when_no_sentinels() {
        let blocks = extract_prd_blocks("# TASKS\n\nno markers here\n");
        assert!(blocks.is_empty());
    }

    #[test]
    fn extract_single_anonymous_block_legacy() {
        let body = "# TASKS\n\n<!-- fleet:prd:begin -->\nplan body\n<!-- fleet:prd:end -->\n";
        let blocks = extract_prd_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, None);
        assert!(blocks[0].body.contains("plan body"));
    }

    #[test]
    fn extract_multiple_id_tagged_blocks_coexist() {
        let body = "\
<!-- fleet:prd:begin id=\"alpha\" -->\n\
alpha body\n\
<!-- fleet:prd:end id=\"alpha\" -->\n\
\n\
filler outside any block\n\
\n\
<!-- fleet:prd:begin id=\"beta\" -->\n\
beta body\n\
<!-- fleet:prd:end id=\"beta\" -->\n";
        let blocks = extract_prd_blocks(body);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].id.as_deref(), Some("alpha"));
        assert!(blocks[0].body.contains("alpha body"));
        assert_eq!(blocks[1].id.as_deref(), Some("beta"));
        assert!(blocks[1].body.contains("beta body"));
    }

    #[test]
    fn extract_drops_block_with_mismatched_end_id() {
        let body = "\
<!-- fleet:prd:begin id=\"alpha\" -->\n\
alpha body\n\
<!-- fleet:prd:end id=\"beta\" -->\n";
        // alpha is opened but never properly closed → discarded.
        assert!(extract_prd_blocks(body).is_empty());
    }

    #[test]
    fn extract_drops_unterminated_block_when_new_one_starts() {
        let body = "\
<!-- fleet:prd:begin id=\"alpha\" -->\n\
alpha body\n\
<!-- fleet:prd:begin id=\"beta\" -->\n\
beta body\n\
<!-- fleet:prd:end id=\"beta\" -->\n";
        let blocks = extract_prd_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id.as_deref(), Some("beta"));
    }

    #[test]
    fn extract_ignores_content_outside_sentinels() {
        let body = "\
prelude that should be ignored\n\
<!-- fleet:prd:begin id=\"x\" -->\n\
inside\n\
<!-- fleet:prd:end id=\"x\" -->\n\
trailing notes outside\n";
        let blocks = extract_prd_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].body.contains("prelude"));
        assert!(!blocks[0].body.contains("trailing notes"));
        assert!(blocks[0].body.contains("inside"));
    }

    // ── Problem detection (P2) ──────────────────────────────────────────────

    #[test]
    fn problems_flag_unterminated_begin() {
        let body = "<!-- fleet:prd:begin id=\"x\" -->\n- [ ] P1\nno end here\n";
        let (blocks, problems) = extract_prd_blocks_with_problems(body);
        assert!(blocks.is_empty(), "unterminated block is dropped");
        assert_eq!(
            problems,
            vec![SentinelProblem::UnterminatedBegin {
                id: Some("x".to_string())
            }]
        );
    }

    #[test]
    fn problems_flag_id_mismatch() {
        let body = "<!-- fleet:prd:begin id=\"a\" -->\nbody\n<!-- fleet:prd:end id=\"b\" -->\n";
        let (blocks, problems) = extract_prd_blocks_with_problems(body);
        assert!(blocks.is_empty());
        assert_eq!(
            problems,
            vec![SentinelProblem::IdMismatch {
                begin_id: Some("a".to_string()),
                end_id: Some("b".to_string()),
            }]
        );
    }

    #[test]
    fn problems_flag_unmatched_end() {
        let body = "some text\n<!-- fleet:prd:end id=\"x\" -->\n";
        let (_, problems) = extract_prd_blocks_with_problems(body);
        assert_eq!(
            problems,
            vec![SentinelProblem::UnmatchedEnd {
                id: Some("x".to_string())
            }]
        );
    }

    #[test]
    fn problems_flag_unclosed_quote_as_malformed() {
        // Missing closing quote → parse_sentinel_line returns None → flagged.
        let body = "<!-- fleet:prd:begin id=\"foo -->\nbody\n";
        let (_, problems) = extract_prd_blocks_with_problems(body);
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, SentinelProblem::MalformedSentinelLine { .. })),
            "unclosed quote should be flagged: {problems:?}"
        );
    }

    #[test]
    fn problems_empty_for_clean_file() {
        let body = "<!-- fleet:prd:begin id=\"x\" -->\n- [ ] P1\n<!-- fleet:prd:end id=\"x\" -->\n";
        let (blocks, problems) = extract_prd_blocks_with_problems(body);
        assert_eq!(blocks.len(), 1);
        assert!(problems.is_empty());
    }

    // ── count_tasks / parse_task_items ──────────────────────────────────────

    #[test]
    fn count_tasks_counts_top_level_only() {
        // NB: written without `\`-continuation so the nested line keeps its
        // leading whitespace (continuation strips it, which would defeat the
        // top-level test).
        let body = "**Plan:** mixed\n- [x] **P1** — done\n  - [ ] nested should not count\n- [ ] **P2** — todo\n- [X] **P3** — done caps\n";
        assert_eq!(count_tasks(body), (2, 3));
    }

    #[test]
    fn parse_task_items_extracts_text_and_state() {
        let body = "- [x] **P1** — first\n  - sub detail\n- [ ] **P2** — second\n";
        let items = parse_task_items(body);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "**P1** — first");
        assert!(items[0].done);
        assert_eq!(items[1].text, "**P2** — second");
        assert!(!items[1].done);
    }

    // ── Sourced helpers ─────────────────────────────────────────────────────

    fn sb(id: Option<&str>, body: &str, source: &Path, mtime: SystemTime) -> SourcedBlock {
        SourcedBlock {
            id: id.map(|s| s.to_string()),
            body: body.to_string(),
            source: source.to_path_buf(),
            mtime,
        }
    }

    #[test]
    fn render_single_legacy_block_keeps_old_shape() {
        let main = PathBuf::from("/repo");
        let blocks = vec![sb(None, "the body\n", &main.join("TASKS.md"), UNIX_EPOCH)];
        let out = render_with_sources(&blocks, Some(&main));
        assert_eq!(out, "the body");
    }

    #[test]
    fn render_multiple_blocks_get_plan_headings() {
        let main = PathBuf::from("/repo");
        let blocks = vec![
            sb(Some("a"), "A body\n", &main.join("TASKS.md"), UNIX_EPOCH),
            sb(Some("b"), "B body\n", &main.join("TASKS.md"), UNIX_EPOCH),
        ];
        let out = render_with_sources(&blocks, Some(&main));
        assert!(out.contains("## Plan: a"));
        assert!(out.contains("## Plan: b"));
        assert!(out.contains("A body"));
        assert!(out.contains("B body"));
        assert!(out.contains("---"));
        assert!(!out.contains("source:"));
    }

    #[test]
    fn render_worktree_blocks_get_source_suffix() {
        let main = PathBuf::from("/repo");
        let wt_path = main.join(".worktrees").join("featX").join("TASKS.md");
        let blocks = vec![
            sb(Some("a"), "A body\n", &main.join("TASKS.md"), UNIX_EPOCH),
            sb(Some("z"), "Z body\n", &wt_path, UNIX_EPOCH),
        ];
        let out = render_with_sources(&blocks, Some(&main));
        assert!(out.contains("## Plan: a\n"));
        assert!(out.contains("## Plan: z — source: .worktrees/featX/TASKS.md"));
    }

    #[test]
    fn render_normalizes_backslashes_in_worktree_path() {
        let main = PathBuf::from("/repo");
        let wt_path = main.join(r".worktrees\featX\TASKS.md");
        let blocks = vec![sb(Some("z"), "Z body\n", &wt_path, UNIX_EPOCH)];
        let out = render_with_sources(&blocks, Some(&main));
        assert!(out.contains("source: .worktrees/featX/TASKS.md"));
        assert!(!out.contains('\\'));
    }

    #[test]
    fn dedup_picks_newer_mtime_for_same_id() {
        let main = PathBuf::from("/repo");
        let wt = main.join(".worktrees").join("featX").join("TASKS.md");
        let old = UNIX_EPOCH;
        let new = UNIX_EPOCH + Duration::from_secs(60);
        let blocks = vec![
            sb(Some("shared"), "main copy", &main.join("TASKS.md"), old),
            sb(Some("shared"), "worktree copy", &wt, new),
            sb(Some("solo"), "main solo", &main.join("TASKS.md"), old),
        ];
        let out = dedup_blocks_keep_latest_mtime(blocks);
        assert_eq!(out.len(), 2);
        let shared = out.iter().find(|b| b.id.as_deref() == Some("shared")).unwrap();
        assert_eq!(shared.body, "worktree copy");
        assert_eq!(shared.source, wt);
        assert!(out.iter().any(|b| b.id.as_deref() == Some("solo")));
    }

    #[test]
    fn dedup_keeps_anonymous_blocks_from_each_source() {
        let main = PathBuf::from("/repo");
        let wt = main.join(".worktrees").join("featX").join("TASKS.md");
        let t = UNIX_EPOCH;
        let blocks = vec![
            sb(None, "main anon", &main.join("TASKS.md"), t),
            sb(None, "wt anon", &wt, t),
        ];
        let out = dedup_blocks_keep_latest_mtime(blocks);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn dedup_preserves_first_seen_order() {
        let main = PathBuf::from("/repo");
        let t = UNIX_EPOCH;
        let blocks = vec![
            sb(Some("b"), "Bx", &main.join("TASKS.md"), t),
            sb(Some("a"), "Ax", &main.join("TASKS.md"), t),
            sb(Some("c"), "Cx", &main.join("TASKS.md"), t),
        ];
        let out = dedup_blocks_keep_latest_mtime(blocks);
        let ids: Vec<&str> = out.iter().filter_map(|b| b.id.as_deref()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    #[test]
    fn collect_task_sources_main_only_when_no_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path();
        std::fs::write(main.join("TASKS.md"), "main body").unwrap();
        let sources = collect_task_sources(main, Some(main));
        assert_eq!(sources, vec![main.join("TASKS.md")]);
    }

    #[test]
    fn collect_task_sources_includes_all_worktrees_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path();
        std::fs::write(main.join("TASKS.md"), "main body").unwrap();
        let wt_dir = main.join(".worktrees");
        std::fs::create_dir_all(wt_dir.join("beta")).unwrap();
        std::fs::create_dir_all(wt_dir.join("alpha")).unwrap();
        std::fs::create_dir_all(wt_dir.join("gamma")).unwrap();
        std::fs::write(wt_dir.join("alpha").join("TASKS.md"), "a").unwrap();
        std::fs::write(wt_dir.join("beta").join("TASKS.md"), "b").unwrap();
        let sources = collect_task_sources(main, Some(main));
        assert_eq!(
            sources,
            vec![
                main.join("TASKS.md"),
                wt_dir.join("alpha").join("TASKS.md"),
                wt_dir.join("beta").join("TASKS.md"),
            ]
        );
    }

    #[test]
    fn collect_task_sources_falls_back_to_cwd_outside_git() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("TASKS.md"), "lone").unwrap();
        let sources = collect_task_sources(tmp.path(), None);
        assert_eq!(sources, vec![tmp.path().join("TASKS.md")]);
    }

    #[test]
    fn discover_main_root_for_plain_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(main.join(".git")).unwrap();
        std::fs::create_dir_all(main.join("sub")).unwrap();
        assert_eq!(discover_main_checkout_root(&main.join("sub")), Some(main.clone()));
        assert_eq!(discover_main_checkout_root(&main), Some(main));
    }

    #[test]
    fn discover_main_root_from_worktree_follows_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().canonicalize().unwrap();
        let main_gitdir = main.join(".git");
        std::fs::create_dir_all(&main_gitdir).unwrap();
        let wt_path = main.join(".worktrees").join("feat");
        std::fs::create_dir_all(&wt_path).unwrap();
        let wt_gitdir = main_gitdir.join("worktrees").join("feat");
        std::fs::create_dir_all(&wt_gitdir).unwrap();
        std::fs::write(wt_path.join(".git"), format!("gitdir: {}", wt_gitdir.display())).unwrap();
        std::fs::write(wt_gitdir.join("commondir"), "../..").unwrap();
        let resolved = discover_main_checkout_root(&wt_path);
        assert_eq!(resolved, Some(main));
    }

    #[test]
    fn discover_main_root_none_when_not_in_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let result = discover_main_checkout_root(tmp.path());
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn load_sourced_blocks_reads_and_distills() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("TASKS.md");
        std::fs::write(
            &path,
            "<!-- fleet:prd:begin id=\"x\" -->\n\
- [ ] **P1** — pending\n\
<!-- fleet:prd:end id=\"x\" -->\n\
<!-- fleet:prd:begin id=\"done\" -->\n\
- [x] **P1** — all done\n\
<!-- fleet:prd:end id=\"done\" -->\n",
        )
        .unwrap();
        let out = load_sourced_blocks(&[path.clone()]);
        assert_eq!(out.len(), 1, "completed plan dropped, active kept");
        assert_eq!(out[0].id.as_deref(), Some("x"));
        assert_eq!(out[0].source, path);
    }

    #[test]
    fn parse_sentinel_handles_id_and_legacy() {
        assert_eq!(parse_sentinel_line("<!-- fleet:prd:begin -->", "begin"), Some(None));
        assert_eq!(
            parse_sentinel_line("<!-- fleet:prd:begin id=\"foo\" -->", "begin"),
            Some(Some("foo".to_string()))
        );
        assert_eq!(
            parse_sentinel_line("<!-- fleet:prd:end id=\"foo\" -->", "end"),
            Some(Some("foo".to_string()))
        );
        assert_eq!(parse_sentinel_line("<!-- fleet:prd:end -->", "begin"), None);
        assert_eq!(parse_sentinel_line("<!-- fleet:prd:begin id=\"\" -->", "begin"), None);
        assert_eq!(parse_sentinel_line("# TASKS", "begin"), None);
    }

    #[test]
    fn parse_sentinel_reads_version_attr() {
        // v1: no `v` attr → version 1.
        let v1 = parse_sentinel("<!-- fleet:prd:begin id=\"x\" -->", "begin").unwrap();
        assert_eq!(v1.id.as_deref(), Some("x"));
        assert_eq!(v1.version, 1);
        // v2: `v="2"` in either order.
        let v2 = parse_sentinel("<!-- fleet:prd:begin id=\"x\" v=\"2\" -->", "begin").unwrap();
        assert_eq!(v2.id.as_deref(), Some("x"));
        assert_eq!(v2.version, 2);
        let v2b = parse_sentinel("<!-- fleet:prd:begin v=\"2\" id=\"x\" -->", "begin").unwrap();
        assert_eq!(v2b.id.as_deref(), Some("x"));
        assert_eq!(v2b.version, 2);
        // anonymous legacy
        let anon = parse_sentinel("<!-- fleet:prd:begin -->", "begin").unwrap();
        assert_eq!(anon.id, None);
        assert_eq!(anon.version, 1);
        // unknown attrs tolerated (forward-compat)
        let fut = parse_sentinel("<!-- fleet:prd:begin id=\"x\" v=\"2\" foo=\"bar\" -->", "begin").unwrap();
        assert_eq!(fut.version, 2);
        // malformed → None
        assert!(parse_sentinel("<!-- fleet:prd:begin id=\"x -->", "begin").is_none());
        assert!(parse_sentinel("<!-- fleet:prd:begin garbage -->", "begin").is_none());
    }

    #[test]
    fn extract_records_block_version() {
        let body = "<!-- fleet:prd:begin id=\"a\" -->\n- [ ] P1\n<!-- fleet:prd:end id=\"a\" -->\n\
<!-- fleet:prd:begin id=\"b\" v=\"2\" -->\n- [ ] P1\n<!-- fleet:prd:end id=\"b\" -->\n";
        let blocks = extract_prd_blocks(body);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].version, 1);
        assert_eq!(blocks[1].version, 2);
    }

    #[test]
    fn migrate_adds_version_to_v1_id_blocks() {
        let v1 = "# TASKS\n\n<!-- fleet:prd:begin id=\"auth\" -->\n\
**Plan:** x\n- [ ] **P1** — a\n<!-- fleet:prd:end id=\"auth\" -->\n";
        let migrated = migrate_v1_to_v2(v1);
        assert!(migrated.contains("<!-- fleet:prd:begin id=\"auth\" v=\"2\" -->"));
        // end sentinel untouched, body untouched
        assert!(migrated.contains("<!-- fleet:prd:end id=\"auth\" -->"));
        assert!(migrated.contains("- [ ] **P1** — a"));
        // idempotent
        assert_eq!(migrate_v1_to_v2(&migrated), migrated);
    }

    #[test]
    fn migrate_leaves_anonymous_and_v2_untouched() {
        let anon = "<!-- fleet:prd:begin -->\n- [ ] P1\n<!-- fleet:prd:end -->\n";
        assert_eq!(migrate_v1_to_v2(anon), anon, "anonymous legacy block left as-is");
        let v2 = "<!-- fleet:prd:begin id=\"x\" v=\"2\" -->\n- [ ] P1\n<!-- fleet:prd:end id=\"x\" -->\n";
        assert_eq!(migrate_v1_to_v2(v2), v2, "already-v2 block unchanged");
    }

    #[test]
    fn migrate_preserves_indentation_and_no_trailing_newline() {
        let v1 = "  <!-- fleet:prd:begin id=\"x\" -->\n- [ ] P1\n  <!-- fleet:prd:end id=\"x\" -->";
        let migrated = migrate_v1_to_v2(v1);
        assert!(migrated.contains("  <!-- fleet:prd:begin id=\"x\" v=\"2\" -->"));
        assert!(!migrated.ends_with('\n'), "no trailing newline added when source had none");
    }

    // ── Mutation helpers ────────────────────────────────────────────────────

    const TWO_PLAN: &str = "<!-- fleet:prd:begin id=\"a\" v=\"2\" -->\n\
**Plan:** Alpha\n\
- [ ] **P1** — first\n\
- [ ] **P2** — second\n\
<!-- fleet:prd:end id=\"a\" -->\n\
<!-- fleet:prd:begin id=\"b\" v=\"2\" -->\n\
**Plan:** Beta\n\
- [ ] **P1** — beta first\n\
<!-- fleet:prd:end id=\"b\" -->\n";

    #[test]
    fn set_checkbox_ticks_only_matching_plan_and_task() {
        let out = set_checkbox(TWO_PLAN, "a", "P1", true).unwrap();
        assert!(out.contains("- [x] **P1** — first"));
        // P2 in same plan untouched; plan b's P1 untouched.
        assert!(out.contains("- [ ] **P2** — second"));
        assert!(out.contains("- [ ] **P1** — beta first"));
    }

    #[test]
    fn set_checkbox_unchecks() {
        let checked = set_checkbox(TWO_PLAN, "a", "P1", true).unwrap();
        let back = set_checkbox(&checked, "a", "P1", false).unwrap();
        assert_eq!(back, TWO_PLAN);
    }

    #[test]
    fn set_checkbox_errors_on_missing_plan_or_task() {
        assert!(set_checkbox(TWO_PLAN, "nope", "P1", true).is_err());
        assert!(set_checkbox(TWO_PLAN, "a", "P9", true).is_err());
    }

    #[test]
    fn add_task_appends_before_end() {
        let out = add_task(TWO_PLAN, "b", "P2", "beta second").unwrap();
        assert!(out.contains("- [ ] **P2** — beta second"));
        // inserted inside plan b, before its end sentinel
        let beta_idx = out.find("beta second").unwrap();
        let end_idx = out.find("<!-- fleet:prd:end id=\"b\" -->").unwrap();
        assert!(beta_idx < end_idx);
        assert!(add_task(TWO_PLAN, "missing", "P1", "x").is_err());
    }

    #[test]
    fn create_plan_appends_v2_block_and_rejects_dupes() {
        let out = create_plan(TWO_PLAN, "c", "Gamma").unwrap();
        assert!(out.contains("<!-- fleet:prd:begin id=\"c\" v=\"2\" -->"));
        assert!(out.contains("**Plan:** Gamma"));
        assert!(create_plan(&out, "c", "again").is_err());
        // create into empty content seeds a header
        let fresh = create_plan("", "x", "X").unwrap();
        assert!(fresh.starts_with("# TASKS"));
        assert!(fresh.contains("id=\"x\" v=\"2\""));
    }

    #[test]
    fn plan_body_returns_named_block() {
        assert!(plan_body(TWO_PLAN, "a").unwrap().contains("Alpha"));
        assert_eq!(plan_body(TWO_PLAN, "zzz"), None);
    }

    #[test]
    fn distill_excludes_fully_completed_plan() {
        let body = "**Plan:** all done\n\n- [x] **P1** — first\n  - detail a\n- [x] **P2** — second\n";
        assert_eq!(distill_active_block(body), None);
    }

    #[test]
    fn distill_keeps_plan_with_pending_task() {
        let body = "**Plan:** mixed\n\n- [x] **P1** — done\n- [ ] **P2** — todo\n  - actionable detail\n";
        let out = distill_active_block(body).expect("active plan kept");
        assert!(out.contains("**Plan:** mixed"));
        assert!(out.contains("- [ ] **P2** — todo"));
        assert!(out.contains("actionable detail"));
    }

    #[test]
    fn summarize_workspace_aggregates_active_plans() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path();
        std::fs::write(
            main.join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"x\" -->\n\
- [x] **P1** — done\n\
- [ ] **P2** — todo\n\
- [ ] **P3** — todo\n\
<!-- fleet:prd:end id=\"x\" -->\n",
        )
        .unwrap();
        let s = summarize_workspace_tasks(main).expect("summary");
        assert_eq!(s.plan_count, 1);
        assert_eq!(s.done, 1);
        assert_eq!(s.total, 3);
        // Single plan → it is the focused one; current task is the first pending.
        assert_eq!(s.current_task.as_deref(), Some("**P2** — todo"));
    }

    #[test]
    fn extract_plan_name_reads_plan_marker() {
        let body = "intro\n\n**Plan:** Migrate auth crate\n\n- [ ] P1\n";
        assert_eq!(extract_plan_name(body).as_deref(), Some("Migrate auth crate"));
        assert_eq!(extract_plan_name("no plan line\n- [ ] P1\n"), None);
    }

    #[test]
    fn first_pending_task_skips_done() {
        let body = "- [x] **P1** — done\n- [ ] **P2** — next\n- [ ] **P3** — later\n";
        assert_eq!(first_pending_task(body).as_deref(), Some("**P2** — next"));
        assert_eq!(first_pending_task("- [x] all done\n"), None);
    }

    #[test]
    fn summarize_picks_plan_name_as_current_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path();
        std::fs::write(
            main.join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"x\" -->\n\
**Plan:** 重构会话中间件\n\
- [x] **P1** — done\n\
- [ ] **P2** — 当前任务\n\
<!-- fleet:prd:end id=\"x\" -->\n",
        )
        .unwrap();
        let s = summarize_workspace_tasks(main).expect("summary");
        assert_eq!(s.current_plan.as_deref(), Some("重构会话中间件"));
        assert_eq!(s.current_task.as_deref(), Some("**P2** — 当前任务"));
    }

    #[test]
    fn summarize_focused_plan_is_newest_file_across_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path();
        std::fs::create_dir_all(main.join(".git")).unwrap();
        std::fs::write(
            main.join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"old\" -->\n\
**Plan:** 旧计划\n- [ ] **P1** — 旧任务\n\
<!-- fleet:prd:end id=\"old\" -->\n",
        )
        .unwrap();
        let wt = main.join(".worktrees").join("feat");
        std::fs::create_dir_all(&wt).unwrap();
        let wt_tasks = wt.join("TASKS.md");
        std::fs::write(
            &wt_tasks,
            "<!-- fleet:prd:begin id=\"new\" -->\n\
**Plan:** 新计划\n- [ ] **P1** — 新任务\n\
<!-- fleet:prd:end id=\"new\" -->\n",
        )
        .unwrap();
        // Make the worktree file strictly newer so it wins the focus.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&wt_tasks)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let s = summarize_workspace_tasks(main).expect("summary");
        assert_eq!(s.plan_count, 2);
        assert_eq!(s.current_plan.as_deref(), Some("新计划"));
        assert_eq!(s.current_task.as_deref(), Some("**P1** — 新任务"));
    }

    #[test]
    fn render_problem_warning_none_when_clean() {
        assert_eq!(render_problem_warning(&[]), None);
    }

    #[test]
    fn render_problem_warning_lists_each_problem_with_path() {
        let probs = vec![
            (
                PathBuf::from("/repo/TASKS.md"),
                SentinelProblem::UnterminatedBegin {
                    id: Some("alpha".to_string()),
                },
            ),
            (
                PathBuf::from("/repo/.worktrees/x/TASKS.md"),
                SentinelProblem::IdMismatch {
                    begin_id: Some("a".to_string()),
                    end_id: Some("b".to_string()),
                },
            ),
        ];
        let out = render_problem_warning(&probs).expect("warning");
        assert!(out.contains("TASKS.md 可能损坏"));
        assert!(out.contains("/repo/TASKS.md"));
        assert!(out.contains("没有匹配的 `end`"));
        assert!(out.contains("/repo/.worktrees/x/TASKS.md"));
        assert!(out.contains("id 不一致"));
    }

    #[test]
    fn list_workspace_task_plans_returns_items() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path();
        std::fs::write(
            main.join("TASKS.md"),
            "<!-- fleet:prd:begin id=\"x\" -->\n\
- [x] **P1** — done\n\
- [ ] **P2** — todo\n\
<!-- fleet:prd:end id=\"x\" -->\n",
        )
        .unwrap();
        let plans = list_workspace_task_plans(main);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].id.as_deref(), Some("x"));
        assert_eq!(plans[0].items.len(), 2);
        assert!(plans[0].items[0].done);
        assert!(!plans[0].items[1].done);
    }
}
