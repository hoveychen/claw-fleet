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

use claw_fleet_task::asset_inject;
use claw_fleet_task::orchestrator::OrchestratorHost;
use claw_fleet_task::runner::{LlmMediator, Resolution, TaskLifecycleHost};
use claw_fleet_task::spawn_specs::PlannerSpawnSpec;
use claw_fleet_task::task::{E2eOutcome, Task};
use claw_fleet_task::worker::{worker_spawn_spec, WorkerSpawnSpec};
use claw_fleet_task::worktree::{self, ConflictSpec};
use claw_fleet_task::{verify, verify_config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Planner,
    Worker,
}

/// Per-session log file capturing the spawned `claude`'s stderr:
/// `<fleet_dir>/logs/session-<session_id>.log`. Without this the planner's
/// startup failure (e.g. "Not logged in" when the process is sandboxed and
/// can't reach the keychain) went to `/dev/null`, leaving a silent hang with
/// no diagnosable cause. The orchestrator reads this on a dead-planner check.
pub fn session_log_path(session_id: &str) -> Option<PathBuf> {
    let dir = claw_fleet_task::paths::get_fleet_dir()?.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("session-{session_id}.log")))
}

/// Last `max_bytes` of a session's captured stderr, trimmed. Empty when the log
/// is missing/unreadable.
pub fn tail_session_log(session_id: &str, max_bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Some(path) = session_log_path(session_id) else {
        return String::new();
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return String::new();
    };
    let Ok(mut f) = std::fs::File::open(&path) else {
        return String::new();
    };
    let start = meta.len().saturating_sub(max_bytes);
    if start > 0 {
        let _ = f.seek(SeekFrom::Start(start));
    }
    let mut buf = String::new();
    let _ = f.read_to_string(&mut buf);
    buf.trim().to_string()
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub pid: u32,
    pub kind: SessionKind,
    /// Which P-item this session belongs to (None for the planner). Recorded on
    /// every worker spawn and asserted in tests, but no production code reads it
    /// yet — the P7 SSE event payloads will. Kept rather than dropped so the
    /// session→P-item association isn't lost in the meantime.
    #[allow(dead_code)]
    pub p_item_id: Option<String>,
}

/// Trait for actually launching a Claude (or test-fake) subprocess. Lets us
/// unit-test `LocalHost` without depending on the real `claude` CLI.
///
/// Two isolation tiers (方案 S), both funnelled through [`spawn_claude`]:
/// - **planner** — interactive: NO safe-mode, keeps AskUserQuestion/fleet__ask,
///   inherits the user's config, persists.
/// - **worker** — isolated: `--safe-mode --permission-mode bypassPermissions
///   --no-session-persistence`, assets injected via the spec's `system_prompt`.
///
/// Review is NOT spawned here: it runs synchronously through
/// `review_gate::RealReviewGate` (P6), which shells out to `claude` and parses
/// the structured verdict inline rather than tracking an async session.
pub trait ProcessLauncher: Send + Sync {
    fn launch_planner(&self, session_id: &str, spec: &PlannerSpawnSpec) -> Result<u32, String>;
    fn launch_worker(&self, session_id: &str, spec: &WorkerSpawnSpec) -> Result<u32, String>;
}

/// The isolation tier a spawned session runs under. `Interactive` keeps the
/// user's full environment (planner); `Sandboxed` is the safe-mode +
/// bypass-permissions + no-persistence profile workers/reviews run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    Interactive,
    Sandboxed,
}

/// Everything `spawn_claude` needs, bundled so the call sites stay readable and
/// clippy's too-many-arguments lint stays quiet.
pub struct ClaudeSpawn<'a> {
    pub session_id: &'a str,
    pub cwd: &'a Path,
    pub prompt: &'a str,
    pub system_prompt: Option<&'a str>,
    pub model: Option<&'a str>,
    pub kind_env: &'a str,
    pub task_id: Option<&'a str>,
    pub p_item_id: Option<&'a str>,
    pub isolation: Isolation,
    /// Domain skill plugin dirs to load (`--plugin-dir`). Empty = built-in
    /// skills only.
    pub plugin_dirs: &'a [String],
}

/// Default launcher that shells out to the `claude` CLI on PATH.
pub struct ClaudeLauncher;

impl ProcessLauncher for ClaudeLauncher {
    fn launch_planner(&self, session_id: &str, spec: &PlannerSpawnSpec) -> Result<u32, String> {
        let prompt = format!(
            "Task `{}` needs a plan. Clarify the requirements with the user \
             (use AskUserQuestion / fleet__ask), then write the DAG via \
             `fleet task update-plan`.",
            spec.task_id
        );
        spawn_claude(&ClaudeSpawn {
            session_id,
            cwd: &spec.cwd,
            prompt: &prompt,
            system_prompt: Some(&spec.system_prompt),
            model: Some(&spec.model),
            kind_env: "planner",
            task_id: Some(&spec.task_id),
            p_item_id: None,
            // Interactive: keep the user's config + interaction tools.
            isolation: Isolation::Interactive,
            plugin_dirs: &[],
        })
    }

    fn launch_worker(&self, session_id: &str, spec: &WorkerSpawnSpec) -> Result<u32, String> {
        let prompt = format!(
            "Execute P-item `{}` per the SYSTEM prompt above. When you are done, \
             stop the process; the orchestrator will run the review.",
            spec.p_item_id
        );
        spawn_claude(&ClaudeSpawn {
            session_id,
            cwd: &spec.cwd,
            prompt: &prompt,
            system_prompt: Some(&spec.system_prompt),
            model: Some(&spec.model),
            kind_env: "worker",
            task_id: Some(&spec.task_id),
            p_item_id: Some(&spec.p_item_id),
            // Isolated: safe-mode + bypass + no-persistence.
            isolation: Isolation::Sandboxed,
            plugin_dirs: &[],
        })
    }
}

/// Env override used by the binary integration test: when
/// `FLEET_TASK_FAKE_LAUNCHER=1`, ClaudeLauncher spawns `sleep 60` instead of
/// the real `claude` CLI so the test doesn't need claude installed.
const FAKE_LAUNCHER_ENV: &str = "FLEET_TASK_FAKE_LAUNCHER";

/// Build the `claude` CLI argument vector (everything after the `claude`
/// program name) for a spawn. Pure — no process side-effects — so the isolation
/// tiers can be unit-tested by asserting on the produced flags.
///
/// Sandboxed (worker / review): `--safe-mode --permission-mode bypassPermissions
/// --no-session-persistence`. Interactive (planner / master): none of those, so
/// the session keeps the user's full config + interaction tools + persistence.
fn build_claude_args(spawn: &ClaudeSpawn) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--session-id".into(),
        spawn.session_id.into(),
    ];
    if spawn.isolation == Isolation::Sandboxed {
        args.push("--safe-mode".into());
        args.push("--permission-mode".into());
        args.push("bypassPermissions".into());
        args.push("--no-session-persistence".into());
    }
    for dir in spawn.plugin_dirs {
        args.push("--plugin-dir".into());
        args.push(dir.clone());
    }
    if let Some(sp) = spawn.system_prompt {
        args.push("--append-system-prompt".into());
        args.push(sp.into());
    }
    if let Some(m) = spawn.model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push(spawn.prompt.into());
    args
}

fn spawn_claude(spawn: &ClaudeSpawn) -> Result<u32, String> {
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
    // Resolve claude's ABSOLUTE path (honouring the user's "Claude binary"
    // override) rather than relying on bare `claude` on PATH. The desktop app,
    // launched from Finder/Dock, hands spawned children a stripped GUI PATH
    // (/usr/bin:/bin:…) without the user's shell additions (~/.local/bin,
    // homebrew, nvm), so bare `claude` failed with "No such file or directory".
    let config = claw_fleet_core::claude_binary::ClaudeBinaryConfig::load();
    let bin = claw_fleet_core::claude_binary::resolve(config.override_path.as_deref())
        .ok_or_else(|| {
            "claude CLI not found (searched PATH + ~/.local/bin, homebrew, nvm). \
             Set the Claude binary path in Fleet Settings → Connection."
                .to_string()
        })?;
    let mut cmd = claw_fleet_task::process_util::command(&bin.path);
    cmd.current_dir(spawn.cwd);
    // Enrich PATH so the resolved claude (and any child tooling it shells out
    // to) can still find common user-installed binaries despite the stripped
    // GUI env — prepend claude's own dir + the usual install locations.
    {
        let mut parts: Vec<String> = Vec::new();
        if let Some(dir) = std::path::Path::new(&bin.path).parent().and_then(|p| p.to_str()) {
            parts.push(dir.to_string());
        }
        if let Some(home) =
            claw_fleet_core::paths::real_home_dir().and_then(|h| h.to_str().map(str::to_string))
        {
            parts.push(format!("{home}/.local/bin"));
            parts.push(format!("{home}/.bun/bin"));
        }
        parts.push("/opt/homebrew/bin".into());
        parts.push("/usr/local/bin".into());
        if let Some(existing) = std::env::var_os("PATH").and_then(|p| p.into_string().ok()) {
            parts.push(existing);
        }
        cmd.env("PATH", parts.join(":"));
    }
    for arg in build_claude_args(spawn) {
        cmd.arg(arg);
    }
    cmd.env("FLEET_SESSION_ID", spawn.session_id)
        .env("FLEET_SESSION_KIND", spawn.kind_env);
    if let Some(t) = spawn.task_id {
        cmd.env("FLEET_TASK_ID", t);
    }
    if let Some(p) = spawn.p_item_id {
        cmd.env("FLEET_P_ITEM_ID", p);
    }
    // Capture stderr to a per-session log (truncated per start) so a startup
    // failure — e.g. the planner exiting "Not logged in" under the sandbox —
    // is diagnosable instead of vanishing into /dev/null. Fall back to null if
    // the log file can't be created so spawning never fails just for logging.
    let stderr = session_log_path(spawn.session_id)
        .and_then(|p| std::fs::File::create(p).ok())
        .map(Stdio::from)
        .unwrap_or_else(Stdio::null);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    let child = cmd.spawn().map_err(|e| format!("spawn claude: {e}"))?;
    Ok(child.id())
}

pub struct LocalHost {
    workspace: PathBuf,
    sessions: Mutex<HashMap<String, SessionRecord>>,
    launcher: Box<dyn ProcessLauncher>,
}

impl LocalHost {
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

    /// The project workspace this host's task runs in (worktree base / planner cwd).
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Drop a tracked session — call after observing the subprocess exit.
    /// Covered by `forget_removes_session_record`; no production caller reaps
    /// individual sessions yet (shutdown signals them all at once), so the
    /// per-session reaper is kept for tests + the eventual P7 event loop.
    #[allow(dead_code)]
    pub fn forget(&self, session_id: &str) -> Option<SessionRecord> {
        self.sessions.lock().unwrap().remove(session_id)
    }

    /// Spawn the interactive planning session (P5). Tracked as
    /// [`SessionKind::Planner`]; returns its session id.
    pub fn enqueue_planner(&self, spec: &PlannerSpawnSpec) -> Result<String, String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let pid = self.launcher.launch_planner(&session_id, spec)?;
        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionRecord {
                session_id: session_id.clone(),
                pid,
                kind: SessionKind::Planner,
                p_item_id: None,
            },
        );
        Ok(session_id)
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
    fn resolve_conflicts(&self, files: &[ConflictSpec]) -> Result<Vec<Resolution>, String> {
        // One `claude` call per conflicted file (mirrors review_gate's spawn
        // pattern; fleet-task can't depend on core's `merge_mediator`, so it
        // shares only the pure prompt/parse helpers in claw_fleet_task).
        let mut out = Vec::with_capacity(files.len());
        for spec in files {
            let prompt = claw_fleet_task::merge_prompt::render_prompt(spec);
            let stdout = run_mediation_claude(&self.workspace, &prompt)?;
            let resolved = claw_fleet_task::merge_prompt::extract_resolved(&stdout).ok_or_else(|| {
                format!("mediator response for {} missing <resolved> wrapper", spec.path.display())
            })?;
            if let Some(marker) = claw_fleet_task::merge_prompt::first_conflict_marker(&resolved) {
                return Err(format!(
                    "mediator left conflict marker `{marker}` in {}",
                    spec.path.display()
                ));
            }
            out.push(Resolution { path: spec.path.clone(), resolved_content: resolved });
        }
        Ok(out)
    }
}

/// Spawn an isolated `claude` session to resolve one merge conflict. Plain-text
/// output (the prompt asks for `<resolved>...</resolved>` tags). Mirrors
/// `review_gate::run_review`'s safe-mode invocation.
fn run_mediation_claude(cwd: &Path, prompt: &str) -> Result<String, String> {
    let mut cmd = claw_fleet_task::process_util::command("claude");
    cmd.current_dir(cwd)
        .arg("--print")
        .arg("--safe-mode")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg("--no-session-persistence")
        .arg("--model")
        .arg("claude-sonnet-4-6")
        .arg(prompt)
        .env("FLEET_SESSION_KIND", "merge-mediator");
    let out = cmd.output().map_err(|e| format!("spawn mediation claude: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "mediation claude exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

impl OrchestratorHost for LocalHost {
    /// Provision the P-item's worktree, build its worker spec, inject the
    /// transplanted assets (P2: engineering principles + relevant memory) into
    /// the system prompt, and spawn the worker. Returns the worker session id.
    fn dispatch_worker(&self, task: &Task, p_item_id: &str) -> Result<String, String> {
        let task_branch = task
            .task_branch
            .as_deref()
            .ok_or_else(|| format!("task {} has no task_branch — call start_task first", task.id))?;
        let cwd = worktree::provision(&self.workspace, task_branch, &task.id, p_item_id)?;
        let mut spec = worker_spawn_spec(task, p_item_id, cwd)?;
        // 方案 S: a safe-mode worker lost the user's principles + project
        // memory; transplant them back via the appended system prompt.
        let p_item = task
            .plan
            .get(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found"))?;
        let principles = asset_inject::compose_engineering_principles();
        let memory = asset_inject::relevant_memory(task, p_item);
        spec.system_prompt = if memory.is_empty() {
            format!("{}\n\n{principles}", spec.system_prompt)
        } else {
            format!("{}\n\n{principles}\n\n{memory}", spec.system_prompt)
        };
        self.enqueue_worker(&spec)
    }

    fn session_finished(&self, session_id: &str) -> bool {
        let pid = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|r| r.pid);
        match pid {
            Some(pid) => !pid_alive(pid),
            // Unknown session id → treat as finished so the loop doesn't wedge.
            None => true,
        }
    }

    /// Merge the P-item's worktree branch back into the task branch and reap
    /// the worktree. A real merge conflict is surfaced as an error (the
    /// orchestrator escalates to the user rather than auto-resolving).
    fn merge_and_reap(&self, task: &Task, p_item_id: &str) -> Result<(), String> {
        let task_branch = task
            .task_branch
            .as_deref()
            .ok_or_else(|| format!("task {} has no task_branch", task.id))?;
        // The worker never commits (its contract forbids it). Commit its
        // uncommitted changes onto the worker branch first, or merge_back would
        // see an empty branch and silently drop the work.
        worktree::commit_worktree(&task.id, p_item_id, &format!("fleet: {p_item_id} worker output"))?;
        let outcome = worktree::merge_back(&self.workspace, task_branch, &task.id, p_item_id)?;
        if let claw_fleet_task::worktree::MergeOutcome::Conflict { files, .. } = &outcome {
            // LLM-mediate the conflict instead of failing the P-item outright.
            // resolve_conflicts spawns claude per file; apply_resolutions re-runs
            // the merge with the resolved content and commits. A mediation
            // failure still surfaces as an error (P-item fails) — but a real
            // 3-way conflict no longer dead-ends the merge.
            let resolutions: Vec<(std::path::PathBuf, String)> = self
                .resolve_conflicts(files)?
                .into_iter()
                .map(|r| (r.path, r.resolved_content))
                .collect();
            worktree::apply_resolutions(&self.workspace, &task.id, p_item_id, &resolutions)?;
        }
        worktree::reap(&self.workspace, &task.id, p_item_id)
    }

    fn run_task_e2e(&self, task: &Task) -> Result<Option<E2eOutcome>, String> {
        let cfg = verify_config::read_verify_config(&self.workspace)?;
        let Some(cmd) = cfg.e2e.filter(|c| !c.trim().is_empty()) else {
            return Ok(None); // no e2e configured → orchestrator proceeds to acceptance
        };
        let task_branch = task
            .task_branch
            .as_deref()
            .ok_or_else(|| format!("task {} has no task_branch for e2e", task.id))?;

        // Run e2e against the finished task-branch state in a throwaway detached
        // worktree so the user's own checkout is never disturbed. `--detach`
        // checks out the commit (not the branch), so it never collides with a
        // branch already checked out elsewhere.
        let wt_dir = worktree::worktree_root()?.join(&task.id).join("__e2e__");
        let remove = |dir: &Path| {
            let _ = Command::new("git")
                .arg("-C")
                .arg(&self.workspace)
                .args(["worktree", "remove", "--force"])
                .arg(dir)
                .output();
        };
        remove(&wt_dir); // clear any stale leftover first
        let add = Command::new("git")
            .arg("-C")
            .arg(&self.workspace)
            .args(["worktree", "add", "--detach"])
            .arg(&wt_dir)
            .arg(task_branch)
            .output()
            .map_err(|e| format!("git worktree add for e2e: {e}"))?;
        if !add.status.success() {
            return Err(format!(
                "git worktree add for e2e failed: {}",
                String::from_utf8_lossy(&add.stderr).trim()
            ));
        }

        let run = verify::run_check(&cmd, &wt_dir);
        remove(&wt_dir); // best-effort reap

        let mut gaps = Vec::new();
        if !run.ok {
            gaps.push(format!("e2e 命令 `{}` 未通过（退出码非 0）", run.command));
            if !run.output_tail.is_empty() {
                gaps.push(format!("输出末尾：{}", run.output_tail));
            }
        }
        let ran_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Ok(Some(E2eOutcome {
            command: cmd,
            passed: run.ok,
            gaps,
            ran_at,
        }))
    }
}

/// True if `pid` is still running. Uses `waitpid(WNOHANG)` to reap our own
/// exited children (the worker/review subprocesses we spawned) so a finished
/// session reads as not-alive rather than lingering as a zombie.
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    let target = pid as libc::pid_t;
    let mut status: libc::c_int = 0;
    let r = unsafe { libc::waitpid(target, &mut status, libc::WNOHANG) };
    if r == target {
        return false; // just reaped → dead
    }
    unsafe { libc::kill(target, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Test launcher that spawns a long-running `sleep` instead of `claude`.
    /// Real signals against real pids prove the SIGSTOP / SIGTERM plumbing.
    struct SleepLauncher {
        children: Mutex<Vec<std::process::Child>>,
    }

    impl SleepLauncher {
        fn new() -> Self {
            Self {
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
        fn launch_planner(
            &self,
            _session_id: &str,
            _spec: &PlannerSpawnSpec,
        ) -> Result<u32, String> {
            self.spawn_sleeper()
        }
        fn launch_worker(
            &self,
            _session_id: &str,
            _spec: &WorkerSpawnSpec,
        ) -> Result<u32, String> {
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

    fn make_worker_spec(task_id: &str, p_id: &str, cwd: &Path) -> WorkerSpawnSpec {
        WorkerSpawnSpec {
            task_id: task_id.into(),
            p_item_id: p_id.into(),
            cwd: cwd.to_path_buf(),
            system_prompt: "sys".into(),
            // WorkerSpawnSpec.model is an owned String now.
            model: "claude-sonnet-4-6".to_string(),
        }
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
        let _ = host.enqueue_planner(&make_planner_spec("t1", tmp.path())).unwrap();
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
        let _ = host.enqueue_planner(&make_planner_spec("t1", tmp.path())).unwrap();
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
        let sid = host.enqueue_planner(&make_planner_spec("t1", tmp.path())).unwrap();
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
        fn launch_planner(
            &self,
            session_id: &str,
            spec: &PlannerSpawnSpec,
        ) -> Result<u32, String> {
            self.0.launch_planner(session_id, spec)
        }
        fn launch_worker(
            &self,
            session_id: &str,
            spec: &WorkerSpawnSpec,
        ) -> Result<u32, String> {
            self.0.launch_worker(session_id, spec)
        }
    }

    // ── isolation-tier argv tests ────────────────────────────────────────────

    fn spawn_for(isolation: Isolation, plugin_dirs: &[String]) -> Vec<String> {
        let cwd = PathBuf::from("/tmp");
        build_claude_args(&ClaudeSpawn {
            session_id: "sid-1",
            cwd: &cwd,
            prompt: "do the thing",
            system_prompt: Some("SYS"),
            model: Some("claude-sonnet-4-6"),
            kind_env: "worker",
            task_id: Some("t1"),
            p_item_id: Some("p1"),
            isolation,
            plugin_dirs,
        })
    }

    #[test]
    fn sandboxed_tier_carries_safe_mode_bypass_and_no_persistence() {
        let args = spawn_for(Isolation::Sandboxed, &[]);
        assert!(args.iter().any(|a| a == "--safe-mode"), "{args:?}");
        assert!(args.iter().any(|a| a == "--permission-mode"), "{args:?}");
        assert!(args.iter().any(|a| a == "bypassPermissions"), "{args:?}");
        assert!(args.iter().any(|a| a == "--no-session-persistence"), "{args:?}");
        // always-present basics + the appended assets/model/prompt.
        assert!(args.iter().any(|a| a == "--session-id"));
        assert!(args.iter().any(|a| a == "--append-system-prompt"));
        assert_eq!(args.last().unwrap(), "do the thing");
    }

    #[test]
    fn interactive_tier_omits_sandbox_flags() {
        let args = spawn_for(Isolation::Interactive, &[]);
        assert!(!args.iter().any(|a| a == "--safe-mode"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--permission-mode"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--no-session-persistence"), "{args:?}");
        // still a real session with the session id + prompt.
        assert!(args.iter().any(|a| a == "--session-id"));
        assert_eq!(args.last().unwrap(), "do the thing");
    }

    #[test]
    fn plugin_dirs_become_plugin_dir_flags() {
        let dirs = vec!["/skills/a".to_string(), "/skills/b".to_string()];
        let args = spawn_for(Isolation::Sandboxed, &dirs);
        let count = args.iter().filter(|a| *a == "--plugin-dir").count();
        assert_eq!(count, 2, "one --plugin-dir per dir: {args:?}");
        assert!(args.iter().any(|a| a == "/skills/a"));
        assert!(args.iter().any(|a| a == "/skills/b"));
    }

    fn make_planner_spec(task_id: &str, cwd: &Path) -> PlannerSpawnSpec {
        PlannerSpawnSpec {
            task_id: task_id.into(),
            cwd: cwd.to_path_buf(),
            system_prompt: "sys".into(),
            model: "claude-sonnet-4-6".to_string(),
        }
    }

    #[test]
    fn enqueue_planner_tracks_kind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launcher = Arc::new(SleepLauncher::new());
        let host = LocalHost::with_launcher(
            tmp.path().to_path_buf(),
            Box::new(SleepLauncherRef(launcher.clone())),
        );
        let psid = host.enqueue_planner(&make_planner_spec("t1", tmp.path())).unwrap();
        let live = host.live_sessions();
        assert_eq!(live.len(), 1);
        let planner = live.iter().find(|r| r.session_id == psid).unwrap();
        assert_eq!(planner.kind, SessionKind::Planner);
        assert_eq!(planner.p_item_id, None);
        host.terminate_task_sessions("t1").unwrap();
    }
}
