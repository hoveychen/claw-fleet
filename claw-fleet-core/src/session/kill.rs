use super::*;

// ── Process kill helpers ─────────────────────────────────────────────────────

pub fn collect_process_tree(root_pid: u32) -> Vec<u32> {
    let output = match std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![root_pid],
    };
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut result = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        result.push(pid);
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                queue.push_back(kid);
            }
        }
    }
    result
}

/// Grace period before a SIGINT that nobody handled escalates to a tree kill.
const INTERRUPT_ESCALATION: Duration = Duration::from_millis(5000);

/// Gracefully interrupt the agent at `pid`: deliver SIGINT to the **root pid
/// only** and let the CLI decide how to unwind. Signalling the whole tree (as
/// [`kill_pid_impl`] does) would kill the tool child behind the CLI's back and
/// lose the transcript marker.
///
/// What SIGINT actually does depends on how the CLI was started — both verified
/// against `claude` 2.1.204 with a blocking foreground Bash call in flight:
///
/// * **headless `-p`** (what the launchpad spawns): aborts the tool call, kills
///   its own tool child, appends `[Request interrupted by user for tool use]`
///   and exits 0. `claude --resume <session-id>` then picks the conversation
///   back up. This is the case worth calling "interrupt".
/// * **interactive, attached to a pty** (what the user runs in a terminal): the
///   TUI reads Ctrl-C as a keystroke in raw mode, so a real SIGINT means "quit".
///   It exits 0 and **abandons its tool child**, reparented to init.
/// * **still booting**, before a handler is installed: killed outright.
///
/// Hence: sweep whatever the captured tree left behind once the root is gone,
/// and escalate to [`kill_pid_impl`] if the root ignored the signal entirely.
pub fn interrupt_pid_impl(pid: u32) -> Result<(), String> {
    interrupt_pid_with_grace(pid, INTERRUPT_ESCALATION)
}

/// [`interrupt_pid_impl`] with an explicit escalation delay. Split out so tests
/// don't have to wait five seconds to observe the fallback.
pub fn interrupt_pid_with_grace(pid: u32, grace: Duration) -> Result<(), String> {
    #[cfg(unix)]
    {
        // Capture the tree BEFORE signalling: once the root exits, its children
        // are reparented to init and walking down from `pid` finds nothing.
        let tree = collect_process_tree(pid);
        crate::log_debug(&format!(
            "interrupt_pid: SIGINT to root {pid} (captured tree of {})",
            tree.len()
        ));
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) } != 0 {
            return Err(format!("no such process: {pid}"));
        }

        std::thread::spawn(move || {
            std::thread::sleep(grace);

            if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
                crate::log_debug(&format!(
                    "interrupt_pid: {pid} still alive {grace:?} after SIGINT; escalating to tree kill"
                ));
                let _ = kill_pid_impl(pid);
                return;
            }

            // The root is gone. A headless CLI reaped its own children; an
            // interactive one abandoned them. Sweep the survivors.
            //
            // Every pid here was alive moments ago, so reuse inside this window
            // is unlikely — the same bet kill_pid_tree's delayed SIGKILL makes.
            let orphans: Vec<u32> = tree
                .iter()
                .copied()
                .filter(|&p| p != pid && unsafe { libc::kill(p as libc::pid_t, 0) } == 0)
                .collect();
            if orphans.is_empty() {
                return;
            }
            crate::log_debug(&format!(
                "interrupt_pid: root {pid} exited but orphaned {orphans:?}; sweeping"
            ));
            for &p in orphans.iter().rev() {
                unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
            }
            std::thread::sleep(Duration::from_millis(2000));
            for &p in orphans.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
            }
        });

        Ok(())
    }

    // Windows has no way to deliver SIGINT to an unrelated process (the console
    // control events only reach the sender's own console group), so there is no
    // graceful tier — fall through to the hard kill.
    #[cfg(not(unix))]
    {
        let _ = grace;
        kill_pid_impl(pid)
    }
}

/// Kill a process by PID (with process tree cleanup).
pub fn kill_pid_impl(pid: u32) -> Result<(), String> {
    kill_pid_tree(pid, false)
}

/// Build `taskkill` arguments to force-kill whole process trees. `None` when
/// there are no pids — the caller must NOT spawn a bare `taskkill /F /T /PID`,
/// which is a syntax error. Each pid needs its OWN `/PID`: `taskkill /F /T /PID
/// a b c` is rejected by taskkill, so the pre-fix multi-pid workspace kill
/// silently no-oped while still reporting success.
#[cfg(any(not(unix), test))]
fn build_taskkill_tree_args(pids: &[u32]) -> Option<Vec<String>> {
    if pids.is_empty() {
        return None;
    }
    let mut args = vec!["/F".to_string(), "/T".to_string()];
    for &p in pids {
        args.push("/PID".to_string());
        args.push(p.to_string());
    }
    Some(args)
}

#[cfg(test)]
mod taskkill_tests {
    use super::build_taskkill_tree_args;

    #[test]
    fn each_pid_gets_its_own_pid_flag_and_empty_is_none() {
        // Zero pids: caller must skip taskkill entirely, not emit `/F /T /PID`.
        assert!(
            build_taskkill_tree_args(&[]).is_none(),
            "empty pid set must yield None, not a bare `taskkill /F /T /PID`"
        );
        // Multiple pids: taskkill needs `/PID a /PID b`, never `/PID a b`.
        let args = build_taskkill_tree_args(&[100, 200]).expect("non-empty must be Some");
        let pid_flags = args.iter().filter(|a| a.as_str() == "/PID").count();
        assert_eq!(pid_flags, 2, "each pid needs its own /PID, got {args:?}");
        assert!(args.contains(&"/F".to_string()) && args.contains(&"/T".to_string()));
    }
}

/// Kill `pid` **and every descendant**. Signalling the root alone leaves the
/// agent's tool children — a build, a test run, a dev server — reparented to
/// init and still burning CPU after the agent itself is gone.
///
/// `force` sends SIGKILL straight away; otherwise SIGTERM now, and SIGKILL to
/// whatever is still standing 2s later.
pub fn kill_pid_tree(pid: u32, force: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        let pids = collect_process_tree(pid);
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        crate::log_debug(&format!(
            "kill_pid: {} to {} pids (root={}): {:?}",
            if force { "SIGKILL" } else { "SIGTERM" },
            pids.len(),
            pid,
            pids
        ));
        for &p in pids.iter().rev() {
            unsafe { libc::kill(p as libc::pid_t, signal) };
        }

        if !force {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2000));
                for &p in pids.iter().rev() {
                    if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                        unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                    }
                }
            });
        }

        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = force;
        crate::process_util::command("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map_err(|e| format!("taskkill failed: {e}"))?;
        Ok(())
    }
}

/// Kill all processes in a workspace.
pub fn kill_workspace_impl(workspace_path: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let procs = scan_cli_processes();
        let root_pids: Vec<u32> = procs
            .iter()
            .filter(|p| same_workspace_path(&p.cwd, workspace_path))
            .map(|p| p.pid)
            .collect();

        if root_pids.is_empty() {
            return Err(format!("No agent processes found in {}", workspace_path));
        }

        let mut all_pids: HashSet<u32> = HashSet::new();
        for &root in &root_pids {
            for pid in collect_process_tree(root) {
                all_pids.insert(pid);
            }
        }
        let pids: Vec<u32> = all_pids.into_iter().collect();

        crate::log_debug(&format!(
            "kill_workspace: SIGTERM to {} pids for workspace '{}': {:?}",
            pids.len(),
            workspace_path,
            pids
        ));

        for &p in pids.iter().rev() {
            unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
        }

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2000));
            for &p in pids.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
            }
        });

        Ok(())
    }

    #[cfg(not(unix))]
    {
        let pids: Vec<u32> = scan_cli_processes()
            .iter()
            .filter(|p| same_workspace_path(&p.cwd, workspace_path))
            .map(|p| p.pid)
            .collect();
        // Match the unix branch: an empty workspace is an explicit error, not a
        // bare `taskkill /F /T /PID` that spawn-succeeds, syntax-errors, and is
        // then reported as success. `build_taskkill_tree_args` also gives each
        // pid its own `/PID` (`/PID a b c` is rejected by taskkill).
        let Some(args) = build_taskkill_tree_args(&pids) else {
            return Err(format!("No agent processes found in {}", workspace_path));
        };
        let status = crate::process_util::command("taskkill")
            .args(&args)
            .status()
            .map_err(|e| format!("taskkill failed: {e}"))?;
        crate::log_debug(&format!(
            "kill_workspace: taskkill {:?} for '{}' -> {}",
            args, workspace_path, status
        ));
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod interrupt_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    /// `sleep` dies on SIGINT's default disposition — the graceful tier, no
    /// escalation. Spawned directly rather than via `sh -c 'sleep 30'`: dash
    /// (Linux `/bin/sh`) does NOT exec a single-command `-c` string, it forks
    /// `sleep` as a child, so a SIGINT aimed at the shell's own pid neither
    /// kills the shell nor reaches sleep — the interrupt would wrongly escalate
    /// to SIGTERM. (bash-as-/bin/sh on macOS execs, which is why it passed
    /// locally but failed on CI.) Spawning `sleep` directly makes the signalled
    /// pid the sleep process itself, with its default SIGINT disposition.
    #[test]
    fn interrupt_delivers_sigint_and_does_not_escalate() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(200));

        interrupt_pid_with_grace(child.id(), Duration::from_millis(300)).expect("interrupt");

        let status = child.wait().expect("wait");
        assert_eq!(
            status.signal(),
            Some(libc::SIGINT),
            "process must die from SIGINT, not from the escalation path"
        );
    }

    /// A process that ignores SIGINT must still go down, via the tree kill.
    #[test]
    fn interrupt_escalates_when_sigint_is_ignored() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' INT; sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(200));

        let started = std::time::Instant::now();
        interrupt_pid_with_grace(child.id(), Duration::from_millis(300)).expect("interrupt");

        // Still alive right after the SIGINT: it was ignored.
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "shell traps INT, so it must survive the graceful tier"
        );

        let status = child.wait().expect("wait");
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "escalation must SIGTERM the tree"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "escalation must wait out the full grace period"
        );
    }

    #[test]
    fn interrupt_reports_missing_process() {
        // Reap a child, then signal its (now free) pid.
        let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().expect("spawn");
        let pid = child.id();
        child.wait().expect("wait");
        std::thread::sleep(Duration::from_millis(100));
        assert!(interrupt_pid_with_grace(pid, Duration::from_millis(50)).is_err());
    }
}

#[cfg(all(test, unix))]
mod interrupt_orphan_tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn alive(pattern: &str) -> bool {
        Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// An interactive `claude` exits on SIGINT (rc 0) and leaves its tool child
    /// reparented to init — verified against claude 2.1.204 on a pty. Only the
    /// headless `-p` sessions reap their own children. So interrupt must sweep
    /// the tree it captured up front, not just escalate when the root survives.
    #[test]
    fn interrupt_reaps_orphans_when_the_root_exits() {
        let marker = "sleep 5051";
        // Dies on SIGINT and abandons its background child — the interactive shape.
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5051 & trap 'exit 0' INT; wait"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(marker), "precondition: the tool child must be running");

        interrupt_pid_with_grace(child.id(), Duration::from_millis(300)).expect("interrupt");
        let status = child.wait().expect("wait");
        assert!(status.success(), "root should exit cleanly on SIGINT");

        // Grace window + the sweep's own SIGTERM->SIGKILL delay.
        std::thread::sleep(Duration::from_millis(1200));
        let leaked = alive(marker);
        Command::new("pkill").args(["-9", "-f", marker]).output().ok();

        assert!(!leaked, "interrupt orphaned the tool child after the root exited");
    }
}
