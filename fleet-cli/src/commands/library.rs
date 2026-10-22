//! `fleet library` — whole-library chores: the one-time summary backfill and
//! re-filing a project's existing items into its folder schema.

use std::time::Duration;

use claw_fleet_core::library::{ProjectHealth, Roots};
use claw_fleet_core::library_ingest::{self, refile};
use claw_fleet_core::library_summary::{self, BackfillOpts, BackfillState, BackfillTick};

use crate::fmt::*;
use crate::LibraryCommands;

fn fail(e: impl std::fmt::Display) -> ! {
    eprintln!("{}Error:{} {e}", "\x1b[31m", c_reset());
    std::process::exit(1);
}

fn minutes(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}m{:02}s", s / 60, s % 60)
}

pub(crate) fn cmd_library(action: LibraryCommands) {
    match action {
        LibraryCommands::Backfill { dry_run, concurrency, limit, json } => {
            backfill(dry_run, concurrency, limit, json)
        }
        LibraryCommands::Workspaces { json } => workspaces(json),
        LibraryCommands::Refile { workspace, all, dry_run, force, json } => {
            refile(workspace, all, dry_run, force, json)
        }
        LibraryCommands::RefileUndo { batch } => {
            let b = library_ingest::undo_batch(&batch).unwrap_or_else(|e| fail(e));
            let s = b.summary();
            println!(
                "Undid batch {} ({}): {} item(s) back where they were; {} folder(s) kept because they hold other items",
                b.id,
                b.workspace_name,
                s.undone,
                b.created_folders.len()
            );
        }
        LibraryCommands::MigrateSlugs { workspace, dry_run, json } => migrate_slugs(workspace, dry_run, json),
        LibraryCommands::FixLanguage { dry_run, concurrency, limit, json } => {
            fix_language(dry_run, concurrency, limit, json)
        }
        LibraryCommands::RestoreSummaries { backup } => {
            let roots = Roots::home().unwrap_or_else(|e| fail(e));
            let n = library_summary::restore_summaries_in(&roots, &backup).unwrap_or_else(|e| fail(e));
            println!("Restored {n} summary(ies) from {}", backup.display());
        }
    }
}

fn fix_language(dry_run: bool, concurrency: usize, limit: Option<usize>, json: bool) {
    let roots = Roots::home().unwrap_or_else(|e| fail(e));
    let index = claw_fleet_core::library_index::LibraryIndex::open_at(&claw_fleet_core::library_index::db_path(&roots))
        .unwrap_or_else(|e| fail(e));
    index.sync(&roots).unwrap_or_else(|e| fail(e));
    let found = library_summary::language_mismatches(&roots, &index).unwrap_or_else(|e| fail(e));
    let todo: Vec<_> = found.iter().take(limit.unwrap_or(usize::MAX)).collect();
    if dry_run {
        if json {
            println!("{}", serde_json::to_string_pretty(&found).unwrap());
            return;
        }
        for m in &todo {
            println!(
                "  [{}] {} {} — {}",
                m.project_language,
                m.kind,
                truncate(&m.title, 50),
                truncate(&m.summary, 70)
            );
        }
        println!(
            "{} summary(ies) not in their project's language; rewriting {} costs about ${:.2}",
            found.len(),
            todo.len(),
            todo.len() as f64 * library_summary::EST_TEXT_CALL_USD
        );
        return;
    }
    let targets: Vec<(String, String)> = todo.iter().map(|m| (m.kind.clone(), m.id.clone())).collect();
    let tick = |t: &BackfillTick| {
        if json {
            return;
        }
        let r = &t.run;
        let mark = match (&t.error, &t.skipped) {
            (Some(e), _) => format!("{}FAILED{} {}", "\x1b[31m", c_reset(), truncate(e, 80)),
            (None, Some(why)) => format!("skipped ({why})"),
            (None, None) => format!("{}ok{} ${:.4}", "\x1b[32m", c_reset(), t.cost_usd),
        };
        println!(
            "[{}/{} · ${:.3}] {} {} — {}",
            r.done + r.failed + r.skipped,
            r.planned,
            r.cost_usd,
            t.kind,
            truncate(&t.title, 50),
            mark
        );
    };
    let report = library_summary::resummarize_in(
        &roots,
        &targets,
        concurrency,
        None,
        &claw_fleet_core::library_ingest::LiveBrain,
        &tick,
    )
    .unwrap_or_else(|e| fail(e));
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        return;
    }
    println!(
        "{}Rewrote {} of {} summary(ies){}, {} failed · ${:.3}",
        c_bold(),
        report.done,
        report.planned,
        c_reset(),
        report.failed,
        report.cost_usd
    );
    for f in &report.failures {
        println!("  {} {} — {}", f.kind, truncate(&f.id, 40), truncate(&f.error, 100));
    }
    if let Some(b) = &report.backup {
        println!("Old summaries backed up — undo with `fleet library restore-summaries {}`", b.display());
    }
}

fn migrate_slugs(workspace: Option<std::path::PathBuf>, dry_run: bool, json: bool) {
    let ws = workspace.map(|p| p.display().to_string());
    let moves = claw_fleet_core::library::migrate_namespaces(ws.as_deref(), dry_run).unwrap_or_else(|e| fail(e));
    if json {
        println!("{}", serde_json::to_string_pretty(&moves).unwrap());
        return;
    }
    for m in &moves {
        match &m.error {
            None => println!("  {} → {}", m.from, m.to),
            Some(e) => println!("  {}{} — not moved: {e}{}", "\x1b[33m", m.from, c_reset()),
        }
    }
    let failed = moves.iter().filter(|m| m.error.is_some()).count();
    println!(
        "{} {} doc(s){}",
        if dry_run { "Would move" } else { "Moved" },
        moves.len() - failed,
        if failed > 0 { format!(", {failed} not moved") } else { String::new() }
    );
}

fn health_label(h: ProjectHealth) -> &'static str {
    match h {
        ProjectHealth::Ok => "ok",
        ProjectHealth::Missing => "missing",
        ProjectHealth::Temp => "temp",
        ProjectHealth::Home => "home",
        ProjectHealth::Tiny => "tiny",
    }
}

fn workspaces(json: bool) {
    let rows = refile::workspaces().unwrap_or_else(|e| fail(e));
    if json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        return;
    }
    let (docs, arts): (usize, usize) = rows.iter().fold((0, 0), |(d, a), r| (d + r.docs, a + r.artifacts));
    println!(
        "{}{} project(s) · {} doc(s) + {} artifact(s){}",
        c_bold(),
        rows.len(),
        docs,
        arts,
        c_reset()
    );
    println!("{:>5} {:>5} {:>7} {:>7}  {:<8} {:<10} project", "docs", "arts", "unfiled", "folders", "health", "refile");
    for r in &rows {
        println!(
            "{:>5} {:>5} {:>7} {:>7}  {:<8} {:<10} {}",
            r.docs,
            r.artifacts,
            r.unfiled,
            r.folders,
            health_label(r.health),
            r.refile_status.map(|s| format!("{s:?}").to_lowercase()).unwrap_or_else(|| "-".into()),
            r.workspace_path
        );
    }
    let by = |h: ProjectHealth| rows.iter().filter(|r| r.health == h).count();
    println!(
        "{}healthy {} · tiny {} · temp {} · missing {} · home {} — refile skips all but healthy unless --force{}",
        c_dim(),
        by(ProjectHealth::Ok),
        by(ProjectHealth::Tiny),
        by(ProjectHealth::Temp),
        by(ProjectHealth::Missing),
        by(ProjectHealth::Home),
        c_reset()
    );
}

fn print_plan(p: &refile::RefilePlan) {
    println!("{}Re-file {} — dry run{}  ({})", c_bold(), p.workspace_name, c_reset(), p.workspace_path);
    println!("  unfiled:   {} doc(s), {} artifact(s)", p.unfiled_docs, p.unfiled_artifacts);
    if p.will_draft {
        println!("  schema:    none yet — one will be drafted from README/CLAUDE.md and the items' titles + summaries");
    } else {
        println!("  schema:    {} folder(s)", p.folders);
    }
    println!("  calls:     ~{} (standard model), est. ${:.2}", p.calls, p.est_cost_usd);
    if p.health != ProjectHealth::Ok {
        println!("  {}skipped:{}   {} (--force to re-file anyway)", "\x1b[33m", c_reset(), p.health_reason);
    }
    if let Some(b) = &p.running_batch {
        println!("  running:   batch {b}");
    }
}

fn print_batch(b: &library_ingest::IngestBatch, elapsed: std::time::Duration) {
    let s = b.summary();
    println!(
        "{}Re-filed {}{}: {:?} · {} item(s): {} filed into folders, {} left in place, {} failed · {} call(s) · ${:.3} · {}",
        c_bold(),
        b.workspace_name,
        c_reset(),
        b.status,
        s.total,
        s.filed - s.unfiled,
        s.unfiled,
        s.failed,
        b.llm_calls,
        b.cost_usd,
        minutes(elapsed.as_millis() as u64)
    );
    if let Some(e) = &b.error {
        println!("  {}note:{} {e}", "\x1b[33m", c_reset());
    }
    if b.schema_drafted {
        println!("  drafted folders: {}", b.created_folders.join(", "));
    }
    for n in &b.new_folders {
        println!("  new folder {} — {}", n.path, n.reason);
    }
    println!("  batch {} — undo with `fleet library refile-undo {}` or in the desktop's library", b.id, b.id);
}

fn refile(workspace: Option<std::path::PathBuf>, all: bool, dry_run: bool, force: bool, json: bool) {
    let roots = Roots::home().unwrap_or_else(|e| fail(e));
    let targets: Vec<String> = if all {
        refile::workspaces_in(&roots)
            .into_iter()
            .filter(|r| r.health == ProjectHealth::Ok)
            .filter(|r| r.refile_status.is_none_or(|s| s == library_ingest::BatchStatus::Undone))
            .map(|r| r.workspace_path)
            .collect()
    } else {
        let ws = workspace
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| fail("no workspace"));
        vec![ws.display().to_string()]
    };
    for ws in targets {
        if dry_run {
            let plan = refile::refile_plan_in(&roots, &ws).unwrap_or_else(|e| fail(e));
            if json {
                println!("{}", serde_json::to_string_pretty(&plan).unwrap());
            } else {
                print_plan(&plan);
            }
            continue;
        }
        let started = std::time::Instant::now();
        let b = match refile::run_refile_in(&roots, &ws, force, &library_ingest::LiveBrain) {
            Ok(b) => b,
            Err(e) if all => {
                eprintln!("{ws}: {e}");
                continue;
            }
            Err(e) => fail(e),
        };
        if json {
            println!("{}", serde_json::to_string_pretty(&b).unwrap());
        } else {
            print_batch(&b, started.elapsed());
        }
    }
}

fn backfill(dry_run: bool, concurrency: usize, limit: Option<usize>, json: bool) {
    let roots = Roots::home().unwrap_or_else(|e| fail(e));
    if dry_run {
        // Extract every artifact's text first: one not extracted yet is not
        // counted as due.
        let status = library_summary::backfill_status_in(&roots, Duration::from_secs(3600))
            .unwrap_or_else(|e| fail(e));
        if json {
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            return;
        }
        let p = &status.plan;
        println!("{}Summary backfill — dry run{}", c_bold(), c_reset());
        println!("  due now:          {} item(s), est. ${:.2}", status.due, status.due_cost_usd);
        println!(
            "  of which backlog: {} doc(s), {} text artifact(s), {} image(s), {} video(s)",
            p.docs, p.text_artifacts, p.image_artifacts, p.video_artifacts
        );
        println!("  new since index:  {}", p.new_pending);
        if let Some(r) = &status.run {
            println!(
                "  last run:         {:?} — {}/{} done, {} failed, ${:.3}, {}",
                r.state,
                r.done,
                r.planned,
                r.failed,
                r.cost_usd,
                minutes(r.elapsed_ms)
            );
        }
        if status.running {
            println!("  a backfill is running right now");
        }
        return;
    }

    let started = std::time::Instant::now();
    let tick = |t: &BackfillTick| {
        if json {
            return;
        }
        let r = &t.run;
        let mark = match (&t.error, &t.skipped) {
            (Some(e), _) => format!("{}FAILED{} {}", "\x1b[31m", c_reset(), truncate(e, 80)),
            (None, Some(why)) => format!("skipped ({why})"),
            (None, None) => format!("{}ok{} ${:.4}", "\x1b[32m", c_reset(), t.cost_usd),
        };
        println!(
            "[{}/{} · {} failed · ${:.3} · {}] {} {} — {}",
            r.done + r.failed + r.skipped,
            r.planned,
            r.failed,
            r.cost_usd,
            minutes(started.elapsed().as_millis() as u64),
            t.kind,
            truncate(&t.title, 50),
            mark
        );
    };
    let opts = BackfillOpts { concurrency, limit };
    let run = library_summary::run_backfill_in(
        &roots,
        &opts,
        &claw_fleet_core::library_ingest::LiveBrain,
        &tick,
    )
    .unwrap_or_else(|e| fail(e));
    if json {
        println!("{}", serde_json::to_string_pretty(&run).unwrap());
        return;
    }
    println!();
    println!(
        "{}Backfill {}{}: {} summarized, {} failed, {} skipped of {} planned · ${:.3} · {} (this run {}, {} resume(s))",
        c_bold(),
        match run.state {
            BackfillState::Done => "done",
            BackfillState::Stopped => "stopped",
            _ => "ended",
        },
        c_reset(),
        run.done,
        run.failed,
        run.skipped,
        run.planned,
        run.cost_usd,
        minutes(run.elapsed_ms),
        minutes(started.elapsed().as_millis() as u64),
        run.resumes
    );
    for f in &run.failures {
        println!("  {} {} — {}", f.kind, truncate(&f.id, 40), truncate(&f.error, 100));
    }
    if run.state == BackfillState::Stopped {
        println!("{}Run `fleet library backfill` again to continue.{}", c_dim(), c_reset());
    }
}
