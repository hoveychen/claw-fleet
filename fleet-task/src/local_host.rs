//! Binary-local `TaskLifecycleHost`. Spawns `claude` subprocesses directly and
//! keeps their pids in an in-memory map — does NOT write to
//! `~/.fleet/fleet-sessions.json`.
//!
//! Pairs with `claw_fleet_core::lifecycle_host::SupervisorHost` (the legacy
//! Phase 1 host that goes through the shared session table). Each `fleet-task`
//! process owns exactly one task, so pause / resume / terminate just signal
//! every pid currently tracked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use claw_fleet_task::auditor::AuditorSpawnSpec;
use claw_fleet_task::master::MasterSpawnSpec;
use claw_fleet_task::runner::{LlmMediator, Resolution, TaskLifecycleHost};
use claw_fleet_task::task::Task;
use claw_fleet_task::worker::WorkerSpawnSpec;
use claw_fleet_task::worktree::ConflictSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Master,
    Worker,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub pid: u32,
    pub kind: SessionKind,
    pub p_item_id: Option<String>,
}

/// Trait for actually launching a Claude (or test-fake) subprocess. Lets us
/// unit-test `LocalHost` without depending on the real `claude` CLI.
pub trait ProcessLauncher: Send + Sync {
    fn launch_master(&self, session_id: &str, spec: &MasterSpawnSpec) -> Result<u32, String>;
    fn launch_worker(&self, session_id: &str, spec: &WorkerSpawnSpec) -> Result<u32, String>;
    /// [WA1] Spawn ONE independent adversarial Auditor session (methodology
    /// pillar 4). The Auditor is *not* a worker: it edits no code, it reads the
    /// Master decision log + the P-item's worker output/diff and emits a
    /// structured `AuditFinding[]` JSON. Spawned with the compile-time-embedded
    /// `AUDITOR_SYSTEM_PROMPT` carried on the spec (so its constitution cannot
    /// be swapped at runtime) and on `AUDITOR_MODEL` (Opus-tier — an auditor
    /// weaker than the Master it polices would be easy to fool).
    ///
    /// `output_path` is where this session must write its findings JSON; the
    /// audit orchestrator (`actions::audit_pitem`) polls that file. The launcher
    /// passes it both as an env var and in the spawn prompt so the session knows
    /// exactly where to deposit its report.
    ///
    /// NOTE: the live driver for WA2 is the `fleet task audit` CLI subprocess
    /// (which the master shells out to per WA3's template) using its own
    /// `CliAuditorLauncher`. This in-fleet-task method exposes the same
    /// capability through `LocalHost::audit_pitem` for a future in-process /
    /// HTTP-route driver; today it is reached only through that method + tests,
    /// hence the allow on the provided default-less trait method.
    #[allow(dead_code)]
    fn launch_auditor(
        &self,
        session_id: &str,
        spec: &AuditorSpawnSpec,
        output_path: &Path,
    ) -> Result<u32, String>;
}

/// Default launcher that shells out to the `claude` CLI on PATH. Mirrors the
/// flags `claw_fleet_core::supervisor::spawn_claude` uses so behaviour is
/// consistent across the two hosts.
pub struct ClaudeLauncher;

impl ProcessLauncher for ClaudeLauncher {
    fn launch_master(&self, session_id: &str, spec: &MasterSpawnSpec) -> Result<u32, String> {
        let prompt = format!(
            "Task `{}` is starting. Inspect the plan, pick the first \
             dispatchable P-item, and proceed per the Acceptance Audit Protocol.",
            spec.task_id
        );
        spawn_claude(
            session_id,
            &spec.cwd,
            &prompt,
            Some(&spec.system_prompt),
            Some(spec.model),
            "master",
            Some(&spec.task_id),
            None,
            &[],
        )
    }

    fn launch_worker(&self, session_id: &str, spec: &WorkerSpawnSpec) -> Result<u32, String> {
        let prompt = format!(
            "Execute P-item `{}` per the SYSTEM prompt above. When you are done, \
             stop the process; the master will run the acceptance audit.",
            spec.p_item_id
        );
        spawn_claude(
            session_id,
            &spec.cwd,
            &prompt,
            Some(&spec.system_prompt),
            // [REQ-016] model is now an owned String on WorkerSpawnSpec; borrow
            // it (&String deref-coerces to &str) like the other spawn_claude args.
            Some(&spec.model),
            "worker",
            Some(&spec.task_id),
            Some(&spec.p_item_id),
            &[],
        )
    }

    fn launch_auditor(
        &self,
        session_id: &str,
        spec: &AuditorSpawnSpec,
        output_path: &Path,
    ) -> Result<u32, String> {
        // [WA1] The prompt tells the auditor what to audit (the Master decision
        // log on the spec) and *exactly where* to write its findings JSON. The
        // output path is also passed as an env var (`FLEET_AUDITOR_OUTPUT`) so a
        // session that loses the prompt thread can still find its drop location.
        let prompt = format!(
            "You are independent Auditor session #{idx} of a 3-session quorum for \
             task `{task}`. Read the Master decision log at `{log}` (and any P-item \
             worker output/diff it references), apply the red lines and weak-impl \
             heuristics from your SYSTEM prompt, and write your `AuditFinding[]` JSON \
             array to the file `{out}`. Write `[]` if you find nothing. Do NOT edit \
             any code; you only observe and report. Stop when the file is written.",
            idx = spec.session_index,
            task = spec.task_id,
            log = spec.master_log_path,
            out = output_path.display(),
        );
        // The Auditor runs read-only against the Master log; cwd doesn't matter
        // for its job, so we anchor it at the log's parent (the ~/.fleet dir).
        let cwd = Path::new(&spec.master_log_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::temp_dir());
        spawn_claude(
            session_id,
            &cwd,
            &prompt,
            // The embedded auditor constitution travels on the spec.
            Some(&spec.system_prompt),
            Some(&spec.model),
            "auditor",
            Some(&spec.task_id),
            None,
            &[("FLEET_AUDITOR_OUTPUT", output_path.to_string_lossy().as_ref())],
        )
    }
}

/// Env override used by the binary integration test: when
/// `FLEET_TASK_FAKE_LAUNCHER=1`, ClaudeLauncher spawns `sleep 60` instead of
/// the real `claude` CLI so the test doesn't need claude installed.
const FAKE_LAUNCHER_ENV: &str = "FLEET_TASK_FAKE_LAUNCHER";

#[allow(clippy::too_many_arguments)]
fn spawn_claude(
    session_id: &str,
    cwd: &Path,
    prompt: &str,
    system_prompt: Option<&str>,
    model: Option<&str>,
    kind_env: &str,
    task_id: Option<&str>,
    p_item_id: Option<&str>,
    // [WA1] Extra (key,value) env pairs set on the child — used to pass the
    // auditor its output-path drop location without widening every call site.
    extra_env: &[(&str, &str)],
) -> Result<u32, String> {
    if std::env::var_os(FAKE_LAUNCHER_ENV).is_some() {
        let child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn fake (sleep): {e}"))?;
        return Ok(child.id());
    }
    let mut cmd = claw_fleet_task::process_util::command("claude");
    cmd.current_dir(cwd)
        .arg("--print")
        .arg("--session-id")
        .arg(session_id);
    if let Some(sp) = system_prompt {
        cmd.arg("--append-system-prompt").arg(sp);
    }
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    cmd.arg(prompt)
        .env("FLEET_SESSION_ID", session_id)
        .env("FLEET_SESSION_KIND", kind_env);
    if let Some(t) = task_id {
        cmd.env("FLEET_TASK_ID", t);
    }
    if let Some(p) = p_item_id {
        cmd.env("FLEET_P_ITEM_ID", p);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = cmd.spawn().map_err(|e| format!("spawn claude: {e}"))?;
    Ok(child.id())
}

pub struct LocalHost {
    workspace: PathBuf,
    sessions: Mutex<HashMap<String, SessionRecord>>,
    launcher: Box<dyn ProcessLauncher>,
}

impl LocalHost {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            sessions: Mutex::new(HashMap::new()),
            launcher: Box::new(ClaudeLauncher),
        }
    }

    pub fn with_launcher(workspace: PathBuf, launcher: Box<dyn ProcessLauncher>) -> Self {
        Self {
            workspace,
            sessions: Mutex::new(HashMap::new()),
            launcher,
        }
    }

    pub fn live_sessions(&self) -> Vec<SessionRecord> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }

    /// [WA1+WA2] Run the live adversarial audit for a P-item in-process, driving
    /// our `ProcessLauncher::launch_auditor` through `actions::audit_pitem`. This
    /// is the in-fleet-task counterpart to the `fleet task audit` CLI command:
    /// it lets the master (or the runtime) trigger the 3-session quorum audit
    /// using the same launcher that spawns workers, so SIGSTOP/CONT/TERM and pid
    /// tracking stay consistent. `timeout` is the auditor hard deadline (use
    /// `actions::AUDITOR_HARD_TIMEOUT` for production).
    #[allow(dead_code)]
    pub fn audit_pitem(
        &self,
        task_id: &str,
        p_item_id: &str,
        timeout: std::time::Duration,
    ) -> Result<claw_fleet_task::actions::AuditOutcome, String> {
        // Adapt our `ProcessLauncher` (master/worker/auditor) to the narrower
        // `AuditorLauncher` the orchestrator expects.
        struct Adapter<'a>(&'a dyn ProcessLauncher);
        impl claw_fleet_task::actions::AuditorLauncher for Adapter<'_> {
            fn launch_auditor(
                &self,
                session_index: usize,
                spec: &AuditorSpawnSpec,
                output_path: &Path,
            ) -> Result<u32, String> {
                // Derive a stable session id per quorum member.
                let session_id = format!("auditor-{}-{}", spec.task_id, session_index);
                self.0.launch_auditor(&session_id, spec, output_path)
            }
        }
        let adapter = Adapter(self.launcher.as_ref());
        claw_fleet_task::actions::audit_pitem(task_id, p_item_id, &adapter, timeout)
    }

    /// Drop a tracked session — call after observing the subprocess exit.
    pub fn forget(&self, session_id: &str) -> Option<SessionRecord> {
        self.sessions.lock().unwrap().remove(session_id)
    }

    #[cfg(unix)]
    fn signal_all(&self, sig: libc::c_int) -> usize {
        let sessions = self.sessions.lock().unwrap();
        let mut count = 0;
        for rec in sessions.values() {
            unsafe {
                if libc::kill(rec.pid as libc::pid_t, sig) == 0 {
                    count += 1;
                }
            }
        }
        count
    }

    #[cfg(not(unix))]
    fn signal_all(&self, _sig: i32) -> usize {
        0
    }
}

impl TaskLifecycleHost for LocalHost {
    fn workspace_for_task(&self, _task: &Task) -> Result<PathBuf, String> {
        Ok(self.workspace.clone())
    }

    fn enqueue_master(&self, spec: &MasterSpawnSpec) -> Result<String, String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let pid = self.launcher.launch_master(&session_id, spec)?;
        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionRecord {
                session_id: session_id.clone(),
                pid,
                kind: SessionKind::Master,
                p_item_id: None,
            },
        );
        Ok(session_id)
    }

    fn enqueue_worker(&self, spec: &WorkerSpawnSpec) -> Result<String, String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let pid = self.launcher.launch_worker(&session_id, spec)?;
        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionRecord {
                session_id: session_id.clone(),
                pid,
                kind: SessionKind::Worker,
                p_item_id: Some(spec.p_item_id.clone()),
            },
        );
        Ok(session_id)
    }

    fn pause_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        #[cfg(unix)]
        {
            Ok(self.signal_all(libc::SIGSTOP))
        }
        #[cfg(not(unix))]
        {
            Ok(0)
        }
    }

    fn resume_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        #[cfg(unix)]
        {
            Ok(self.signal_all(libc::SIGCONT))
        }
        #[cfg(not(unix))]
        {
            Ok(0)
        }
    }

    fn terminate_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        #[cfg(unix)]
        {
            Ok(self.signal_all(libc::SIGTERM))
        }
        #[cfg(not(unix))]
        {
            Ok(0)
        }
    }
}

impl LlmMediator for LocalHost {
    fn resolve_conflicts(&self, _files: &[ConflictSpec]) -> Result<Vec<Resolution>, String> {
        // Phase 2 stub: fleet-task's LocalHost doesn't yet have an LLM
        // provider wired up. mark_done's call site bubbles this error back
        // to the master agent, which can retry or escalate via the human
        // gate. Phase 3 plumbs a real provider through here.
        Err("LLM merge mediation not configured for fleet-task (Phase 3)".into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Test launcher that spawns a long-running `sleep` instead of `claude`.
    /// Real signals against real pids prove the SIGSTOP / SIGTERM plumbing.
    struct SleepLauncher {
        master_calls: AtomicU32,
        worker_calls: AtomicU32,
        auditor_calls: AtomicU32,
        children: Mutex<Vec<std::process::Child>>,
    }

    impl SleepLauncher {
        fn new() -> Self {
            Self {
                master_calls: AtomicU32::new(0),
                worker_calls: AtomicU32::new(0),
                auditor_calls: AtomicU32::new(0),
                children: Mutex::new(Vec::new()),
            }
        }

        fn spawn_sleeper(&self) -> Result<u32, String> {
            let child = Command::new("sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("sleep: {e}"))?;
            let pid = child.id();
            self.children.lock().unwrap().push(child);
            Ok(pid)
        }

        /// Poll `try_wait` on every tracked child for up to `timeout_ms` ms.
        /// Returns the number that exited (vs. still running). Use this in
        /// tests to verify SIGTERM actually killed the subprocesses — naïve
        /// `kill(pid, 0)` probing reports zombies as alive.
        fn wait_all_exited(&self, timeout_ms: u64) -> usize {
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(timeout_ms);
            let mut exited = 0usize;
            let mut children = self.children.lock().unwrap();
            for child in children.iter_mut() {
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            exited += 1;
                            break;
                        }
                        Ok(None) => {
                            if std::time::Instant::now() >= deadline {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            }
            exited
        }
    }

    impl ProcessLauncher for SleepLauncher {
        fn launch_master(
            &self,
            _session_id: &str,
            _spec: &MasterSpawnSpec,
        ) -> Result<u32, String> {
            self.master_calls.fetch_add(1, Ordering::SeqCst);
            self.spawn_sleeper()
        }
        fn launch_worker(
            &self,
            _session_id: &str,
            _spec: &WorkerSpawnSpec,
        ) -> Result<u32, String> {
            self.worker_calls.fetch_add(1, Ordering::SeqCst);
            self.spawn_sleeper()
        }
        fn launch_auditor(
            &self,
            _session_id: &str,
            _spec: &AuditorSpawnSpec,
            _output_path: &Path,
        ) -> Result<u32, String> {
            self.auditor_calls.fetch_add(1, Ordering::SeqCst);
            self.spawn_sleeper()
        }
    }

    impl Drop for SleepLauncher {
        fn drop(&mut self) {
            for c in self.children.lock().unwrap().iter_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    fn make_master_spec(task_id: &str, cwd: &Path) -> MasterSpawnSpec {
        MasterSpawnSpec {
            task_id: task_id.into(),
            cwd: cwd.to_path_buf(),
            system_prompt: "sys".into(),
            model: "claude-sonnet-4-6",
        }
    }

    fn make_worker_spec(task_id: &str, p_id: &str, cwd: &Path) -> WorkerSpawnSpec {
        WorkerSpawnSpec {
            task_id: task_id.into(),
            p_item_id: p_id.into(),
            cwd: cwd.to_path_buf(),
            system_prompt: "sys".into(),
            // [REQ-016] WorkerSpawnSpec.model is an owned String now.
            model: "claude-sonnet-4-6".to_string(),
        }
    }

    fn pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[test]
    fn enqueue_master_records_pid() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launcher = Arc::new(SleepLauncher::new());
        let host = LocalHost::with_launcher(
            tmp.path().to_path_buf(),
            Box::new(SleepLauncherRef(launcher.clone())),
        );
        let spec = make_master_spec("t1", tmp.path());
        let sid = host.enqueue_master(&spec).unwrap();
        let live = host.live_sessions();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].session_id, sid);
        assert_eq!(live[0].kind, SessionKind::Master);
        assert!(pid_alive(live[0].pid));
        // cleanup
        host.terminate_task_sessions("t1").unwrap();
    }

    #[test]
    fn enqueue_worker_records_pid_and_p_item() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launcher = Arc::new(SleepLauncher::new());
        let host = LocalHost::with_launcher(
            tmp.path().to_path_buf(),
            Box::new(SleepLauncherRef(launcher.clone())),
        );
        let spec = make_worker_spec("t1", "p1", tmp.path());
        let sid = host.enqueue_worker(&spec).unwrap();
        let live = host.live_sessions();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].session_id, sid);
        assert_eq!(live[0].kind, SessionKind::Worker);
        assert_eq!(live[0].p_item_id.as_deref(), Some("p1"));
        host.terminate_task_sessions("t1").unwrap();
    }

    #[test]
    fn terminate_sends_sigterm_to_all_sessions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launcher = Arc::new(SleepLauncher::new());
        let host = LocalHost::with_launcher(
            tmp.path().to_path_buf(),
            Box::new(SleepLauncherRef(launcher.clone())),
        );
        let _ = host.enqueue_master(&make_master_spec("t1", tmp.path())).unwrap();
        let _ = host.enqueue_worker(&make_worker_spec("t1", "p1", tmp.path())).unwrap();
        let pids: Vec<u32> = host.live_sessions().iter().map(|r| r.pid).collect();
        assert_eq!(pids.len(), 2);

        let n = host.terminate_task_sessions("t1").unwrap();
        assert_eq!(n, 2);

        // try_wait avoids the zombie-pid false positive that `kill(pid, 0)` hits.
        let exited = launcher.wait_all_exited(2_000);
        assert_eq!(exited, 2, "expected both sleeps to exit after SIGTERM");
        assert!(pids.iter().all(|p| *p > 0));
    }

    #[test]
    fn pause_then_resume_signals_all() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launcher = Arc::new(SleepLauncher::new());
        let host = LocalHost::with_launcher(
            tmp.path().to_path_buf(),
            Box::new(SleepLauncherRef(launcher.clone())),
        );
        let _ = host.enqueue_master(&make_master_spec("t1", tmp.path())).unwrap();
        let _ = host.enqueue_worker(&make_worker_spec("t1", "p1", tmp.path())).unwrap();

        let paused = host.pause_task_sessions("t1").unwrap();
        assert_eq!(paused, 2);
        let resumed = host.resume_task_sessions("t1").unwrap();
        assert_eq!(resumed, 2);

        host.terminate_task_sessions("t1").unwrap();
    }

    #[test]
    fn forget_removes_session_record() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launcher = Arc::new(SleepLauncher::new());
        let host = LocalHost::with_launcher(
            tmp.path().to_path_buf(),
            Box::new(SleepLauncherRef(launcher.clone())),
        );
        let sid = host.enqueue_master(&make_master_spec("t1", tmp.path())).unwrap();
        assert_eq!(host.live_sessions().len(), 1);
        let rec = host.forget(&sid).unwrap();
        assert_eq!(rec.session_id, sid);
        assert_eq!(host.live_sessions().len(), 0);

        host.terminate_task_sessions("t1").unwrap();
    }

    /// Thin wrapper because `Box<dyn ProcessLauncher>` needs an owned impl,
    /// but the tests want to share the same `SleepLauncher` for cleanup.
    struct SleepLauncherRef(Arc<SleepLauncher>);
    impl ProcessLauncher for SleepLauncherRef {
        fn launch_master(
            &self,
            session_id: &str,
            spec: &MasterSpawnSpec,
        ) -> Result<u32, String> {
            self.0.launch_master(session_id, spec)
        }
        fn launch_worker(
            &self,
            session_id: &str,
            spec: &WorkerSpawnSpec,
        ) -> Result<u32, String> {
            self.0.launch_worker(session_id, spec)
        }
        fn launch_auditor(
            &self,
            session_id: &str,
            spec: &AuditorSpawnSpec,
            output_path: &Path,
        ) -> Result<u32, String> {
            self.0.launch_auditor(session_id, spec, output_path)
        }
    }

    /// [WA1] `launch_auditor` is invoked and spawns a real (sleeping) pid — the
    /// ProcessLauncher contract now covers the adversarial Auditor session, not
    /// just master/worker. We call it directly on the launcher (LocalHost has no
    /// auditor-enqueue method; the audit orchestrator in `actions::audit_pitem`
    /// drives the launcher itself).
    #[test]
    fn launch_auditor_spawns_a_tracked_pid() {
        use claw_fleet_task::auditor::auditor_quorum_specs;
        let launcher = SleepLauncher::new();
        let specs = auditor_quorum_specs("t-aud", "/tmp/master-decisions.jsonl");
        let out = std::env::temp_dir().join("wa1_audit_out_0.json");
        let pid = launcher
            .launch_auditor("aud-sid-0", &specs[0], &out)
            .unwrap();
        assert!(pid_alive(pid), "launch_auditor must spawn a live process");
        assert_eq!(
            launcher.auditor_calls.load(Ordering::SeqCst),
            1,
            "launch_auditor must be invoked exactly once"
        );
        // cleanup (Drop kills children too, but be explicit).
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}
