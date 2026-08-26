//! Reclaim long-lived background processes a finished session left behind.
//!
//! An agent's `Bash` tool routinely starts a server that outlives the command
//! that spawned it — `pnpm dev &`, `patchwright-cli -s=<name> open` (whose
//! browser is a *persistent* session by design). The turn ends, the session
//! ends, and the process keeps running: reparented to init, holding hundreds of
//! MB, invisible to everything. Nothing on the machine ever reclaims it.
//!
//! They are not free-floating, though. Every descendant of a Fleet-spawned
//! session inherits `CLAUDE_CODE_SESSION_ID` in its environment, so a leftover
//! can be attributed back to the session that started it even after the parent
//! chain is gone. That is what this module reaps against.
//!
//! # Why a whitelist and not "kill anything that session left"
//!
//! Attribution by inherited env is broader than it looks: a session that
//! launched Docker Desktop leaves `com.docker.backend` (ppid 1, alive for
//! days) stamped with that session's id. Measured on 2026-08-26, ten Docker
//! processes carried a single agent session's id. Killing "everything the dead
//! session left" would take the user's Docker down with it. So membership in
//! [`REAPABLE`] is required, and [`NEVER_REAP`] overrides it — a leak class is
//! opted *in*, never inferred.

use std::collections::HashSet;

/// Minimal description of a candidate process, lifted out of `sysinfo` so the
/// selection rule is a pure function and testable without spawning anything.
#[derive(Debug, Clone)]
pub struct ProcCandidate {
    pub pid: u32,
    pub ppid: u32,
    /// `CLAUDE_CODE_SESSION_ID` from the process environment, when readable.
    pub session_id: Option<String>,
    /// Full command line, space-joined.
    pub cmd: String,
}

/// Command-line signatures of the leak classes we reclaim. Each is a
/// long-lived server or browser an agent starts as a side effect of a turn and
/// never shuts down.
const REAPABLE: &[&str] = &[
    "/vite/bin/vite.js",
    "pnpm dev",
    "patchwright",
    "playwright",
];

/// Overrides [`REAPABLE`] unconditionally. These carry a session id purely
/// because a session happened to launch them; they are not that session's
/// scratch work and killing them is user-visible damage.
const NEVER_REAP: &[&str] = &[
    "Claw Fleet.app",
    "/Applications/Docker.app",
    "com.docker.",
    ".local/bin/claude",
];

/// Decide which candidates to reclaim.
///
/// A process is reaped only when **all** of these hold, so every widening of
/// the rule has to be deliberate:
///
/// 1. it carries a `CLAUDE_CODE_SESSION_ID` (it is a session's descendant);
/// 2. that session is no longer alive (a live session still owns its servers);
/// 3. `ppid == 1` — it has already outlived its parent chain, so nothing else
///    is in a position to wait on it;
/// 4. its command line matches [`REAPABLE`] and none of [`NEVER_REAP`].
pub fn select_reapable(candidates: &[ProcCandidate], live_sessions: &HashSet<String>) -> Vec<u32> {
    candidates
        .iter()
        .filter(|c| {
            let Some(sid) = c.session_id.as_deref() else {
                return false;
            };
            if live_sessions.contains(sid) {
                return false;
            }
            if c.ppid != 1 {
                return false;
            }
            if NEVER_REAP.iter().any(|n| c.cmd.contains(n)) {
                return false;
            }
            REAPABLE.iter().any(|r| c.cmd.contains(r))
        })
        .map(|c| c.pid)
        .collect()
}

/// Scan every process on the machine and reclaim the leftovers of sessions not
/// in `live_sessions`. Returns how many were killed.
///
/// `live_sessions` is the set of session ids still running; anything else is
/// treated as finished. Passing an empty set is therefore *not* a no-op — it
/// means "no session is alive", so give it the real list.
pub fn reap_orphaned_session_processes(live_sessions: &HashSet<String>) -> usize {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sys = System::new();
    // cmd + environ, deliberately no cwd: refreshing cwd for every process on
    // macOS triggers TCC consent dialogs for unrelated apps (same reason as
    // `dsh_server::sweep_unregistered_orphans` and `scan_codex_processes`).
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );

    let self_pid = std::process::id();
    let candidates: Vec<ProcCandidate> = sys
        .processes()
        .iter()
        .filter(|(pid, _)| pid.as_u32() != self_pid)
        .map(|(pid, p)| ProcCandidate {
            pid: pid.as_u32(),
            ppid: p.parent().map(|x| x.as_u32()).unwrap_or(0),
            session_id: p.environ().iter().find_map(|e| {
                e.to_string_lossy()
                    .strip_prefix("CLAUDE_CODE_SESSION_ID=")
                    .map(|v| v.to_string())
            }),
            cmd: p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect();

    let doomed = select_reapable(&candidates, live_sessions);
    for pid in &doomed {
        let cmd = candidates
            .iter()
            .find(|c| c.pid == *pid)
            .map(|c| c.cmd.as_str())
            .unwrap_or("");
        crate::log_debug(&format!(
            "orphan_reaper: killing leftover pid={pid} from a finished session ({})",
            &cmd[..cmd.len().min(120)]
        ));
        crate::llm_provider::kill_process(*pid);
    }
    doomed.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(pid: u32, ppid: u32, sid: Option<&str>, cmd: &str) -> ProcCandidate {
        ProcCandidate {
            pid,
            ppid,
            session_id: sid.map(|s| s.to_string()),
            cmd: cmd.to_string(),
        }
    }

    fn live(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reaps_a_dead_sessions_orphaned_vite_server() {
        let c = vec![cand(
            500,
            1,
            Some("dead-session"),
            "node /repo/node_modules/.bin/../vite/bin/vite.js --port 1430",
        )];
        assert_eq!(select_reapable(&c, &live(&["other"])), vec![500]);
    }

    #[test]
    fn reaps_an_orphaned_patchwright_browser() {
        let c = vec![cand(
            501,
            1,
            Some("dead-session"),
            "node /opt/homebrew/lib/node_modules/patchwright-cli/cli.js -s=uiold",
        )];
        assert_eq!(select_reapable(&c, &live(&[])), vec![501]);
    }

    #[test]
    fn leaves_a_live_sessions_server_alone() {
        let c = vec![cand(
            502,
            1,
            Some("live-session"),
            "node /repo/node_modules/.bin/../vite/bin/vite.js --port 1430",
        )];
        assert!(select_reapable(&c, &live(&["live-session"])).is_empty());
    }

    #[test]
    fn leaves_a_still_parented_process_alone() {
        // ppid != 1: something is still in a position to wait on it.
        let c = vec![cand(
            503,
            42_000,
            Some("dead-session"),
            "node /repo/node_modules/.bin/../vite/bin/vite.js --port 1430",
        )];
        assert!(select_reapable(&c, &live(&[])).is_empty());
    }

    #[test]
    fn leaves_a_process_without_a_session_id_alone() {
        // The user's own `pnpm dev`, started from their terminal.
        let c = vec![cand(504, 1, None, "node /repo/.bin/../vite/bin/vite.js")];
        assert!(select_reapable(&c, &live(&[])).is_empty());
    }

    /// The finding that forced the whitelist: Docker Desktop inherits the id of
    /// whichever session first launched it, sits at ppid 1, and outlives that
    /// session by days. A rule without [`NEVER_REAP`] takes the user's Docker
    /// down the moment that session ends.
    #[test]
    fn never_reaps_docker_even_though_it_carries_a_dead_sessions_id() {
        let c = vec![
            cand(
                505,
                1,
                Some("dead-session"),
                "/Applications/Docker.app/Contents/MacOS/com.docker.backend",
            ),
            cand(
                506,
                1,
                Some("dead-session"),
                "/Applications/Docker.app/Contents/MacOS/com.docker.backend services",
            ),
        ];
        assert!(select_reapable(&c, &live(&[])).is_empty());
    }

    #[test]
    fn never_reaps_fleets_own_helpers() {
        let c = vec![cand(
            507,
            1,
            Some("dead-session"),
            "/Applications/Claw Fleet.app/Contents/MacOS/fleet watch fire 9407e601 1",
        )];
        assert!(select_reapable(&c, &live(&[])).is_empty());
    }

    #[test]
    fn leaves_an_unrecognised_leftover_alone() {
        // Not in REAPABLE: a build that outlived its session is still not ours
        // to guess about. Opt-in only.
        let c = vec![cand(508, 1, Some("dead-session"), "zig build-exe -fllvm")];
        assert!(select_reapable(&c, &live(&[])).is_empty());
    }
}
