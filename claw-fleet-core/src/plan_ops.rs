//! Plan mutations shared by the `fleet plan` CLI and the `fleet__plan` MCP tool.
//!
//! The pure string transforms live in [`crate::prd_tasks`]; this layer adds the
//! read-mutate-write against TASKS.md on disk plus the session focus-attribution
//! side-channel ([`crate::task_progress`]). It was lifted verbatim out of the
//! `fleet plan` CLI so the MCP handler executes exactly the same logic — crucial
//! for rca remote-workspace sessions, where a `Bash` `fleet plan` is routed to a
//! remote executor that has no `fleet`, but an MCP tool call reaches the local
//! server that owns the local TASKS.md.
//!
//! Both callers resolve a session id from the environment and pass it in, so
//! this module never reads env itself. A missing session id degrades focus
//! attribution to a warning (the file edit still applies) for `check` / `create`
//! — matching the CLI's long-standing best-effort contract — but is a hard error
//! for `resume`, whose entire purpose is to claim focus.

use crate::prd_tasks as pt;
use std::path::{Path, PathBuf};

/// Result of a plan-mutating operation: the primary line to show the user, plus
/// any non-fatal focus-attribution warnings. The CLI prints warnings to stderr
/// (prefixed `warning: `) then the message to stdout; the MCP handler folds both
/// into its returned text.
#[derive(Clone, Debug, Default)]
pub struct PlanOutcome {
    pub message: String,
    pub warnings: Vec<String>,
}

impl PlanOutcome {
    fn ok() -> Self {
        Self {
            message: "ok".to_string(),
            warnings: Vec::new(),
        }
    }
}

/// Workspace's primary TASKS.md (main checkout root, else cwd).
pub fn workspace_tasks_path(cwd: &Path) -> PathBuf {
    match pt::discover_main_checkout_root(cwd) {
        Some(root) => root.join("TASKS.md"),
        None => cwd.join("TASKS.md"),
    }
}

/// Record `session_id`'s focus (plan + current pending P) in the side-channel.
/// Returns warnings rather than failing: a missing session id or a
/// `task_progress` write error both degrade to a warning, matching the CLI's
/// "the edit still applies, attribution is best-effort" contract.
fn record_focus(
    cwd: &Path,
    plan_id: &str,
    content: &str,
    session_id: Option<&str>,
) -> Vec<String> {
    let Some(sid) = session_id else {
        return vec![format!(
            "no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set); \
             plan '{plan_id}' was updated but this session won't be attributed to it."
        )];
    };
    let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let current = pt::plan_body(content, plan_id).and_then(|b| pt::first_pending_task(&b));
    if let Err(e) = crate::task_progress::set_current(sid, &ws.to_string_lossy(), plan_id, current) {
        return vec![format!("could not record task focus: {e}")];
    }
    Vec::new()
}

/// Tick or untick a task's checkbox, then record focus. When ticking the last
/// box of a child plan, backtrack focus to the nearest pending ancestor and
/// return that directive as the message.
pub fn mutate_checkbox(
    cwd: &Path,
    plan_id: &str,
    task: &str,
    done: bool,
    session_id: Option<&str>,
) -> Result<PlanOutcome, String> {
    let path = pt::find_plan_source(cwd, plan_id)
        .ok_or_else(|| format!("plan '{plan_id}' not found in any TASKS.md"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = pt::set_checkbox(&content, plan_id, task, done)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    let mut warnings = record_focus(cwd, plan_id, &updated, session_id);
    if done {
        if let Some((msg, mut w)) = backtrack_on_completion(cwd, plan_id, &updated, session_id) {
            warnings.append(&mut w);
            return Ok(PlanOutcome { message: msg, warnings });
        }
    }
    Ok(PlanOutcome {
        message: "ok".to_string(),
        warnings,
    })
}

/// When `plan_id` just became fully complete (no pending top-level task left)
/// AND it is a child plan whose nearest pending ancestor still has work,
/// re-attribute `session_id`'s focus to that ancestor and return a directive
/// telling the agent to keep going there. Returns `None` when the plan still has
/// pending tasks or there is nowhere to backtrack to.
fn backtrack_on_completion(
    cwd: &Path,
    plan_id: &str,
    content: &str,
    session_id: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let body = pt::plan_body(content, plan_id)?;
    if body.lines().any(pt::is_pending_task_line) {
        return None; // not fully complete yet
    }
    let target = pt::resolve_backtrack_target(cwd, plan_id)?;
    // Re-point focus at the ancestor so the desktop card follows immediately,
    // without waiting for the agent to run `fleet plan resume` itself.
    let mut warnings = Vec::new();
    if let Some(sid) = session_id {
        let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        if let Err(e) = crate::task_progress::set_current(
            sid,
            &ws.to_string_lossy(),
            &target.plan_id,
            target.next_task.clone(),
        ) {
            warnings.push(format!("could not re-attribute focus to parent plan: {e}"));
        }
    }
    let next = target.next_task.as_deref().unwrap_or("第一个未完成的 P");
    Some((
        format!(
            "ok — 子 plan '{plan_id}' 已全部完成。其父 plan '{parent}' 尚有未完成任务,\
             Fleet 已把你的焦点切回 '{parent}'。请从 {next} 继续执行,不要结束 turn。",
            parent = target.plan_id,
        ),
        warnings,
    ))
}

/// Resolve which P `session_id` should now be focused on and record it. Unlike
/// `check`/`create`, a missing session id is a hard error — `resume`'s whole
/// job is to claim focus, so there is nothing to do without an id.
pub fn resume(
    cwd: &Path,
    plan_id: &str,
    task: Option<&str>,
    session_id: Option<&str>,
) -> Result<PlanOutcome, String> {
    let current = pt::resolve_current_task(cwd, plan_id, task)?;
    let sid = session_id
        .ok_or("no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set)")?;
    let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    crate::task_progress::set_current(sid, &ws.to_string_lossy(), plan_id, current)?;
    Ok(PlanOutcome::ok())
}

/// Create a new plan block in the workspace's primary TASKS.md and claim focus
/// (authoring a plan is starting it). `parent` records a sub-plan relationship.
///
/// Exactly one of `parent` / `root` must be given. Making the tree position an
/// explicit choice is the point: with `--parent` merely *available*, 350 of the
/// first 355 plans authored in this repo were created flat, so the parent-link
/// backtrack had no tree to walk and a decomposed task never ran to completion
/// as a unit. Declaring `--root` is cheap; forgetting to declare anything is
/// what used to happen silently.
pub fn create(
    cwd: &Path,
    plan_id: &str,
    title: &str,
    parent: Option<&str>,
    root: bool,
    kind: pt::PlanKind,
    session_id: Option<&str>,
) -> Result<PlanOutcome, String> {
    let parent = parent.filter(|p| !p.trim().is_empty());
    match (parent, root) {
        (Some(_), true) => {
            return Err(
                "--parent and --root are mutually exclusive: a plan is either a child of \
                 another plan or a new tree root, not both."
                    .to_string(),
            )
        }
        (None, false) => {
            let candidates = pending_plan_ids(cwd);
            let hint = if candidates.is_empty() {
                "no plan here has pending work, so --root is probably right.".to_string()
            } else {
                format!(
                    "plans with pending work you could be branching off: {}.",
                    candidates.join(", ")
                )
            };
            return Err(format!(
                "declare this plan's position in the tree: pass --parent <id> if it is side \
                 work spawned mid-plan (Fleet will point you back at the parent when it \
                 completes), or --root if it starts a new top-level tree. {hint}"
            ))
        }
        _ => {}
    }
    let path = workspace_tasks_path(cwd);
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = pt::create_plan(&content, plan_id, title, parent, kind)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    // Writing a plan is starting it: the agent authoring the block is the agent
    // about to execute it. Claiming focus here spares it a separate `resume`.
    let warnings = record_focus(cwd, plan_id, &updated, session_id);
    let kindness = match kind {
        pt::PlanKind::Explore => " [explore — 交付物是 exec 子 plan,不是代码改动]",
        pt::PlanKind::Exec => "",
    };
    let message = match parent {
        Some(p) if !p.trim().is_empty() => format!(
            "created child plan '{plan_id}'{kindness} (parent '{}') in {}",
            p.trim(),
            path.display()
        ),
        _ => format!("created plan '{plan_id}'{kindness} in {}", path.display()),
    };
    Ok(PlanOutcome { message, warnings })
}

/// Ids of plans in this workspace that still have a pending P-task — the
/// plausible `--parent` candidates offered when a `create` declares neither
/// position. Best-effort: an unreadable TASKS.md yields an empty list rather
/// than failing the create's error message.
fn pending_plan_ids(cwd: &Path) -> Vec<String> {
    let main_root = pt::discover_main_checkout_root(cwd);
    let sources = pt::collect_task_sources(cwd, main_root.as_deref());
    let (raw, _) = pt::collect_from_sources(&sources, true);
    let mut ids: Vec<String> = pt::dedup_blocks_keep_latest_mtime(raw)
        .into_iter()
        .filter_map(|b| b.id)
        .collect();
    ids.sort();
    ids.truncate(12); // an error message, not a listing — `fleet plan list` has the rest
    ids
}

/// Append a pending task. Deliberately does NOT claim focus: `add` edits a
/// plan's structure and says nothing about who is executing it — a master
/// appending a P-item to another session's plan must not repoint its own card.
pub fn add(cwd: &Path, plan_id: &str, task: &str, text: &str) -> Result<PlanOutcome, String> {
    let path = pt::find_plan_source(cwd, plan_id)
        .ok_or_else(|| format!("plan '{plan_id}' not found in any TASKS.md"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = pt::add_task(&content, plan_id, task, text)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(PlanOutcome::ok())
}

/// Upgrade a TASKS.md from the v1 (unmarked) to the v2 (`id="..."`) sentinel
/// format. Idempotent: a no-op when already v2.
pub fn migrate(cwd: &Path, path: Option<PathBuf>) -> Result<PlanOutcome, String> {
    let target = path.unwrap_or_else(|| workspace_tasks_path(cwd));
    let content =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let migrated = pt::migrate_v1_to_v2(&content);
    if migrated == content {
        return Ok(PlanOutcome {
            message: format!("already v2 (no changes): {}", target.display()),
            warnings: Vec::new(),
        });
    }
    std::fs::write(&target, &migrated).map_err(|e| format!("write {}: {e}", target.display()))?;
    Ok(PlanOutcome {
        message: format!("migrated {}", target.display()),
        warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the workspace's TASKS.md body.
    fn read_tasks(cwd: &Path) -> String {
        std::fs::read_to_string(workspace_tasks_path(cwd)).unwrap_or_default()
    }

    /// `create` writes a v2 sentinel block and `mutate_checkbox` flips its box on
    /// disk — the extracted read-mutate-write path the MCP tool will reuse. Passing
    /// `session_id = None` keeps focus attribution a warning (no `~/.fleet` write),
    /// so the test is hermetic to the tempdir.
    #[test]
    fn create_then_check_mutates_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        let created = create(cwd, "demo", "Demo work", None, true, pt::PlanKind::Exec, None).unwrap();
        assert!(created.message.contains("created plan 'demo'"));
        // No session id ⇒ attribution is a warning, but the edit still lands.
        assert_eq!(created.warnings.len(), 1);
        let body = read_tasks(cwd);
        assert!(body.contains("<!-- fleet:prd:begin id=\"demo\" v=\"2\" -->"));

        add(cwd, "demo", "P1", "first task").unwrap();
        assert!(read_tasks(cwd).contains("[ ] **P1** — first task"));

        let checked = mutate_checkbox(cwd, "demo", "P1", true, None).unwrap();
        assert_eq!(checked.message, "ok");
        assert!(read_tasks(cwd).contains("[x] **P1**"));
    }

    /// A `check` on a plan that isn't in any TASKS.md is a hard error, not a
    /// silent no-op — same contract the CLI had.
    #[test]
    fn check_unknown_plan_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = mutate_checkbox(tmp.path(), "nope", "P1", true, None).unwrap_err();
        assert!(err.contains("not found"));
    }

    /// A `create` that declares neither position is refused, and the error names
    /// the plans it could plausibly branch off. This is the whole point of P3:
    /// with `--parent` merely optional, plans were authored flat by default and
    /// the tree never came into existence.
    #[test]
    fn create_without_parent_or_root_is_refused_and_suggests_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        // An existing plan with pending work becomes a suggested parent.
        create(cwd, "host", "Host plan", None, true, pt::PlanKind::Exec, None).unwrap();
        add(cwd, "host", "P1", "open task").unwrap();

        let err = create(cwd, "orphan", "Orphan", None, false, pt::PlanKind::Exec, None).unwrap_err();
        assert!(err.contains("--parent"), "names the flag: {err}");
        assert!(err.contains("--root"), "names the alternative: {err}");
        assert!(err.contains("host"), "suggests the pending plan as a parent: {err}");
        // Nothing was written.
        assert!(!read_tasks(cwd).contains("orphan"), "refused create must not write");
    }

    /// Declaring both positions is contradictory, not a silent precedence rule.
    #[test]
    fn create_with_both_parent_and_root_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        create(tmp.path(), "par", "Parent", None, true, pt::PlanKind::Exec, None).unwrap();
        let err = create(tmp.path(), "kid", "Kid", Some("par"), true, pt::PlanKind::Exec, None).unwrap_err();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    /// An empty/whitespace `--parent` is not a declaration — it must be treated
    /// as absent (matching `create_plan`'s own empty-slot handling) and so still
    /// require `--root`.
    #[test]
    fn create_with_blank_parent_still_requires_a_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        let err = create(tmp.path(), "x", "X", Some("   "), false, pt::PlanKind::Exec, None).unwrap_err();
        assert!(err.contains("--root"), "blank parent ⇒ still undeclared: {err}");
    }

    /// The two valid shapes both write, and a child records its parent link.
    #[test]
    fn create_accepts_root_and_parent_shapes() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        create(cwd, "top", "Top", None, true, pt::PlanKind::Exec, None).unwrap();
        assert!(read_tasks(cwd).contains("<!-- fleet:prd:begin id=\"top\" v=\"2\" -->"));

        let child = create(cwd, "side", "Side", Some("top"), false, pt::PlanKind::Exec, None).unwrap();
        assert!(child.message.contains("parent 'top'"), "{}", child.message);
        assert!(read_tasks(cwd).contains("id=\"side\" v=\"2\" parent=\"top\""));
    }

    /// `resume` refuses without a session id (its whole job is claiming focus),
    /// whereas `create`/`check` merely warn — locking that asymmetry.
    #[test]
    fn resume_without_session_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        create(tmp.path(), "demo", "Demo", None, true, pt::PlanKind::Exec, None).unwrap();
        add(tmp.path(), "demo", "P1", "task").unwrap();
        let err = resume(tmp.path(), "demo", None, None).unwrap_err();
        assert!(err.contains("no session id"));
    }
}
