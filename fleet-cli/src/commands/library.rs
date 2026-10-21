//! `fleet library` — whole-library chores: the one-time summary backfill and
//! re-filing a project's existing items into its folder schema.

use std::time::Duration;

use claw_fleet_core::library::Roots;
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
            "  of which backlog: {} doc(s), {} text artifact(s), {} image(s)",
            p.docs, p.text_artifacts, p.image_artifacts
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
        let mark = match &t.error {
            None => format!("{}ok{} ${:.4}", "\x1b[32m", c_reset(), t.cost_usd),
            Some(e) => format!("{}FAILED{} {}", "\x1b[31m", c_reset(), truncate(e, 80)),
        };
        println!(
            "[{}/{} · {} failed · ${:.3} · {}] {} {} — {}",
            r.done + r.failed,
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
        "{}Backfill {}{}: {} summarized, {} failed of {} planned · ${:.3} · {} (this run {}, {} resume(s))",
        c_bold(),
        match run.state {
            BackfillState::Done => "done",
            BackfillState::Stopped => "stopped",
            _ => "ended",
        },
        c_reset(),
        run.done,
        run.failed,
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
