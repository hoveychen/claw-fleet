//! `fleet plan` — manage the workspace's TASKS.md PRD plans and record which
//! session is working which plan/P (PRD Discipline mode). All mutation logic
//! lives in [`claw_fleet_core::plan_ops`]; this file only resolves the session
//! id from env, dispatches, and prints — so the `fleet__plan` MCP tool executes
//! the identical logic.

use crate::commands::session::read_fleet_session_id;
use crate::PlanCommands;
use claw_fleet_core::plan_ops;

pub(crate) fn cmd_plan(action: PlanCommands) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let sid = read_fleet_session_id();
    let result: Result<(), String> = match action {
        PlanCommands::Check { plan_id, task } => {
            emit(plan_ops::mutate_checkbox(&cwd, &plan_id, &task, true, sid.as_deref()))
        }
        PlanCommands::Uncheck { plan_id, task } => {
            emit(plan_ops::mutate_checkbox(&cwd, &plan_id, &task, false, sid.as_deref()))
        }
        PlanCommands::Resume { plan_id, task } => {
            emit(plan_ops::resume(&cwd, &plan_id, task.as_deref(), sid.as_deref()))
        }
        PlanCommands::Create {
            plan_id,
            title,
            parent,
        } => emit(plan_ops::create(
            &cwd,
            &plan_id,
            &title,
            parent.as_deref(),
            sid.as_deref(),
        )),
        PlanCommands::Add { plan_id, task, text } => {
            emit(plan_ops::add(&cwd, &plan_id, &task, &text))
        }
        PlanCommands::Migrate { path } => emit(plan_ops::migrate(&cwd, path)),
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
