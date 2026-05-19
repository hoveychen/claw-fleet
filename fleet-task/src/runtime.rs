//! Glue layer that boots a new (or resumed) task in the current process:
//! creates / loads the task json, opens the git branch, spawns the master
//! via the given `ProcessLauncher`, writes a runtime registry entry, and
//! starts the HTTP server.
//!
//! Pure of CLI / TUI concerns so the boot path is unit-testable with a
//! `SleepLauncher` against a tempdir git repo. `main.rs` only handles arg
//! parsing and the foreground wait loop.

use std::path::PathBuf;
use std::sync::Arc;

use claw_fleet_task::actions::{self, StartOpts};
use claw_fleet_task::runner::TaskLifecycleHost;
use claw_fleet_task::task::{
    create_task, get_task, write_task_atomic, TaskInput, TaskStatus,
};

use crate::http::{self, HttpHandle, ServerConfig};
use crate::local_host::{LocalHost, ProcessLauncher};
use crate::registry::{self, RegistryEntry};
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
    pub master_session_id: String,
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
    let task = create_task(TaskInput {
        project_id: FLEET_TASK_LOCAL_PROJECT.into(),
        title: args.title.clone(),
        description: args.prompt,
    })?;

    let host = Arc::new(LocalHost::with_launcher(args.workspace, launcher));
    actions::start_task(&task.id, &*host, StartOpts::default())?;

    let master_sid = get_task(&task.id)?
        .master_session_id
        .ok_or_else(|| "actions::start_task did not stamp master_session_id".to_string())?;
    finish_boot(task.id, host, master_sid)
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
    actions::start_task(&task_id, &*host, StartOpts::default())?;
    let master_sid = get_task(&task_id)?
        .master_session_id
        .ok_or_else(|| "actions::start_task did not stamp master_session_id".to_string())?;
    finish_boot(task_id, host, master_sid)
}

fn finish_boot(
    task_id: String,
    host: Arc<LocalHost>,
    master_sid: String,
) -> Result<Bootstrap, String> {
    let broadcaster = SseBroadcaster::new();
    let cfg = ServerConfig {
        task_id: task_id.clone(),
        broadcaster: broadcaster.clone(),
        dispatcher: None,
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
        master_session_id: master_sid,
    })
}

pub fn shutdown(boot: Bootstrap) {
    shutdown_with_deadline(boot, std::time::Duration::from_secs(30));
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
    use claw_fleet_task::master::MasterSpawnSpec;
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
        fn launch_master(&self, _s: &str, _spec: &MasterSpawnSpec) -> Result<u32, String> {
            self.spawn()
        }
        fn launch_worker(&self, _s: &str, _spec: &WorkerSpawnSpec) -> Result<u32, String> {
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
    fn boot_new_task_creates_task_branch_master_and_registry() {
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

        // Task persisted to disk in Running state with a branch and master sid.
        let loaded = claw_fleet_task::task::get_task(&boot.task_id).unwrap();
        assert!(matches!(
            loaded.status,
            claw_fleet_task::task::TaskStatus::Running
        ));
        assert!(loaded.task_branch.is_some());
        assert_eq!(loaded.master_session_id.as_deref(), Some(boot.master_session_id.as_str()));
        assert_eq!(loaded.description, "do the thing");

        // LocalHost has exactly one tracked session (the master).
        assert_eq!(boot.host.live_sessions().len(), 1);

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
}
