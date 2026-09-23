//! `fleet plan` — manage the workspace's TASKS.md PRD plans and record which
//! session is working which plan/P (PRD Discipline mode). All mutation logic
//! lives in [`claw_fleet_core::plan_ops`]; this file only resolves the session
//! id from env, dispatches, and prints — so the `fleet__plan` MCP tool executes
//! the identical logic.

use crate::commands::session::resolve_session_id;
use crate::PlanCommands;
use claw_fleet_core::plan_ops;

pub(crate) fn cmd_plan(action: PlanCommands, workspace: Option<&str>, session: Option<&str>) {
    let cwd = match crate::commands::session::resolve_workspace_dir(workspace) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };
    let sid = resolve_session_id(session);
    let result: Result<(), String> = match action {
        PlanCommands::Check { plan_id, task } => emit(plan_ops::mutate_checkbox(
            &cwd,
            &plan_id,
            &task,
            true,
            sid.as_deref(),
        )),
        PlanCommands::Uncheck { plan_id, task } => emit(plan_ops::mutate_checkbox(
            &cwd,
            &plan_id,
            &task,
            false,
            sid.as_deref(),
        )),
        PlanCommands::Resume { plan_id, task } => emit(plan_ops::resume(
            &cwd,
            &plan_id,
            task.as_deref(),
            sid.as_deref(),
        )),
        PlanCommands::Create {
            plan_id,
            title,
            parent,
            root,
            root_reason,
            kind,
        } => emit(plan_ops::create(
            &cwd,
            &plan_id,
            &title,
            parent.as_deref(),
            root,
            root_reason.as_deref(),
            claw_fleet_core::prd_tasks::PlanKind::from_attr(Some(&kind)),
            sid.as_deref(),
        )),
        PlanCommands::Add {
            plan_id,
            task,
            text,
        } => emit(plan_ops::add(&cwd, &plan_id, &task, &text)),
        PlanCommands::Migrate { path } => emit(plan_ops::migrate(&cwd, path)),
        PlanCommands::Snooze {
            plan_id,
            duration,
            reason,
        } => emit(plan_ops::snooze(
            &cwd,
            &plan_id,
            &duration,
            &reason,
            sid.as_deref(),
        )),
        PlanCommands::Unsnooze { plan_id } => emit(plan_ops::unsnooze(&cwd, &plan_id)),
        PlanCommands::Orphans { json } => plan_orphans(json),
        PlanCommands::List => plan_list(&cwd),
        PlanCommands::Get { plan_id } => plan_get(&cwd, &plan_id),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Print a [`plan_ops::PlanOutcome`]: focus-attribution warnings to stderr
/// (prefixed, matching the historical CLI output), then the message to stdout.
fn emit(outcome: Result<plan_ops::PlanOutcome, String>) -> Result<(), String> {
    let o = outcome?;
    for w in &o.warnings {
        eprintln!("warning: {w}");
    }
    println!("{}", o.message);
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

/// `fleet plan orphans`: the reviver's verdict for every candidate plan, across
/// all workspaces. Read-only.
fn plan_orphans(json: bool) -> Result<(), String> {
    let report = claw_fleet_core::plan_revive::dry_run();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if report.is_empty() {
        println!("(no recently claimed plan has pending work)");
        return Ok(());
    }
    for r in report {
        println!(
            "{} {} [{}/{}] owner={} — {}",
            r.workspace_path,
            r.plan_id,
            r.done,
            r.total,
            &r.newest_owner[..r.newest_owner.len().min(8)],
            r.verdict
        );
    }
    Ok(())
}
