//! `fleet plan` — manage the workspace's TASKS.md PRD plans and record which
//! session is working which plan/P (PRD Discipline mode).

use crate::commands::session::read_fleet_session_id;
use crate::PlanCommands;

pub(crate) fn cmd_plan(action: PlanCommands) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let result: Result<(), String> = match action {
        PlanCommands::Check { plan_id, task } => plan_mutate_checkbox(&cwd, &plan_id, &task, true),
        PlanCommands::Uncheck { plan_id, task } => {
            plan_mutate_checkbox(&cwd, &plan_id, &task, false)
        }
        PlanCommands::Resume { plan_id, task } => plan_resume(&cwd, &plan_id, task.as_deref()),
        PlanCommands::Create {
            plan_id,
            title,
            parent,
        } => plan_create(&cwd, &plan_id, &title, parent.as_deref()),
        PlanCommands::Add { plan_id, task, text } => plan_add(&cwd, &plan_id, &task, &text),
        PlanCommands::Migrate { path } => plan_migrate(&cwd, path),
        PlanCommands::List => plan_list(&cwd),
        PlanCommands::Get { plan_id } => plan_get(&cwd, &plan_id),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Workspace's primary TASKS.md (main checkout root, else cwd).
fn workspace_tasks_path(cwd: &std::path::Path) -> std::path::PathBuf {
    match claw_fleet_core::prd_tasks::discover_main_checkout_root(cwd) {
        Some(root) => root.join("TASKS.md"),
        None => cwd.join("TASKS.md"),
    }
}

/// Record this session's focus (plan + current pending P) in the side-channel.
/// Warns and skips attribution when no session id is set — the file edit still
/// applies, but the desktop card will show no plan, and a silent skip makes that
/// impossible to diagnose.
fn plan_record_focus(cwd: &std::path::Path, plan_id: &str, content: &str) {
    use claw_fleet_core::prd_tasks as pt;
    let Some(sid) = read_fleet_session_id() else {
        eprintln!(
            "warning: no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set); \
             plan '{plan_id}' was updated but this session won't be attributed to it."
        );
        return;
    };
    let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let current = pt::plan_body(content, plan_id).and_then(|b| pt::first_pending_task(&b));
    if let Err(e) =
        claw_fleet_core::task_progress::set_current(&sid, &ws.to_string_lossy(), plan_id, current)
    {
        eprintln!("warning: could not record task focus: {e}");
    }
}

fn plan_mutate_checkbox(
    cwd: &std::path::Path,
    plan_id: &str,
    task: &str,
    done: bool,
) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let path = pt::find_plan_source(cwd, plan_id)
        .ok_or_else(|| format!("plan '{plan_id}' not found in any TASKS.md"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = pt::set_checkbox(&content, plan_id, task, done)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    plan_record_focus(cwd, plan_id, &updated);
    // Ticking the last box of a child plan: point the session back at the
    // nearest ancestor that still has work, so the plan tree doesn't strand.
    if done {
        if let Some(msg) = backtrack_on_completion(cwd, plan_id, &updated) {
            println!("{msg}");
            return Ok(());
        }
    }
    println!("ok");
    Ok(())
}

/// When `plan_id` just became fully complete (no pending top-level task left)
/// AND it is a child plan whose nearest pending ancestor still has work,
/// re-attribute this session's focus to that ancestor and return a directive
/// telling the agent to keep going there. Returns `None` when the plan still
/// has pending tasks or there is nowhere to backtrack to (top-level plan, or
/// every ancestor already complete).
fn backtrack_on_completion(
    cwd: &std::path::Path,
    plan_id: &str,
    content: &str,
) -> Option<String> {
    use claw_fleet_core::prd_tasks as pt;
    let body = pt::plan_body(content, plan_id)?;
    if body.lines().any(pt::is_pending_task_line) {
        return None; // not fully complete yet
    }
    let target = pt::resolve_backtrack_target(cwd, plan_id)?;
    // Re-point focus at the ancestor so the desktop card follows immediately,
    // without waiting for the agent to run `fleet plan resume` itself.
    if let Some(sid) = read_fleet_session_id() {
        let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        if let Err(e) = claw_fleet_core::task_progress::set_current(
            &sid,
            &ws.to_string_lossy(),
            &target.plan_id,
            target.next_task.clone(),
        ) {
            eprintln!("warning: could not re-attribute focus to parent plan: {e}");
        }
    }
    let next = target
        .next_task
        .as_deref()
        .unwrap_or("第一个未完成的 P");
    Some(format!(
        "ok — 子 plan '{plan_id}' 已全部完成。其父 plan '{parent}' 尚有未完成任务,\
         Fleet 已把你的焦点切回 '{parent}'。请从 {next} 继续执行,不要结束 turn。",
        parent = target.plan_id,
    ))
}

fn plan_resume(cwd: &std::path::Path, plan_id: &str, task: Option<&str>) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let current = pt::resolve_current_task(cwd, plan_id, task)?;
    let sid = read_fleet_session_id()
        .ok_or("no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set)")?;
    let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    claw_fleet_core::task_progress::set_current(&sid, &ws.to_string_lossy(), plan_id, current)?;
    println!("ok");
    Ok(())
}

fn plan_create(
    cwd: &std::path::Path,
    plan_id: &str,
    title: &str,
    parent: Option<&str>,
) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let path = workspace_tasks_path(cwd);
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = pt::create_plan(&content, plan_id, title, parent)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    // Writing a plan is starting it: the agent authoring the block is the agent
    // about to execute it. Claiming focus here spares it a separate `resume`.
    plan_record_focus(cwd, plan_id, &updated);
    match parent {
        Some(p) if !p.trim().is_empty() => {
            println!(
                "created child plan '{plan_id}' (parent '{}') in {}",
                p.trim(),
                path.display()
            );
        }
        _ => println!("created plan '{plan_id}' in {}", path.display()),
    }
    Ok(())
}

/// Appends a pending task. Deliberately does NOT claim focus: `add` edits a
/// plan's structure and says nothing about who is executing it — a master
/// appending a P-item to another session's plan must not repoint its own card.
fn plan_add(cwd: &std::path::Path, plan_id: &str, task: &str, text: &str) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let path = pt::find_plan_source(cwd, plan_id)
        .ok_or_else(|| format!("plan '{plan_id}' not found in any TASKS.md"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = pt::add_task(&content, plan_id, task, text)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("ok");
    Ok(())
}

fn plan_migrate(cwd: &std::path::Path, path: Option<std::path::PathBuf>) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let target = path.unwrap_or_else(|| workspace_tasks_path(cwd));
    let content =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let migrated = pt::migrate_v1_to_v2(&content);
    if migrated == content {
        println!("already v2 (no changes): {}", target.display());
        return Ok(());
    }
    std::fs::write(&target, &migrated).map_err(|e| format!("write {}: {e}", target.display()))?;
    println!("migrated {}", target.display());
    Ok(())
}

fn plan_list(cwd: &std::path::Path) -> Result<(), String> {
    let plans = claw_fleet_core::prd_tasks::list_workspace_task_plans(cwd, None);
    if plans.is_empty() {
        println!("(no plans)");
        return Ok(());
    }
    for p in plans {
        let done = p.items.iter().filter(|i| i.done).count();
        let src = p.source.map(|s| format!(" — {s}")).unwrap_or_default();
        println!(
            "{} [{}/{}]{}",
            p.id.as_deref().unwrap_or("(anonymous)"),
            done,
            p.items.len(),
            src
        );
    }
    Ok(())
}

fn plan_get(cwd: &std::path::Path, plan_id: &str) -> Result<(), String> {
    let plans = claw_fleet_core::prd_tasks::list_workspace_task_plans(cwd, None);
    let p = plans
        .iter()
        .find(|p| p.id.as_deref() == Some(plan_id))
        .ok_or_else(|| format!("plan '{plan_id}' not found"))?;
    for it in &p.items {
        println!("{} {}", if it.done { "[x]" } else { "[ ]" }, it.text);
    }
    Ok(())
}
