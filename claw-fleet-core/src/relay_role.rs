//! Which local process gets to be *the* relay agent.
//!
//! The relay hands every client frame to **all** agents in the channel
//! (`fleet-relay/src/registry.rs::deliver_or_queue` loops over `ch.agents`),
//! and each agent runs the frame's handler for real. So two Fleet processes on
//! one machine sharing one pairing secret means every phone-side *write* runs
//! twice.
//!
//! Observed 2026-08-27: the desktop app and a hand-started
//! `fleet-cli webui --port 0` were both in the channel. One tap of "send" on
//! the phone produced two `resume_session` handlers 14ms apart, two
//! `claude --resume` processes (pid 28291 / 28296) on the same transcript, two
//! identical `user` rows under one `parentUuid`, and two assistant branches
//! answering in parallel. The phone rendered the duplicate the user reported.
//!
//! The fix is a machine-local single-holder lock, `~/.fleet/relay-agent.json`.
//! It is deliberately *not* config: a probe box or the Fleet Cloud container
//! runs `fleet serve` as its only agent and must keep joining unchanged, so the
//! gate has to answer "is another live Fleet process already the agent here?"
//! by itself.
//!
//! Priority: the desktop app outranks a headless `serve` / `webui`. The desktop
//! is the one with the decision panel, the parked-card store and the session
//! spawner the phone actually wants to reach, and it is the process the user is
//! looking at — so it takes the role over from a headless holder, and the
//! headless holder notices (via [`is_preempted`]) and drops its socket.

use serde::{Deserialize, Serialize};

use crate::session::{is_process_alive, process_start_time};

/// What kind of Fleet process is asking to be the relay agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// The desktop app (`claw-fleet-desktop`).
    Desktop,
    /// A headless host: `fleet serve` / `fleet webui`.
    Headless,
}

/// The process currently registered as this machine's relay agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub pid: u32,
    /// Snapshotted so a recycled pid can't masquerade as the holder — same
    /// defence as [`crate::session::HolderEntry`].
    #[serde(default)]
    pub start_time_secs: u64,
    pub kind: AgentKind,
}

impl Holder {
    pub fn capture(pid: u32, kind: AgentKind) -> Self {
        Self {
            pid,
            start_time_secs: process_start_time(pid).unwrap_or(0),
            kind,
        }
    }
}

/// Is `holder` a process that is still running, and still the same process?
///
/// `start_time_secs == 0` means "unknown" (legacy file, or the pid was already
/// gone at capture time) — treated as dead so a stale record can never wedge
/// the role.
pub fn holder_alive(holder: &Holder) -> bool {
    holder.start_time_secs != 0
        && is_process_alive(holder.pid)
        && process_start_time(holder.pid) == Some(holder.start_time_secs)
}

/// May a process of kind `me` (pid `my_pid`) join the relay channel, given the
/// currently recorded `holder` and whether that holder is still alive?
///
/// Pure so the whole priority table is testable without touching the process
/// table or the filesystem.
pub fn may_join(holder: Option<&Holder>, holder_alive: bool, my_pid: u32, me: AgentKind) -> bool {
    match holder {
        // Nobody registered, or the registered holder is gone — role is free.
        None => true,
        Some(_) if !holder_alive => true,
        // Re-entrant: our own record (we restarted our socket, not the process).
        Some(h) if h.pid == my_pid => true,
        // The desktop outranks a headless host and takes the role over.
        Some(h) => me == AgentKind::Desktop && h.kind == AgentKind::Headless,
    }
}

// ── The on-disk role record ──────────────────────────────────────────────────

/// `~/.fleet/relay-agent.json`, or `None` when there is no home to write into.
fn lock_path() -> Option<std::path::PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("relay-agent.json"))
}

/// The registered holder, or `None` when nothing (readable) is registered. A
/// corrupt file is treated as no holder — [`crate::atomic_json::load_preserving`]
/// renames the bytes aside first, so nothing is lost by continuing.
pub fn read_holder() -> Option<Holder> {
    let path = lock_path()?;
    match crate::atomic_json::load_preserving::<Holder>(&path) {
        crate::atomic_json::JsonLoad::Loaded(h) => Some(h),
        _ => None,
    }
}

/// Try to become this machine's relay agent. `true` = go ahead and join.
///
/// Read-modify-write under a cross-process file lock, so two processes starting
/// at the same moment can't both come away believing they hold the role.
pub fn claim(kind: AgentKind) -> bool {
    let Some(path) = lock_path() else {
        // No home to arbitrate with — never block the only agent a machine has.
        return true;
    };
    let my_pid = std::process::id();
    crate::atomic_json::with_file_lock(&path, || {
        let holder = read_holder();
        let alive = holder.as_ref().map(holder_alive).unwrap_or(false);
        if !may_join(holder.as_ref(), alive, my_pid, kind) {
            return false;
        }
        let me = Holder::capture(my_pid, kind);
        match serde_json::to_vec_pretty(&me) {
            Ok(bytes) => {
                if let Err(e) = crate::atomic_json::write_atomic(&path, &bytes) {
                    crate::log_debug(&format!("[relay-role] write {}: {e}", path.display()));
                }
            }
            Err(e) => crate::log_debug(&format!("[relay-role] serialize holder: {e}")),
        }
        true
    })
}

/// Has someone else taken the role over since we claimed it? Polled by a
/// connected agent so a headless host drops its socket the moment the desktop
/// app starts and claims the role, instead of doubling every write until it
/// happens to reconnect.
pub fn is_preempted() -> bool {
    match read_holder() {
        Some(h) => h.pid != std::process::id(),
        // A missing record is not preemption: nobody else claimed it, and the
        // re-claim on the next connect will write ours back.
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(pid: u32, kind: AgentKind) -> Holder {
        Holder { pid, start_time_secs: 42, kind }
    }

    #[test]
    fn free_role_lets_anyone_join() {
        assert!(may_join(None, false, 100, AgentKind::Desktop));
        assert!(may_join(None, false, 100, AgentKind::Headless));
    }

    #[test]
    fn headless_stays_out_while_a_live_desktop_holds_the_role() {
        // The 2026-08-27 duplicate-message bug: `fleet webui` joined the channel
        // next to the running desktop app, so every phone write ran twice.
        let h = holder(45057, AgentKind::Desktop);
        assert!(!may_join(Some(&h), true, 10903, AgentKind::Headless));
    }

    #[test]
    fn headless_stays_out_while_another_live_headless_holds_the_role() {
        let h = holder(10903, AgentKind::Headless);
        assert!(!may_join(Some(&h), true, 20000, AgentKind::Headless));
    }

    #[test]
    fn desktop_preempts_a_live_headless_holder() {
        let h = holder(10903, AgentKind::Headless);
        assert!(may_join(Some(&h), true, 45057, AgentKind::Desktop));
    }

    #[test]
    fn desktop_does_not_preempt_another_live_desktop() {
        let h = holder(45057, AgentKind::Desktop);
        assert!(!may_join(Some(&h), true, 46000, AgentKind::Desktop));
    }

    #[test]
    fn dead_holder_frees_the_role() {
        let h = holder(45057, AgentKind::Desktop);
        assert!(may_join(Some(&h), false, 10903, AgentKind::Headless));
    }

    #[test]
    fn own_record_is_reentrant() {
        let h = holder(10903, AgentKind::Headless);
        assert!(may_join(Some(&h), true, 10903, AgentKind::Headless));
    }

    #[test]
    fn holder_alive_rejects_unknown_start_time() {
        // start_time 0 = legacy/unknown: must never count as a live holder.
        let h = Holder { pid: std::process::id(), start_time_secs: 0, kind: AgentKind::Desktop };
        assert!(!holder_alive(&h));
    }

    #[test]
    fn holder_alive_accepts_self() {
        let h = Holder::capture(std::process::id(), AgentKind::Desktop);
        assert!(holder_alive(&h));
    }

    #[test]
    fn holder_alive_rejects_start_time_mismatch() {
        let mut h = Holder::capture(std::process::id(), AgentKind::Desktop);
        h.start_time_secs += 1; // pid recycled: same pid, different process
        assert!(!holder_alive(&h));
    }

    // ── Lock file ───────────────────────────────────────────────────────────

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-relay-role-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };
        f();
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn write_holder(h: &Holder) {
        let path = lock_path().unwrap();
        crate::atomic_json::write_atomic(&path, &serde_json::to_vec(h).unwrap()).unwrap();
    }

    #[test]
    fn claim_on_a_free_machine_records_us() {
        with_temp_home(|| {
            assert!(claim(AgentKind::Headless));
            let h = read_holder().expect("holder written");
            assert_eq!(h.pid, std::process::id());
            assert_eq!(h.kind, AgentKind::Headless);
        });
    }

    #[test]
    fn claim_is_refused_while_a_live_desktop_holds_the_role() {
        with_temp_home(|| {
            // A live desktop holder: this test process itself, tagged Desktop.
            write_holder(&Holder::capture(std::process::id(), AgentKind::Desktop));
            // Pretend to be a *different* headless process by re-checking the
            // pure gate with a foreign pid — the file path can only ever see our
            // own pid, so the cross-process case is asserted on `may_join`.
            let h = read_holder().unwrap();
            assert!(holder_alive(&h));
            assert!(!may_join(Some(&h), true, std::process::id() + 1, AgentKind::Headless));
        });
    }

    #[test]
    fn claim_takes_over_from_a_dead_holder() {
        with_temp_home(|| {
            // pid 0 never resolves → holder_alive false → role is free.
            write_holder(&Holder { pid: 0, start_time_secs: 12345, kind: AgentKind::Desktop });
            assert!(claim(AgentKind::Headless));
            assert_eq!(read_holder().unwrap().pid, std::process::id());
        });
    }

    #[test]
    fn preemption_is_someone_elses_pid_in_the_record() {
        with_temp_home(|| {
            assert!(claim(AgentKind::Headless));
            assert!(!is_preempted(), "our own record is not preemption");
            write_holder(&Holder::capture(std::process::id() + 1, AgentKind::Desktop));
            assert!(is_preempted());
        });
    }

    #[test]
    fn missing_record_is_not_preemption() {
        with_temp_home(|| {
            assert!(!is_preempted());
        });
    }
}
