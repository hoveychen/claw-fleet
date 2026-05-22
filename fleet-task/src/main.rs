//! `fleet-task` — Phase 2 standalone binary that owns one task's complete
//! lifecycle (master + workers + plan) in a single process.
//!
//! See `TASKS.md` plan `fleet-task-binary` for the full Phase 2 scope.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};

mod http;
mod local_host;
mod runtime;
mod sse;
mod tui;

#[derive(Parser, Debug)]
#[command(name = "fleet-task", version, about = "Run one task's lifecycle in this process. Run with no arguments to open the task picker (resume) — mirrors how `claude` lists recents on bare invocation.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start a new task in the given workspace and spawn its master agent.
    New {
        /// Project workspace directory (must be a git repo).
        #[arg(long)]
        workspace: PathBuf,
        /// Initial task description / prompt for the master agent.
        #[arg(long)]
        prompt: String,
        /// Task title; defaults to the prompt's first 40 chars.
        #[arg(long)]
        title: Option<String>,
        /// Skip the TUI; run as a headless HTTP-only daemon.
        #[arg(long, default_value_t = false)]
        no_tui: bool,
    },
    /// Resume an existing task by id (`~/.fleet/tasks/<id>.json`).
    Resume {
        /// Task id to resume.
        task_id: String,
        /// Workspace directory (Phase 2 doesn't persist it in task.json yet).
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long, default_value_t = false)]
        no_tui: bool,
    },
    /// Open a TUI picker over all tasks on disk; Enter resumes the
    /// highlighted task. Uses `task.workspace` (persisted by Phase 3+
    /// `start_task`); supply `--workspace` to override for older tasks.
    List {
        /// Override the workspace path for the resumed task. When omitted,
        /// fleet-task reads it from `task.workspace`; errors if neither is
        /// set (legacy task json from before Phase 3).
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_tui: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Default behaviour with no subcommand: open the launchpad — a textarea
    // for entering a new prompt in `$PWD`, with Ctrl-T to switch to the
    // resume picker. Mirrors `claude`'s bare-invocation UX (prompt waiting +
    // shortcut to recents).
    let Some(command) = cli.command else {
        return run_launchpad();
    };
    match command {
        Command::New {
            workspace,
            prompt,
            title,
            no_tui,
        } => {
            let title = title.unwrap_or_else(|| default_title_from_prompt(&prompt));
            run_task(
                runtime::boot_new_task(
                    runtime::NewTaskArgs {
                        workspace,
                        title,
                        prompt,
                    },
                    Box::new(local_host::ClaudeLauncher),
                )
                .map_err(anyhow::Error::msg)?,
                no_tui,
            )
        }
        Command::Resume {
            task_id,
            workspace,
            no_tui,
        } => run_task(
            runtime::boot_resume(task_id, workspace, Box::new(local_host::ClaudeLauncher))
                .map_err(anyhow::Error::msg)?,
            no_tui,
        ),
        Command::List { workspace, no_tui } => {
            let outcome = tui::run_picker().map_err(anyhow::Error::from)?;
            match outcome {
                tui::PickerOutcome::Empty => {
                    eprintln!("no tasks on disk (~/.fleet/tasks/ is empty)");
                    Ok(())
                }
                tui::PickerOutcome::Quit => Ok(()),
                tui::PickerOutcome::Selected(task_id) => {
                    let task = claw_fleet_task::task::get_task(&task_id)
                        .map_err(anyhow::Error::msg)?;
                    let ws = workspace.or(task.workspace.clone()).ok_or_else(|| {
                        anyhow::anyhow!(
                            "task {} has no persisted workspace (pre-Phase-3); \
                             pass --workspace to override",
                            task_id
                        )
                    })?;
                    run_task(
                        runtime::boot_resume(task_id, ws, Box::new(local_host::ClaudeLauncher))
                            .map_err(anyhow::Error::msg)?,
                        no_tui,
                    )
                }
            }
        }
    }
}

/// Bare-invocation entry point: open the launchpad TUI and dispatch on the
/// user's choice. New tasks default to `$PWD` as workspace.
fn run_launchpad() -> anyhow::Result<()> {
    let pwd = std::env::current_dir().map_err(anyhow::Error::from)?;
    let action = tui::run_launchpad(pwd).map_err(anyhow::Error::from)?;
    match action {
        tui::LaunchAction::Quit => Ok(()),
        tui::LaunchAction::StartNew {
            workspace,
            prompt,
            title,
        } => run_task(
            runtime::boot_new_task(
                runtime::NewTaskArgs {
                    workspace,
                    title,
                    prompt,
                },
                Box::new(local_host::ClaudeLauncher),
            )
            .map_err(anyhow::Error::msg)?,
            false,
        ),
        tui::LaunchAction::Resume { task_id } => {
            let task = claw_fleet_task::task::get_task(&task_id)
                .map_err(anyhow::Error::msg)?;
            let ws = task.workspace.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "task {task_id} has no persisted workspace (pre-Phase-3); \
                     use `fleet-task resume {task_id} --workspace=...` instead"
                )
            })?;
            run_task(
                runtime::boot_resume(task_id, ws, Box::new(local_host::ClaudeLauncher))
                    .map_err(anyhow::Error::msg)?,
                false,
            )
        }
    }
}

fn run_task(boot: runtime::Bootstrap, no_tui: bool) -> anyhow::Result<()> {
    eprintln!(
        "fleet-task running task {} on http://127.0.0.1:{} (pid {})",
        boot.task_id,
        boot.http_handle.port,
        std::process::id()
    );
    let stop = Arc::new(AtomicBool::new(false));
    install_signals(stop.clone());

    if no_tui {
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    } else {
        let ctx = tui::TuiContext {
            task_id: boot.task_id.clone(),
            http_port: boot.http_handle.port,
        };
        let _ = tui::run(ctx, stop.clone());
    }

    eprintln!("fleet-task shutting down (30s deadline)…");
    runtime::shutdown_with_deadline(boot, Duration::from_secs(30));
    Ok(())
}

/// Install SIGINT + SIGTERM handlers. First signal flips the stop flag for a
/// graceful shutdown; a second signal forces `process::exit(130)` so a stuck
/// foreground loop can't trap the operator.
fn install_signals(stop: Arc<AtomicBool>) {
    #[cfg(unix)]
    {
        use std::sync::OnceLock;
        static SIGNAL_COUNT: OnceLock<Arc<AtomicU8>> = OnceLock::new();
        static STOP_REF: OnceLock<Arc<AtomicBool>> = OnceLock::new();
        let _ = SIGNAL_COUNT.set(Arc::new(AtomicU8::new(0)));
        let _ = STOP_REF.set(stop);

        extern "C" fn handler(_sig: libc::c_int) {
            if let (Some(count), Some(stop)) = (SIGNAL_COUNT.get(), STOP_REF.get()) {
                let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                stop.store(true, Ordering::SeqCst);
                if n >= 2 {
                    // Second signal — operator is impatient; force exit.
                    std::process::exit(130);
                }
            }
        }
        unsafe {
            libc::signal(libc::SIGINT, handler as libc::sighandler_t);
            libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = stop;
    }
}

fn default_title_from_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let cut: String = trimmed.chars().take(40).collect();
    if cut.is_empty() {
        "untitled fleet-task".into()
    } else {
        cut
    }
}
