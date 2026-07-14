//! `fleet loop` — a recurring prompt Fleet re-runs on an interval by spawning a
//! fresh detached session each time, so it survives the turn boundary.

use crate::commands::session::read_fleet_session_id;
use crate::LoopCommands;

pub(crate) fn cmd_loop(action: LoopCommands) {
    use claw_fleet_core::agent_loop;
    match action {
        LoopCommands::Fire { id, generation } => {
            // The detached timer body — blocks, sleeping until due and firing.
            agent_loop::run_timer_blocking(&id, generation);
        }
        LoopCommands::List { json } => {
            let loops = agent_loop::list();
            if json {
                println!("{}", serde_json::to_string_pretty(&loops).unwrap_or_default());
                return;
            }
            if loops.is_empty() {
                println!("no loops registered");
                return;
            }
            let now = now_ms_wall();
            println!("{:<10}  {:<8}  {:<8}  {:<10}  PROMPT", "ID", "EVERY", "DONE", "NEXT");
            for l in loops {
                let cap = l
                    .max_iterations
                    .map(|m| format!("{}/{}", l.iterations_done, m))
                    .unwrap_or_else(|| l.iterations_done.to_string());
                let next = if l.is_due(now) {
                    "due".to_string()
                } else {
                    fmt_duration_ms(l.due_in_ms(now))
                };
                let prompt = l.prompt.replace('\n', " ");
                let prompt = if prompt.chars().count() > 48 {
                    format!("{}…", prompt.chars().take(48).collect::<String>())
                } else {
                    prompt
                };
                println!(
                    "{:<10}  {:<8}  {:<8}  {:<10}  {}",
                    l.id,
                    fmt_interval_secs(l.interval_secs),
                    cap,
                    next,
                    prompt
                );
            }
        }
        LoopCommands::Stop { id } => {
            if agent_loop::stop(&id) {
                println!("ok: loop {id} stopped (its timer exits on next wake)");
            } else {
                eprintln!("no loop with id {id}");
                std::process::exit(1);
            }
        }
        LoopCommands::Create {
            interval,
            prompt,
            max,
        } => {
            let interval_secs = match agent_loop::parse_interval(&interval) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            };
            let prompt = prompt.trim();
            if prompt.is_empty() {
                eprintln!("Error: --prompt is required.");
                std::process::exit(2);
            }
            // Inherit the creating session's workspace / model / effort, exactly
            // like handoff — so a loop created from a fable-5 session keeps
            // running on fable-5, in the session's real cwd (not a worktree that
            // may later be removed).
            let sid = read_fleet_session_id();
            let shell_cwd = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .to_string_lossy()
                .to_string();
            let ws = sid
                .as_deref()
                .and_then(claw_fleet_core::session::resolve_session_cwd)
                .unwrap_or_else(|| shell_cwd.clone());
            let model = sid
                .as_deref()
                .and_then(claw_fleet_core::session::resolve_session_model_spec);
            let effort = std::env::var("CLAUDE_EFFORT").ok().filter(|e| !e.trim().is_empty());

            match agent_loop::create(
                &ws,
                prompt,
                interval_secs,
                max,
                model.as_deref(),
                effort.as_deref(),
                sid.as_deref(),
            ) {
                Ok(rec) => {
                    // Arm the detached timer so the loop actually fires. A create
                    // that can't arm still leaves the record, and the Stop-hook
                    // reconcile will pick it up — so warn, don't fail.
                    match agent_loop::arm_timer(&rec) {
                        Ok(pid) => println!(
                            "ok: loop {} created — every {}, next in {}, model={}. \
                             计时器已启动 (pid {})。停止用 `fleet loop stop {}`。",
                            rec.id,
                            fmt_interval_secs(rec.interval_secs),
                            fmt_interval_secs(rec.interval_secs),
                            rec.model.as_deref().unwrap_or("<CLI 默认>"),
                            pid,
                            rec.id,
                        ),
                        Err(e) => println!(
                            "ok: loop {} created (every {}), 但计时器启动失败: {e}。\
                             Stop hook 的 reconcile 会在下次任意会话结束 turn 时补上。停止用 `fleet loop stop {}`。",
                            rec.id,
                            fmt_interval_secs(rec.interval_secs),
                            rec.id,
                        ),
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn now_ms_wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `300` → `5m`, `7200` → `2h`, mirroring the accepted interval spellings.
fn fmt_interval_secs(secs: u64) -> String {
    if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Human "time until" for the NEXT column, coarse on purpose.
fn fmt_duration_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}
