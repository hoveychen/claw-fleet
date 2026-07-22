//! `fleet watch` — a one-shot condition wait. A detached timer polls a shell
//! condition and, when it succeeds, resumes THIS session (`claude --resume`) so
//! the next turn sees the result — the survivable replacement for `Monitor` /
//! background `Bash` / `ScheduleWakeup`, which all die with a headless `-p` turn.

use crate::commands::session::read_fleet_session_id;
use crate::WatchCommands;

pub(crate) fn cmd_watch(action: WatchCommands) {
    use claw_fleet_core::watch;
    match action {
        WatchCommands::Fire { id, generation } => {
            // The detached timer body — blocks, polling the condition until it
            // fires (or the deadline passes) then resumes the session.
            watch::run_timer_blocking(&id, generation);
        }
        WatchCommands::List { json } => {
            let watches = watch::list();
            if json {
                println!("{}", serde_json::to_string_pretty(&watches).unwrap_or_default());
                return;
            }
            if watches.is_empty() {
                println!("no watches registered");
                return;
            }
            let now = now_ms_wall();
            println!("{:<10}  {:<12}  {:<10}  UNTIL", "ID", "SESSION", "TIMEOUT");
            for w in watches {
                let left = w.deadline_at.saturating_sub(now);
                let timeout = if w.is_expired(now) {
                    "expired".to_string()
                } else {
                    fmt_duration_ms(left)
                };
                let sess = if w.session_id.chars().count() > 10 {
                    format!("{}…", w.session_id.chars().take(9).collect::<String>())
                } else {
                    w.session_id.clone()
                };
                let until = w.until_cmd.replace('\n', " ");
                let until = if until.chars().count() > 44 {
                    format!("{}…", until.chars().take(44).collect::<String>())
                } else {
                    until
                };
                println!("{:<10}  {:<12}  {:<10}  {}", w.id, sess, timeout, until);
            }
        }
        WatchCommands::Stop { id } => {
            if watch::stop(&id) {
                println!("ok: watch {id} stopped (its timer exits on next poll)");
            } else {
                eprintln!("no watch with id {id}");
                std::process::exit(1);
            }
        }
        WatchCommands::Create {
            until,
            capture,
            note,
            poll,
            timeout,
        } => create(until, capture, note, poll, timeout),
    }
}

fn create(
    until: String,
    capture: Option<String>,
    note: Option<String>,
    poll: Option<String>,
    timeout: Option<String>,
) {
    use claw_fleet_core::watch;

    let until = until.trim();
    if until.is_empty() {
        eprintln!("Error: --until is required.");
        std::process::exit(2);
    }
    let poll_secs = match poll.as_deref() {
        Some(s) => match watch::parse_poll(s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error: --poll: {e}");
                std::process::exit(2);
            }
        },
        None => watch::DEFAULT_POLL_SECS,
    };
    let timeout_secs = match timeout.as_deref() {
        Some(s) => match watch::parse_timeout(s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error: --timeout: {e}");
                std::process::exit(2);
            }
        },
        None => watch::DEFAULT_TIMEOUT_SECS,
    };

    // A watch resumes the session that registered it, so it MUST know which
    // session that is. No id ⇒ nothing to reanimate — refuse rather than register
    // a watch that could never deliver its event.
    let Some(sid) = read_fleet_session_id() else {
        eprintln!(
            "Error: cannot resolve this session's id (FLEET_SESSION_ID / \
             CLAUDE_CODE_SESSION_ID unset). `fleet watch` reanimates the calling \
             session, so it can only run inside one."
        );
        std::process::exit(2);
    };
    // Subagent transcripts (`agent-*`) can't be independently `--resume`d — they
    // are child turns of a parent session. A subagent is also awaited by its
    // parent, so it never hits the die-at-turn-end problem a watch solves.
    if sid.starts_with("agent-") {
        eprintln!(
            "Error: this looks like a subagent session ({sid}); subagents are \
             awaited by their parent and cannot be resumed. Register the watch \
             from the top-level session instead."
        );
        std::process::exit(2);
    }
    // IDE / interactive session guard: resuming a session that is open in an IDE
    // headlessly (`claude --resume -p`) spawns a SECOND process on the same
    // session id — the exact hazard auto-resume refuses. A full scan (one-time,
    // at registration) is the authoritative signal: `SessionInfo.ide_name` is
    // Some only for IDE-attached, non-Fleet-owned sessions — Fleet's own headless
    // spawns have it stripped (`strip_ide_name_from_fleet_spawns`). Allow on a
    // scan miss: a just-spawned session may not be scannable yet, and blocking a
    // legitimate headless session is worse than missing a rare IDE edge case.
    let sources = claw_fleet_core::agent_source::build_sources();
    let sessions = claw_fleet_core::session::scan_all_sources(&sources);
    if let Some(ide) = ide_block_reason(&sid, &sessions) {
        eprintln!(
            "Error: this session is attached to {ide}; resuming it headlessly \
             would spawn a duplicate process on the same session id. `fleet \
             watch` is for headless Fleet sessions — the interactive one stays \
             alive across turns and doesn't need it."
        );
        std::process::exit(2);
    }

    // Inherit the session's real cwd (not a worktree that may later be removed),
    // model, effort, and agent source — exactly like `fleet loop` / handoff, so a
    // fable-5 codex session resumes as fable-5 codex.
    let ctx = claw_fleet_core::session::inherit_launch_context(Some(&sid));

    match watch::create(
        &sid,
        &ctx.workspace,
        until,
        capture.as_deref(),
        note.as_deref(),
        poll_secs,
        timeout_secs,
        ctx.model.as_deref(),
        ctx.effort.as_deref(),
        ctx.source.as_deref(),
    ) {
        Ok(rec) => {
            // Arm the detached timer so the watch actually polls. A create that
            // can't arm still leaves the record for the Stop-hook reconcile to
            // pick up — so warn, don't fail.
            match watch::arm_timer(&rec) {
                Ok(pid) => println!(
                    "ok: watch {} created — polling every {}, times out in {}, \
                     resumes session {}. 计时器已启动 (pid {})。停止用 `fleet watch stop {}`。\n\
                     现在可以正常结束这个 turn：条件满足时 Fleet 会自动 resume 本会话，\
                     不要再注册 Monitor / 后台任务去等它。",
                    rec.id,
                    fmt_secs(rec.poll_secs),
                    fmt_secs(timeout_secs),
                    rec.session_id,
                    pid,
                    rec.id,
                ),
                Err(e) => println!(
                    "ok: watch {} created (polling every {}), 但计时器启动失败: {e}。\
                     Stop hook 的 reconcile 会在下次任意会话结束 turn 时补上。停止用 `fleet watch stop {}`。",
                    rec.id,
                    fmt_secs(rec.poll_secs),
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

fn now_ms_wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `300` → `5m`, `7200` → `2h`, mirroring the accepted duration spellings.
fn fmt_secs(secs: u64) -> String {
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

/// Human "time until" for the TIMEOUT column, coarse on purpose.
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

/// The IDE a session is attached to, if any — the reason to refuse registering a
/// watch on it. Pure over a scanned session list so the guard is testable without
/// a live scan. Returns `None` when the session isn't found (allow — never
/// false-refuse a headless session that hasn't been scanned yet) or when its
/// `ide_name` is `None` (a headless / Fleet-owned session, safe to resume).
fn ide_block_reason(sid: &str, sessions: &[claw_fleet_core::session::SessionInfo]) -> Option<String> {
    sessions
        .iter()
        .find(|s| s.id == sid)
        .and_then(|s| s.ide_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_fleet_core::session::SessionInfo;

    fn sess(id: &str, ide: Option<&str>) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            ide_name: ide.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn ide_attached_session_is_blocked() {
        let sessions = vec![sess("s1", Some("Visual Studio Code"))];
        assert_eq!(ide_block_reason("s1", &sessions).as_deref(), Some("Visual Studio Code"));
    }

    #[test]
    fn headless_session_is_allowed() {
        // ide_name None ⇒ headless / Fleet-owned (stripped) ⇒ resumable
        let sessions = vec![sess("s1", None)];
        assert_eq!(ide_block_reason("s1", &sessions), None);
    }

    #[test]
    fn a_scan_miss_allows_rather_than_false_refuses() {
        // the registering session isn't in the scan (too fresh) — must not block
        let sessions = vec![sess("other", Some("Cursor"))];
        assert_eq!(ide_block_reason("s1", &sessions), None);
    }
}
