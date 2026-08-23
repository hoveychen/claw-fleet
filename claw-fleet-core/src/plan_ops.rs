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
/// (authoring a plan is starting it).
///
/// **A plan spawned while you are executing another plan defaults to being its
/// child.** That default is the whole mechanism — see [`resolve_tree_position`]
/// for why the two previous designs (silent flat default, then a mandatory
/// explicit choice) both failed to build a tree.
pub fn create(
    cwd: &Path,
    plan_id: &str,
    title: &str,
    parent: Option<&str>,
    root: bool,
    root_reason: Option<&str>,
    kind: pt::PlanKind,
    session_id: Option<&str>,
) -> Result<PlanOutcome, String> {
    let focus = session_id
        .and_then(crate::task_progress::read)
        .map(|r| r.plan_id)
        // Another workspace's plan is not a parent candidate here.
        .filter(|id| pt::find_plan_source(cwd, id).is_some());
    create_in(
        cwd,
        plan_id,
        title,
        parent,
        root,
        root_reason,
        kind,
        session_id,
        focus.as_deref(),
    )
}

/// [`create`] with the session's focused plan injected, so tests can drive the
/// tree-position rules without writing into the process-global `~/.fleet`
/// (same rationale as [`crate::plan_gate::gate_reason_in`]).
#[allow(clippy::too_many_arguments)]
fn create_in(
    cwd: &Path,
    plan_id: &str,
    title: &str,
    parent: Option<&str>,
    root: bool,
    root_reason: Option<&str>,
    kind: pt::PlanKind,
    session_id: Option<&str>,
    focus: Option<&str>,
) -> Result<PlanOutcome, String> {
    let parent = parent.filter(|p| !p.trim().is_empty());
    let root_reason = root_reason.filter(|r| !r.trim().is_empty());
    let parent = resolve_tree_position(parent, root, root_reason, focus)?;

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
        Some(p) => {
            let inherited = if Some(p) == focus && !p.is_empty() {
                " — 默认挂在你当前执行的 plan 下;要另起一棵树用 --root --root-reason"
            } else {
                ""
            };
            format!(
                "created child plan '{plan_id}'{kindness} (parent '{p}') in {}{inherited}",
                path.display()
            )
        }
        None => format!("created plan '{plan_id}'{kindness} in {}", path.display()),
    };
    Ok(PlanOutcome { message, warnings })
}

/// Shortest `--root-reason` that counts as a justification, in characters. The
/// gate exists to make leaving your current plan's tree cost a moment's thought,
/// so the cheap answers it must price out are exactly the short ones — `-`,
/// `n/a`, `无`. Counted in `char`s, not bytes, so a four-character CJK reason is
/// not held to a stricter bar than a four-letter English one.
const MIN_ROOT_REASON_CHARS: usize = 4;

/// Resolve the new plan's parent from the explicit flags plus the session's
/// focused plan. `Ok(None)` means a root.
///
/// **Why the default is the focused plan.** Two earlier designs both failed to
/// produce a tree:
///
/// 1. `--parent` merely *available* → 350 of the first 355 plans in this repo
///    came out flat.
/// 2. An explicit `--parent` / `--root` choice made *mandatory* (2026-08-20) →
///    109 more came out flat, because `--root` answers the question at zero
///    cost without the agent working out how the new plan relates to what it is
///    already doing.
///
/// Both left the backtrack with no edges. The observed damage is a relay chain
/// in `agent-workspace`: one boss request ("audit the research flow for gaps")
/// produced a six-item list, each item became its own root plan (8 plans, 0
/// `parent=`), and so every hop finished its one plan, found no ancestor, and
/// ended the turn with the list unfinished. The boss had to keep asking whether
/// his original question was done — the macro goal existed only in a wiki doc
/// and in prose the agents hand-copied between handoff notes.
///
/// So the choice is not made mandatory, it is made *correct by default*: a plan
/// authored while you are executing another plan is that plan's child. Chaining
/// siblings into a line is fine — [`pt::resolve_backtrack_target`] skips
/// completed ancestors, so the walk always lands on the nearest unfinished work.
/// Leaving the tree stays possible, it just has to be said out loud.
fn resolve_tree_position<'a>(
    parent: Option<&'a str>,
    root: bool,
    root_reason: Option<&str>,
    focus: Option<&'a str>,
) -> Result<Option<&'a str>, String> {
    if parent.is_some() && root {
        return Err(
            "--parent and --root are mutually exclusive: a plan is either a child of \
             another plan or a new tree root, not both."
                .to_string(),
        );
    }
    if let Some(p) = parent {
        if root_reason.is_some() {
            return Err(format!(
                "--root-reason justifies starting a NEW TREE, but this plan declares \
                 --parent {p}, so it is a child and needs no such justification. Drop \
                 --root-reason to create it as a child of '{p}', or drop --parent and \
                 keep --root-reason to make it a root instead."
            ));
        }
        return Ok(Some(p));
    }

    let Some(focused) = focus else {
        // No plan in flight: this is a fresh topic, so a root is the only option
        // and demanding either a declaration or a justification is pure ceremony.
        return Ok(None);
    };

    if !root {
        return Ok(Some(focused)); // the default that builds the tree
    }
    let given = root_reason.map(str::trim).unwrap_or("");
    if given.chars().count() >= MIN_ROOT_REASON_CHARS {
        return Ok(None);
    }
    Err(format!(
        "you are currently executing plan '{focused}', so by default this new plan \
         becomes its child and Fleet walks you back to '{focused}' when it completes. \
         Starting a separate top-level tree instead needs `--root-reason \"<why this \
         work does not belong under {focused}>\"` — one line is enough. Drop --root \
         to accept the default, or pass `--parent <id>` to attach it elsewhere."
    ))
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

        let created = create(cwd, "demo", "Demo work", None, true, None, pt::PlanKind::Exec, None).unwrap();
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

    /// Declaring both positions is contradictory, not a silent precedence rule.
    #[test]
    fn create_with_both_parent_and_root_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        create(tmp.path(), "par", "Parent", None, true, None, pt::PlanKind::Exec, None).unwrap();
        let err = create(tmp.path(), "kid", "Kid", Some("par"), true, None, pt::PlanKind::Exec, None).unwrap_err();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    /// The two explicit shapes both write, and a child records its parent link.
    #[test]
    fn create_accepts_root_and_parent_shapes() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        create(cwd, "top", "Top", None, true, None, pt::PlanKind::Exec, None).unwrap();
        assert!(read_tasks(cwd).contains("<!-- fleet:prd:begin id=\"top\" v=\"2\" -->"));

        let child = create(cwd, "side", "Side", Some("top"), false, None, pt::PlanKind::Exec, None).unwrap();
        assert!(child.message.contains("parent 'top'"), "{}", child.message);
        assert!(read_tasks(cwd).contains("id=\"side\" v=\"2\" parent=\"top\""));
    }

    /// A workspace already holding a plan, plus the session focused on it —
    /// i.e. an agent that is mid-plan and now authors another one.
    fn ws_focused_on_host() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        create(tmp.path(), "host", "Host plan", None, true, None, pt::PlanKind::Exec, None).unwrap();
        add(tmp.path(), "host", "P1", "open task").unwrap();
        tmp
    }

    /// **The mechanism.** A plan authored while the session is executing another
    /// plan defaults to being its child — no flag needed. Two earlier designs
    /// (optional `--parent`, then a mandatory explicit choice) both produced flat
    /// forests; this is the one that actually builds an edge.
    #[test]
    fn a_plan_spawned_while_executing_another_defaults_to_its_child() {
        let tmp = ws_focused_on_host();
        let out = create_in(
            tmp.path(),
            "spawned",
            "Spawned mid-plan",
            None,
            false,
            None,
            pt::PlanKind::Exec,
            None,
            Some("host"),
        )
        .unwrap();
        assert!(
            read_tasks(tmp.path()).contains("id=\"spawned\" v=\"2\" parent=\"host\""),
            "the edge must be written: {}",
            read_tasks(tmp.path())
        );
        assert!(
            out.message.contains("parent 'host'") && out.message.contains("--root"),
            "the message must say the parent was inherited and how to opt out: {}",
            out.message
        );
    }

    /// The default must never override an explicit `--parent`.
    #[test]
    fn an_explicit_parent_wins_over_the_focused_plan() {
        let tmp = ws_focused_on_host();
        create(tmp.path(), "other", "Other", None, true, None, pt::PlanKind::Exec, None).unwrap();
        create_in(
            tmp.path(),
            "kid",
            "Kid",
            Some("other"),
            false,
            None,
            pt::PlanKind::Exec,
            None,
            Some("host"),
        )
        .unwrap();
        assert!(read_tasks(tmp.path()).contains("id=\"kid\" v=\"2\" parent=\"other\""));
    }

    /// Leaving your current plan's tree stays possible, but must be said out loud
    /// — this is the case the boss kept hitting: hop N finishes its plan, authors
    /// the next item as a fresh root, and the macro goal loses its last edge.
    #[test]
    fn leaving_the_focused_tree_requires_a_reason() {
        let tmp = ws_focused_on_host();
        let err = create_in(
            tmp.path(),
            "flat",
            "Flat",
            None,
            true,
            None,
            pt::PlanKind::Exec,
            None,
            Some("host"),
        )
        .unwrap_err();
        assert!(err.contains("--root-reason"), "names the flag: {err}");
        assert!(err.contains("host"), "names the plan being left: {err}");
        assert!(
            !read_tasks(tmp.path()).contains("flat"),
            "refused create must not write"
        );
    }

    /// The escape hatch is real: a stated reason lets the new tree through.
    #[test]
    fn a_stated_reason_lets_a_new_tree_through() {
        let tmp = ws_focused_on_host();
        let out = create_in(
            tmp.path(),
            "flat",
            "Flat",
            None,
            true,
            Some("与 host 无关,是另一条产品线的独立需求"),
            pt::PlanKind::Exec,
            None,
            Some("host"),
        )
        .unwrap();
        assert!(out.message.contains("created plan 'flat'"), "{}", out.message);
        assert!(read_tasks(tmp.path()).contains("id=\"flat\""));
        assert!(
            !read_tasks(tmp.path()).contains("id=\"flat\" v=\"2\" parent="),
            "a justified root must not be given a parent anyway"
        );
    }

    /// A token reason ("-", "n/a", "无") is the cheap answer the gate exists to
    /// price out, so it must not pass as a justification.
    #[test]
    fn a_token_reason_does_not_buy_a_new_tree() {
        let tmp = ws_focused_on_host();
        for token in ["-", "n/a", "无", "   x  "] {
            let err = create_in(
                tmp.path(),
                "flat",
                "Flat",
                None,
                true,
                Some(token),
                pt::PlanKind::Exec,
                None,
                Some("host"),
            )
            .unwrap_err();
            assert!(
                err.contains("--root-reason"),
                "token {token:?} must not satisfy the gate: {err}"
            );
        }
    }

    /// No plan in flight ⇒ this is a fresh topic from the boss. A root is the
    /// only sensible outcome, so neither a declaration nor a justification is
    /// demanded. This is the case that made the previous mandatory-choice design
    /// pure ceremony: a workspace's stale active plans are not *your* context.
    #[test]
    fn without_a_focused_plan_a_bare_create_is_a_root() {
        let tmp = ws_focused_on_host(); // 'host' exists and is pending…
        let out = create_in(
            tmp.path(),
            "fresh",
            "Fresh topic",
            None,
            false,
            None,
            pt::PlanKind::Exec,
            None,
            None, // …but this session is executing nothing
        )
        .unwrap();
        assert!(out.message.contains("created plan 'fresh'"), "{}", out.message);
        assert!(
            !read_tasks(tmp.path()).contains("id=\"fresh\" v=\"2\" parent="),
            "no focus ⇒ no inherited parent"
        );
    }

    /// A focused plan that is already complete still parents the next one. The
    /// walk skips finished ancestors ([`pt::resolve_backtrack_target`]), so
    /// chaining is harmless — whereas requiring the focus to be *pending* would
    /// re-open the exact hole this fixes: hop N ticks its last box, authors the
    /// next item, and the chain breaks right at the handoff boundary.
    #[test]
    fn a_completed_focus_still_parents_the_next_plan() {
        let tmp = ws_focused_on_host();
        mutate_checkbox(tmp.path(), "host", "P1", true, None).unwrap();
        create_in(
            tmp.path(),
            "next",
            "Next",
            None,
            false,
            None,
            pt::PlanKind::Exec,
            None,
            Some("host"),
        )
        .unwrap();
        assert!(read_tasks(tmp.path()).contains("id=\"next\" v=\"2\" parent=\"host\""));
    }

    /// A blank `--parent` is not a declaration — it falls through to the default
    /// rather than being written as an empty parent attribute.
    #[test]
    fn a_blank_parent_falls_through_to_the_default() {
        let tmp = ws_focused_on_host();
        create_in(
            tmp.path(),
            "x",
            "X",
            Some("   "),
            false,
            None,
            pt::PlanKind::Exec,
            None,
            Some("host"),
        )
        .unwrap();
        assert!(read_tasks(tmp.path()).contains("id=\"x\" v=\"2\" parent=\"host\""));
    }

    /// `--root-reason` justifies *being a root*; pairing it with `--parent` means
    /// the agent misread the flag. Refuse rather than silently ignore it, or it
    /// reads as "I wrote a reason, so I'm covered" while the plan is a child.
    #[test]
    fn root_reason_with_parent_is_refused() {
        let tmp = ws_focused_on_host();
        let err = create(
            tmp.path(),
            "kid",
            "Kid",
            Some("host"),
            false,
            Some("这条理由不该出现在子 plan 上"),
            pt::PlanKind::Exec,
            None,
        )
        .unwrap_err();
        assert!(err.contains("--root-reason"), "{err}");
    }

    /// `resume` refuses without a session id (its whole job is claiming focus),
    /// whereas `create`/`check` merely warn — locking that asymmetry.
    #[test]
    fn resume_without_session_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        create(tmp.path(), "demo", "Demo", None, true, None, pt::PlanKind::Exec, None).unwrap();
        add(tmp.path(), "demo", "P1", "task").unwrap();
        let err = resume(tmp.path(), "demo", None, None).unwrap_err();
        assert!(err.contains("no session id"));
    }
}
