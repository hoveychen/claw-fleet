//! `fleet watch` — a one-shot condition wait. A detached timer polls a shell
//! condition and, when it succeeds, resumes THIS session (`claude --resume`) so
//! the next turn sees the result — the survivable replacement for `Monitor` /
//! background `Bash` / `ScheduleWakeup`, which all die with a headless `-p` turn.

use crate::commands::session::read_fleet_session_id;
use crate::WatchCommands;

pub(crate) fn cmd_watch(action: WatchCommands, session: Option<&str>) {
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
                println!(
                    "{}",
                    serde_json::to_string_pretty(&watches).unwrap_or_default()
                );
                return;
            }
            if watches.is_empty() {
                println!("no watches registered");
                return;
            }
            let now = now_ms_wall();
            println!(
                "{:<10}  {:<12}  {:<10}  {:<16}  UNTIL",
                "ID", "SESSION", "TIMEOUT", "LAST POLL"
            );
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
                println!(
                    "{:<10}  {:<12}  {:<10}  {:<16}  {}",
                    w.id,
                    sess,
                    timeout,
                    poll_state(&w),
                    until
                );
                // The whole point of keeping stderr: a watch that has only ever
                // failed structurally is broken, not waiting, and the operator
                // should not have to read a debug log to find that out.
                if let Some(t) = w.expect_by {
                    println!(
                        "{:<10}  expect-by {}{}",
                        "",
                        watch::fmt_local(t),
                        if w.checked_in { "（已唤醒自查）" } else { "" }
                    );
                }
                if let Some(p) = &w.progress {
                    println!(
                        "{:<10}  progress {p}{}",
                        "",
                        w.progress_changed_at
                            .map(|t| format!("（{} 未变）", fmt_duration_ms(now.saturating_sub(t))))
                            .unwrap_or_default()
                    );
                }
                if w.structural_fail_streak > 0 {
                    println!(
                        "{:<10}  ⚠️ 连续 {} 次跑不起来（不是条件没满足）：{}",
                        "", w.structural_fail_streak, w.last_stderr
                    );
                }
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
            expect_by,
            progress,
        } => create(until, capture, note, poll, timeout, expect_by, progress, session),
    }
}

fn create(
    until: String,
    capture: Option<String>,
    note: Option<String>,
    poll: Option<String>,
    timeout: Option<String>,
    expect_by: Option<String>,
    progress: Option<String>,
    session: Option<&str>,
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
    let expect_by = match expect_by.as_deref() {
        Some(s) => match watch::parse_expect_by(s, now_ms_wall()) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Error: --expect-by: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };

    // A watch resumes the session that registered it, so it MUST know which
    // session that is. No id ⇒ nothing to reanimate — refuse rather than register
    // a watch that could never deliver its event.
    //
    // `--session` is the explicit channel for a harness with no per-session
    // environment: every dsh session runs inside one shared `dsh web`, so no
    // FLEET_SESSION_ID can be stamped per session and the agent must name its own
    // id (which its per-turn Fleet context tells it). An explicit id outranks the
    // env, which in that shell describes the *server*, not the session.
    let explicit = explicit_sid(session);
    let Some(sid) = explicit.clone().or_else(read_fleet_session_id) else {
        eprintln!(
            "Error: cannot resolve this session's id (FLEET_SESSION_ID / \
             CLAUDE_CODE_SESSION_ID unset, no --session given). `fleet watch` \
             reanimates the calling session, so it must know which one that is — \
             pass `--session <id>` if your harness has no per-session environment."
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
    let sources = claw_fleet_core::agent_source::build_sources();
    let sessions = claw_fleet_core::session::scan_all_sources(&sources);

    // An explicitly named id has to exist: a typo would register a watch whose
    // fire resumes nothing, and the failure would only surface hours later when
    // the condition fired. The env path stays permissive on a scan miss (a
    // just-spawned session may not be scannable yet) — there the id came from
    // the harness itself, not from a hand-typed flag.
    if explicit.is_some() && !sessions.iter().any(|s| s.id == sid) {
        eprintln!(
            "Error: no session with id {sid} was found on this machine, so a watch \
             registered for it could never be resumed. Check the id — a dsh session \
             is told its own id in its per-turn Fleet context."
        );
        std::process::exit(2);
    }

    // Inherit the session's real cwd (not a worktree that may later be removed),
    // model, effort, and agent source — exactly like `fleet loop` / handoff, so a
    // fable-5 codex session resumes as fable-5 codex. The roster overlay is what
    // makes this right for dsh: its source/cwd/model live in no env and no
    // transcript, only in the scanned session list (see
    // `inherit_launch_context_from_roster`).
    let ctx = claw_fleet_core::session::inherit_launch_context_from_roster(&sid, &sessions);

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
        expect_by,
        progress.as_deref(),
    ) {
        Ok((rec, probe)) => {
            // The preflight's verdict, when it has one to give (already true /
            // too slow to pre-judge). A structurally broken `until` never gets
            // here — `create` refuses it.
            let note = watch::preflight_note(&probe);
            if !note.is_empty() {
                println!("{note}");
            }
            let note = watch::expect_by_note(&rec);
            if !note.is_empty() {
                println!("{note}");
            }
            let note = watch::progress_note(&rec);
            if !note.is_empty() {
                println!("{note}");
            }
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
                     Fleet 桌面端或 `fleet serve` 在运行时会在 30 秒内自动补上。停止用 `fleet watch stop {}`。",
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

/// The LAST POLL column: how many times the condition has been checked and what
/// the most recent check said. `exit 1 ×204` is a healthy wait; `exit 127 ×204`
/// is a watch that will never fire, and the two used to render identically.
fn poll_state(w: &claw_fleet_core::watch::WatchRecord) -> String {
    match w.last_exit {
        Some(code) => format!("exit {code} ×{}", w.poll_count),
        // No exit code recorded: either nothing has been polled yet, or this is
        // a record written before the field existed. Both are "unknown" — do not
        // render them as a signal kill, which is a much rarer and scarier thing.
        None if w.poll_count == 0 => "—".to_string(),
        None => format!("exit ? ×{}", w.poll_count),
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

/// The `--session` value, normalised: a blank or whitespace-only flag is the
/// same as not passing one (so `--session "$SOME_UNSET_VAR"` falls back to the
/// env rather than registering a watch on an empty id).
fn explicit_sid(session: Option<&str>) -> Option<String> {
    session
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_session_flag_falls_back_to_the_env() {
        assert_eq!(explicit_sid(None), None);
        assert_eq!(explicit_sid(Some("   ")), None);
        assert_eq!(
            explicit_sid(Some(" dsh-uuid-1 ")).as_deref(),
            Some("dsh-uuid-1")
        );
    }

}
