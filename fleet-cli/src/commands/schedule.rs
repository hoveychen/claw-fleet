//! `fleet schedule` — a one-shot prompt Fleet fires at an absolute future time by
//! spawning a fresh detached session, so it survives the turn boundary. Sibling
//! of `fleet loop`: a loop repeats on an interval, a schedule fires once at a
//! named time (`--at "2026-07-25 09:00"` or `--in 5d`) with no 7-day ceiling.

use crate::commands::session::read_fleet_session_id;
use crate::ScheduleCommands;

pub(crate) fn cmd_schedule(action: ScheduleCommands) {
    use claw_fleet_core::schedule;
    match action {
        ScheduleCommands::Fire { id, generation } => {
            // The detached timer body — blocks, sleeping until due, then fires once.
            schedule::run_timer_blocking(&id, generation);
        }
        ScheduleCommands::List { json } => {
            let items = schedule::list();
            if json {
                println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
                return;
            }
            if items.is_empty() {
                println!("no schedules registered");
                return;
            }
            let now = now_ms_wall();
            println!("{:<10}  {:<16}  {:<9}  {:<10}  PROMPT", "ID", "WHEN", "STATUS", "IN");
            for s in items {
                let status = match s.status {
                    schedule::ScheduleStatus::Pending => "pending",
                    schedule::ScheduleStatus::Fired => "fired",
                };
                let when_col = if s.is_pending() {
                    if s.is_due(now) {
                        "due".to_string()
                    } else {
                        fmt_duration_ms(s.due_in_ms(now))
                    }
                } else {
                    "—".to_string()
                };
                let prompt = s.prompt.replace('\n', " ");
                let prompt = if prompt.chars().count() > 44 {
                    format!("{}…", prompt.chars().take(44).collect::<String>())
                } else {
                    prompt
                };
                println!(
                    "{:<10}  {:<16}  {:<9}  {:<10}  {}",
                    s.id,
                    fmt_local(s.fire_at),
                    status,
                    when_col,
                    prompt
                );
            }
        }
        ScheduleCommands::Get { id, json } => match schedule::get(&id) {
            Some(rec) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&rec).unwrap_or_default());
                } else {
                    println!("id:        {}", rec.id);
                    println!("fire_at:   {}", fmt_local(rec.fire_at));
                    println!(
                        "status:    {}",
                        match rec.status {
                            schedule::ScheduleStatus::Pending => "pending",
                            schedule::ScheduleStatus::Fired => "fired",
                        }
                    );
                    if let Some(fa) = rec.fired_at {
                        println!("fired_at:  {}", fmt_local(fa));
                    }
                    if let Some(s) = &rec.fired_session_id {
                        println!("session:   {s}");
                    }
                    println!("workspace: {}", rec.workspace_path);
                    println!("model:     {}", rec.model.as_deref().unwrap_or("<CLI 默认>"));
                    println!("prompt:    {}", rec.prompt);
                }
            }
            None => {
                eprintln!("no schedule with id {id}");
                std::process::exit(1);
            }
        },
        ScheduleCommands::Update { id, at, r#in, prompt, model, effort } => {
            let now = now_ms_wall();
            let fire_at = match (at.as_deref(), r#in.as_deref()) {
                (Some(_), Some(_)) => {
                    eprintln!("Error: pass at most one of --at or --in, not both.");
                    std::process::exit(2);
                }
                (Some(a), None) => match schedule::parse_at(a, now) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(2);
                    }
                },
                (None, Some(i)) => match schedule::parse_in(i, now) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(2);
                    }
                },
                (None, None) => None,
            };
            if fire_at.is_none()
                && prompt.is_none()
                && model.is_none()
                && effort.is_none()
            {
                eprintln!("Error: nothing to update — pass --at/--in, --prompt, --model and/or --effort.");
                std::process::exit(2);
            }
            match schedule::update(&schedule::ScheduleUpdate {
                id: id.clone(),
                fire_at,
                prompt,
                model,
                effort,
                ..Default::default()
            }) {
                Ok(rec) => {
                    // Re-arm so the new time/generation takes effect; the old
                    // timer exits as superseded on its next wake.
                    let _ = schedule::arm_timer(&rec);
                    println!(
                        "ok: schedule {} updated — fires {} (in {}). 计时器已重挂。",
                        rec.id,
                        fmt_local(rec.fire_at),
                        fmt_duration_ms(rec.due_in_ms(now)),
                    );
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        ScheduleCommands::Cancel { id } => {
            if schedule::cancel(&id) {
                println!("ok: schedule {id} cancelled (its timer exits on next wake)");
            } else {
                eprintln!("no schedule with id {id}");
                std::process::exit(1);
            }
        }
        ScheduleCommands::Run { id } => match schedule::run_now(&id) {
            Ok(sid) => match sid {
                Some(s) => println!("ok: schedule {id} run now -> session {s} (schedule still fires at its scheduled time)"),
                None => println!("ok: schedule {id} run now (schedule still fires at its scheduled time)"),
            },
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        ScheduleCommands::Create { at, r#in, prompt, model: model_flag, effort: effort_flag } => {
            let now = now_ms_wall();
            // Exactly one of --at / --in.
            let fire_at = match (at.as_deref(), r#in.as_deref()) {
                (Some(_), Some(_)) => {
                    eprintln!("Error: pass exactly one of --at or --in, not both.");
                    std::process::exit(2);
                }
                (None, None) => {
                    eprintln!("Error: pass --at \"2026-07-25 09:00\" or --in 5d.");
                    std::process::exit(2);
                }
                (Some(a), None) => match schedule::parse_at(a, now) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(2);
                    }
                },
                (None, Some(i)) => match schedule::parse_in(i, now) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(2);
                    }
                },
            };
            let prompt = prompt.trim();
            if prompt.is_empty() {
                eprintln!("Error: --prompt is required.");
                std::process::exit(2);
            }
            // Inherit the creating session's workspace / model / effort / source,
            // exactly like `fleet loop` and handoff — so a schedule created from a
            // fable-5 codex session fires on fable-5 codex, in the session's real
            // cwd (not a worktree that may later be removed).
            let sid = read_fleet_session_id();
            let shell_cwd = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .to_string_lossy()
                .to_string();
            let ws = sid
                .as_deref()
                .and_then(claw_fleet_core::session::resolve_session_cwd)
                .unwrap_or_else(|| shell_cwd.clone());
            // An explicit --model/--effort flag overrides the value inherited from
            // the creating session (mirrors handoff's --model/--effort override).
            let model = model_flag
                .filter(|m| !m.trim().is_empty())
                .or_else(|| {
                    sid.as_deref()
                        .and_then(claw_fleet_core::session::resolve_session_model_spec)
                });
            let effort = effort_flag
                .filter(|e| !e.trim().is_empty())
                .or_else(|| std::env::var("CLAUDE_EFFORT").ok())
                .filter(|e| !e.trim().is_empty());
            let agent_source = std::env::var("FLEET_AGENT_SOURCE")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            match schedule::create(
                &ws,
                prompt,
                fire_at,
                model.as_deref(),
                effort.as_deref(),
                agent_source.as_deref(),
                sid.as_deref(),
            ) {
                Ok(rec) => match schedule::arm_timer(&rec) {
                    Ok(pid) => println!(
                        "ok: schedule {} created — fires {} (in {}), model={}. \
                         计时器已启动 (pid {})。取消用 `fleet schedule cancel {}`。",
                        rec.id,
                        fmt_local(rec.fire_at),
                        fmt_duration_ms(rec.due_in_ms(now)),
                        rec.model.as_deref().unwrap_or("<CLI 默认>"),
                        pid,
                        rec.id,
                    ),
                    Err(e) => println!(
                        "ok: schedule {} created (fires {}), 但计时器启动失败: {e}。\
                         Stop hook 的 reconcile 会在下次任意会话结束 turn 时补上。取消用 `fleet schedule cancel {}`。",
                        rec.id,
                        fmt_local(rec.fire_at),
                        rec.id,
                    ),
                },
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

/// Epoch ms → human-readable local time for display.
fn fmt_local(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(dt) => dt.format("%m-%d %H:%M").to_string(),
        _ => format!("{ms}ms"),
    }
}

/// Human "time until", coarse on purpose.
fn fmt_duration_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 86400 {
        format!("{}d{}h", s / 86400, (s % 86400) / 3600)
    } else if s >= 3600 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}
