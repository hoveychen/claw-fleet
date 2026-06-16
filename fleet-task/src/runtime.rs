//! Glue layer that boots a new (or resumed) task in the current process:
//! creates / loads the task json, opens the git branch (via `start_task`),
//! writes a runtime registry entry, and starts the HTTP server. The task is
//! then driven by the deterministic [`run_orchestrator_loop`] — no LLM master.
//!
//! Pure of CLI / TUI concerns so the boot path is unit-testable with a
//! `SleepLauncher` against a tempdir git repo. `main.rs` only handles arg
//! parsing, the orchestrator thread, and the foreground wait loop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use claw_fleet_task::actions::{self, StartOpts};
use claw_fleet_task::orchestrator::{Orchestrator, OrchestratorStep, ReviewGate};
use claw_fleet_task::planning;
use claw_fleet_task::runner::TaskLifecycleHost;
use claw_fleet_task::task::{
    create_task, get_task, write_task_atomic, TaskInput, TaskStatus,
};

use claw_fleet_task::registry::{self, RegistryEntry};

use crate::http::{self, HttpHandle, ServerConfig};
use crate::local_host::{LocalHost, ProcessLauncher};
use crate::sse::SseBroadcaster;

/// Synthetic project id used when `fleet-task` boots without a desktop-side
/// project entry. Phase 3 will let real project ids flow through; until then
/// every `fleet-task new` belongs to this catch-all bucket.
pub const FLEET_TASK_LOCAL_PROJECT: &str = "fleet-task-local";

pub struct Bootstrap {
    pub task_id: String,
    pub http_handle: HttpHandle,
    pub host: Arc<LocalHost>,
    pub broadcaster: SseBroadcaster,
}

pub struct NewTaskArgs {
    pub workspace: PathBuf,
    pub title: String,
    pub prompt: String,
}

pub fn boot_new_task(
    args: NewTaskArgs,
    launcher: Box<dyn ProcessLauncher>,
) -> Result<Bootstrap, String> {
    if !args.workspace.is_dir() {
        return Err(format!(
            "workspace not a directory: {}",
            args.workspace.display()
        ));
    }
    // Phase 5: reverse-look the desktop's `projects.json` so a task started
    // via `fleet-task new` on a workspace the user already added in the
    // desktop UI lands under the same project_id — that's what makes it
    // visible in the Kanban/TasksView grouped by project.
    //
    // When no registered project owns this workspace, fall back to a
    // *workspace-stable* synthetic id (`derive_auto_project_id`) so the
    // desktop can auto-discover one virtual project per distinct workspace
    // (see `claw_fleet_core::project::list_projects_discovered`). The old
    // constant `fleet-task-local` collapsed every unregistered workspace
    // into a single bucket; we only fall back to it when the workspace path
    // can't be canonicalised.
    let project_id = lookup_project_id(&args.workspace)
        .unwrap_or_else(|| derive_auto_project_id(&args.workspace));
    let task = create_task(TaskInput {
        project_id,
        title: args.title.clone(),
        description: args.prompt,
    })?;

    let host = Arc::new(LocalHost::with_launcher(args.workspace, launcher));
    // Prepare the task (branch + Running); the orchestrator loop drives it.
    actions::start_task(&task.id, &*host, StartOpts::default())?;
    maybe_launch_planner(&task.id, &host)?;
    finish_boot(task.id, host)
}

/// P5: if the task has no plan yet, spawn the single interactive planner
/// session. The orchestrator idles on the empty plan until the planner writes
/// the DAG via `fleet task update-plan`, then dispatches workers. No-op when a
/// plan already exists (e.g. resuming a planned task).
fn maybe_launch_planner(task_id: &str, host: &LocalHost) -> Result<(), String> {
    let task = get_task(task_id)?;
    if !task.plan.is_empty() {
        return Ok(());
    }
    let spec = planning::planner_spawn_spec(&task, host.workspace().to_path_buf());
    host.enqueue_planner(&spec)?;
    Ok(())
}

pub fn boot_resume(
    task_id: String,
    workspace: PathBuf,
    launcher: Box<dyn ProcessLauncher>,
) -> Result<Bootstrap, String> {
    if !workspace.is_dir() {
        return Err(format!(
            "workspace not a directory: {}",
            workspace.display()
        ));
    }
    let mut task = get_task(&task_id)?;
    let host = Arc::new(LocalHost::with_launcher(workspace, launcher));
    // start_task is idempotent on Running tasks; for Paused we flip to
    // Running here first so the master gets a fresh spawn either way.
    if matches!(task.status, TaskStatus::Paused) {
        task.status = TaskStatus::Running;
        write_task_atomic(&task)?;
    }
    // Idempotent on already-Running tasks; (re)stamps branch/workspace.
    actions::start_task(&task_id, &*host, StartOpts::default())?;
    maybe_launch_planner(&task_id, &host)?;
    finish_boot(task_id, host)
}

fn finish_boot(task_id: String, host: Arc<LocalHost>) -> Result<Bootstrap, String> {
    let broadcaster = SseBroadcaster::new();
    let dispatcher: Arc<dyn http::DispatchTrigger> = Arc::new(LocalDispatcher {
        task_id: task_id.clone(),
        host: host.clone(),
    });
    let cfg = ServerConfig {
        task_id: task_id.clone(),
        broadcaster: broadcaster.clone(),
        dispatcher: Some(dispatcher),
    };
    let http_handle = http::spawn(cfg).map_err(|e| format!("http: {e}"))?;
    registry::write(&RegistryEntry {
        task_id: task_id.clone(),
        pid: std::process::id(),
        port: http_handle.port,
        started_at: chrono::Utc::now().to_rfc3339(),
    })
    .map_err(|e| format!("registry: {e}"))?;
    Ok(Bootstrap {
        task_id,
        http_handle,
        host,
        broadcaster,
    })
}

/// Drive the task to completion with the deterministic orchestrator. Loops
/// `Orchestrator::step`, sleeping on `Idle`, until the plan completes or
/// `stop` is set. Runs on its own thread (spawned by `main.rs`).
pub fn run_orchestrator_loop(
    task_id: String,
    host: Arc<LocalHost>,
    gate: Arc<dyn ReviewGate + Send + Sync>,
    stop: Arc<AtomicBool>,
) {
    let orch = Orchestrator::new(task_id.clone());
    // A persistently-failing step (e.g. a review session that never produces a
    // parseable verdict) must NOT spin forever re-spawning claude. Bound the
    // consecutive errors; on the limit, stop driving and leave the task for the
    // user to inspect rather than burning quota in a tight loop.
    const MAX_CONSECUTIVE_ERRORS: u32 = 5;
    let mut consecutive_errors = 0u32;
    while !stop.load(Ordering::SeqCst) {
        match orch.step(&*host, &*gate) {
            Ok(OrchestratorStep::PlanComplete) => break,
            Ok(OrchestratorStep::Idle) => {
                consecutive_errors = 0;
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            Ok(_) => {
                // A transition happened; loop immediately to take the next.
                consecutive_errors = 0;
            }
            Err(e) => {
                consecutive_errors += 1;
                eprintln!(
                    "[orchestrator] task {task_id} step error ({consecutive_errors}/{MAX_CONSECUTIVE_ERRORS}): {e}"
                );
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    eprintln!(
                        "[orchestrator] task {task_id}: too many consecutive errors — \
                         stopping the driver (task left in place for inspection)."
                    );
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
}

/// Read the desktop's `projects.json` (under `<home>/.claude/fleet/`, mirroring
/// `claw_fleet_core::project::projects_file()`) and return the `id` of any
/// project whose `workspace` canonicalises to the same path. Returns None
/// when the file is missing, malformed, or no project matches.
fn lookup_project_id(workspace: &std::path::Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct MiniProject {
        id: String,
        workspace: String,
    }
    let home = claw_fleet_task::paths::real_home_dir()?;
    let path = home.join(".claude").join("fleet").join("projects.json");
    let bytes = std::fs::read(&path).ok()?;
    let projects: Vec<MiniProject> = serde_json::from_slice(&bytes).ok()?;
    let canonical = workspace.canonicalize().ok()?;
    for p in projects {
        if let Ok(c) = std::path::Path::new(&p.workspace).canonicalize() {
            if c == canonical {
                return Some(p.id);
            }
        }
    }
    None
}

/// Deterministic, workspace-stable project id for a workspace that no
/// registered project owns. Two `fleet-task new` invocations on the same
/// workspace produce the same id, so the desktop's auto-discovery groups
/// their tasks under a single virtual project; two *different* workspaces
/// produce different ids, so each surfaces as its own virtual project.
///
/// Format: `auto-<16 hex>` where the hash is `DefaultHasher` (SipHash-1-3
/// with the fixed zero key — deterministic across runs) over the
/// canonicalised path, falling back to the raw path when canonicalisation
/// fails (e.g. the dir was removed between creation and this call). When even
/// the raw path is unavailable we return the legacy `fleet-task-local`
/// constant so behaviour degrades rather than panics.
fn derive_auto_project_id(workspace: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let key = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let key = key.to_string_lossy();
    if key.is_empty() {
        return FLEET_TASK_LOCAL_PROJECT.into();
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    format!("auto-{:016x}", h.finish())
}

pub fn shutdown(boot: Bootstrap) {
    shutdown_with_deadline(boot, std::time::Duration::from_secs(30));
}

/// Bridge that lets the HTTP server reach back into the LocalHost owning
/// this task's pid map. Per Phase 3 P8: `POST /p-items/<id>/dispatch` no
/// longer returns 503 — it routes here and `actions::dispatch_pitem` flips
/// the P-item to Running + spawns a Worker via LocalHost.
struct LocalDispatcher {
    task_id: String,
    host: Arc<LocalHost>,
}

impl http::DispatchTrigger for LocalDispatcher {
    fn trigger(&self, p_item_id: &str) -> Result<(), String> {
        claw_fleet_task::actions::dispatch_pitem(&self.task_id, p_item_id, &*self.host)
    }
}

/// Graceful shutdown with a hard wall clock:
///   1. SIGTERM all subprocesses (host.terminate_task_sessions)
///   2. Poll `kill(pid, 0)` for up to `deadline`; break early if all dead.
///   3. Any pid still alive past `deadline` gets SIGKILL.
///   4. Registry entry + HTTP handle close last.
///
/// Tests can pass a short deadline (e.g. 200ms) to exercise the SIGKILL path
/// without lingering. Production CLI uses 30s.
///
/// Phase-2 known limitation: we probe via `kill(pid, 0)` rather than
/// `Child::try_wait`, so zombie pids (subprocess exited but never reaped
/// because we don't own the Child handle) read as alive until the kernel
/// reaps them on our own exit. In practice fleet-task immediately exits
/// after this returns, so the kernel cleans up anyway.
pub fn shutdown_with_deadline(boot: Bootstrap, deadline: std::time::Duration) {
    let pids: Vec<u32> = boot.host.live_sessions().iter().map(|r| r.pid).collect();
    let _ = boot.host.terminate_task_sessions(&boot.task_id);

    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if !pids.iter().any(|p| pid_alive_reaping(*p)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    #[cfg(unix)]
    for &p in &pids {
        if pid_alive_reaping(p) {
            unsafe {
                libc::kill(p as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    let _ = registry::remove(&boot.task_id);
    boot.http_handle.shutdown();
}

/// True if `pid` is still running (returns false for reaped, exited, or
/// unknown pids). Uses `waitpid(WNOHANG)` so zombie children of the current
/// process get reaped here rather than appearing alive forever to a
/// `kill(pid, 0)` probe.
#[cfg(unix)]
fn pid_alive_reaping(pid: u32) -> bool {
    let target = pid as libc::pid_t;
    let mut status: libc::c_int = 0;
    // Try to reap a child first; if it's our child and has exited, this
    // collects it and returns target.
    let r = unsafe { libc::waitpid(target, &mut status, libc::WNOHANG) };
    if r == target {
        return false; // just reaped — definitely dead
    }
    if r == -1 {
        // ECHILD = not our child / already reaped. Fall through to kill(0).
    }
    // Either still running (waitpid returned 0) or not our child; double-check
    // with the signal-0 probe so unrelated already-dead pids read as dead.
    unsafe { libc::kill(target, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive_reaping(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_host::ProcessLauncher;
    use claw_fleet_task::paths::fleet_home_lock;
    use claw_fleet_task::worker::WorkerSpawnSpec;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }
    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }
    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    struct SleepLauncher {
        children: Mutex<Vec<std::process::Child>>,
    }
    impl SleepLauncher {
        fn new() -> Self {
            Self {
                children: Mutex::new(Vec::new()),
            }
        }
        fn spawn(&self) -> Result<u32, String> {
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
    }
    impl ProcessLauncher for SleepLauncher {
        fn launch_planner(
            &self,
            _s: &str,
            _spec: &claw_fleet_task::spawn_specs::PlannerSpawnSpec,
        ) -> Result<u32, String> {
            self.spawn()
        }
        fn launch_worker(&self, _s: &str, _spec: &WorkerSpawnSpec) -> Result<u32, String> {
            self.spawn()
        }
        fn launch_review(
            &self,
            _s: &str,
            _spec: &claw_fleet_task::spawn_specs::ReviewSpawnSpec,
        ) -> Result<u32, String> {
            self.spawn()
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

    fn init_git(path: &std::path::Path) {
        let repo = git2::Repository::init(path).expect("init");
        let sig = git2::Signature::now("Test", "t@e.com").unwrap();
        let tree_oid = {
            let tb = repo.treebuilder(None).unwrap();
            tb.write().unwrap()
        };
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    }

    #[test]
    fn boot_new_task_creates_task_branch_and_registry() {
        let _g = fleet_home_lock();
        let home = tempfile::TempDir::new().unwrap();
        let ws = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());
        init_git(ws.path());

        let boot = boot_new_task(
            NewTaskArgs {
                workspace: ws.path().to_path_buf(),
                title: "smoke task".into(),
                prompt: "do the thing".into(),
            },
            Box::new(SleepLauncher::new()),
        )
        .unwrap();

        // Task persisted to disk in Running state with a branch — the
        // deterministic orchestrator (not a master session) drives it.
        let loaded = claw_fleet_task::task::get_task(&boot.task_id).unwrap();
        assert!(matches!(
            loaded.status,
            claw_fleet_task::task::TaskStatus::Running
        ));
        assert!(loaded.task_branch.is_some());
        assert_eq!(loaded.description, "do the thing");

        // A new task has no plan yet, so boot launches the single interactive
        // planner session (P5). No master is ever spawned.
        let live = boot.host.live_sessions();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].kind, crate::local_host::SessionKind::Planner);

        // Registry has our entry pointing at the http port.
        let entry = registry::read(&boot.task_id).unwrap().unwrap();
        assert_eq!(entry.port, boot.http_handle.port);
        assert_eq!(entry.pid, std::process::id());

        shutdown(boot);
        assert!(registry::read(&loaded.id).unwrap().is_none());
    }

    #[test]
    fn boot_new_task_errors_when_workspace_missing() {
        let _g = fleet_home_lock();
        let home = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        let result = boot_new_task(
            NewTaskArgs {
                workspace: PathBuf::from("/nope/does/not/exist"),
                title: "x".into(),
                prompt: "y".into(),
            },
            Box::new(SleepLauncher::new()),
        );
        let err = match result {
            Ok(_) => panic!("expected workspace-missing error"),
            Err(e) => e,
        };
        assert!(err.contains("workspace"));
    }

    #[test]
    fn lookup_project_id_matches_workspace_path() {
        let _g = fleet_home_lock();
        let home = tempfile::TempDir::new().unwrap();
        let ws = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        let projects_dir = home.path().join(".claude").join("fleet");
        std::fs::create_dir_all(&projects_dir).unwrap();
        // Build via serde so the workspace path is JSON-escaped — on Windows it
        // contains backslashes that would otherwise form invalid JSON escapes.
        let projects_json = serde_json::to_string(&serde_json::json!([
            {"id": "proj-uuid-1", "name": "x", "workspace": ws.path()}
        ]))
        .unwrap();
        std::fs::write(projects_dir.join("projects.json"), projects_json).unwrap();

        let found = lookup_project_id(ws.path());
        assert_eq!(found.as_deref(), Some("proj-uuid-1"));
    }

    #[test]
    fn lookup_project_id_returns_none_when_workspace_not_in_projects() {
        let _g = fleet_home_lock();
        let home = tempfile::TempDir::new().unwrap();
        let ws = tempfile::TempDir::new().unwrap();
        let other = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        let projects_dir = home.path().join(".claude").join("fleet");
        std::fs::create_dir_all(&projects_dir).unwrap();
        let projects_json = serde_json::to_string(&serde_json::json!([
            {"id": "proj-other", "name": "x", "workspace": other.path()}
        ]))
        .unwrap();
        std::fs::write(projects_dir.join("projects.json"), projects_json).unwrap();

        assert!(lookup_project_id(ws.path()).is_none());
    }

    #[test]
    fn lookup_project_id_returns_none_when_projects_file_missing() {
        let _g = fleet_home_lock();
        let home = tempfile::TempDir::new().unwrap();
        let ws = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());
        assert!(lookup_project_id(ws.path()).is_none());
    }

    #[test]
    fn derive_auto_project_id_is_stable_and_workspace_scoped() {
        let ws_a = tempfile::TempDir::new().unwrap();
        let ws_b = tempfile::TempDir::new().unwrap();

        let a1 = derive_auto_project_id(ws_a.path());
        let a2 = derive_auto_project_id(ws_a.path());
        let b = derive_auto_project_id(ws_b.path());

        // Same workspace → same id across calls (so the desktop groups all
        // tasks of one workspace under a single virtual project).
        assert_eq!(a1, a2);
        // Different workspaces → different ids (each surfaces separately).
        assert_ne!(a1, b);
        // Format contract relied on by the discovery layer.
        assert!(a1.starts_with("auto-"), "got {a1}");
        assert_eq!(a1.len(), "auto-".len() + 16);
        // Never the legacy collapse-all bucket for a real, canonicalisable dir.
        assert_ne!(a1, FLEET_TASK_LOCAL_PROJECT);
    }
}
