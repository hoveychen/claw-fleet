//! Lifecycle of the `dsh web` server Fleet talks to over [`crate::dsh_client`].
//!
//! Unlike Claude Code and Codex — whose sessions are files on disk Fleet reads
//! directly — dsh exposes its sessions only through a running server. Fleet
//! therefore owns that process: it starts one `dsh web` per workspace root,
//! learns the port the OS assigned it, health-checks it, and kills it on exit.
//!
//! The server is deliberately **not** detached. `dsh web` has no authentication
//! layer (only a Host-header loopback fence), so a stray instance surviving
//! Fleet would leave an unauthenticated port open that can read every session
//! and start new ones. Its lifetime is bound to ours through [`Drop`].
//!
//! ## Why [`Drop`] is not enough, and what the registry adds
//!
//! Two exit paths run no Rust code at all — `SIGKILL`, and a panic that aborts —
//! and one more skips destructors by construction: the server lives in a
//! `static` ([`crate::dsh_source`]'s `SERVER`), and statics are never dropped at
//! process exit. On any of those the child simply keeps running, because a
//! parent's death does not kill its children on either platform.
//!
//! So ownership is also recorded on disk, at `~/.fleet/dsh-server.json`: every
//! live server is one record pairing the *server* process with the *Fleet
//! process that owns it*, both as [`HolderEntry`]s (pid plus start time, so pid
//! reuse cannot fool the liveness check). [`reap_orphans`] walks that file and
//! kills any server whose owner is gone — which is what makes the next Fleet
//! start, or the next `dsh` use, clean up after the previous crash.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::atomic_json::JsonLoad;
use crate::dsh_client::DshClient;
use crate::session::{prune_dead_holders, HolderEntry};

/// How long to wait for the server to print its listen URL. A cold profile
/// materializes its plugin tree on first launch, so this is generous.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to keep retrying the health endpoint after the URL appears. The
/// port is printed by the launcher shell, which can win the race against the
/// listener.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// The endpoint Fleet polls to decide the server is answering RPC.
///
/// Any cheap side-effect-free call would do; this one is picked because it
/// touches no session state. It is *not* `host.describe` any more — dsh 0.1.2
/// dropped the whole `host.*` service, so that call now 404s where it used to
/// be the readiness signal.
const HEALTH_ENDPOINT: &str = "settings/describe";

/// The oldest dsh Fleet can actually drive.
///
/// Not a policy choice — every integration point here was written against the
/// 0.1.2 wire contract, and older builds fail at a different layer each:
/// `≤0.1.0-rc.7` rejects [`web_args`]' `--no-open` as an unknown option,
/// `0.1.1` starts but prints no `?token=` for [`parse_launch_line`] to take,
/// and before 0.1.2 the `{args:…}` gateway envelope and the paged
/// `session/events` read were different shapes again
/// ([`crate::dsh_client`], [`crate::dsh_source`]). Checking the version once
/// up front turns three unrelated failures — all of which surface as "dsh web
/// exited before reporting a port" — into one actionable message.
pub const MIN_VERSION: &str = "0.1.2";

/// Does `version` (a `--version` token like `0.1.2-rc.1`) meet [`MIN_VERSION`]?
///
/// **Prerelease tags are ignored, deliberately.** Under strict semver
/// `0.1.2-rc.1 < 0.1.2`, but the published stream is still on rc tags and
/// `0.1.2-rc.1` is the build every integration point here was verified
/// against — a strict-semver floor would reject the only working version
/// there is. So only the numeric `major.minor.patch` core is compared.
///
/// `None` in, `true` out: a version we could not read is not evidence of an
/// old binary (the probe times out on a cold npm profile), and refusing to
/// launch on a failed probe would turn a slow machine into a broken one.
pub fn meets_min_version(version: Option<&str>) -> bool {
    let Some(found) = version.and_then(numeric_core) else {
        return true;
    };
    found >= numeric_core(MIN_VERSION).unwrap_or_default()
}

/// `"0.1.2-rc.1"` → `[0, 1, 2]`: leading numeric segments only, stopping at
/// the first one carrying a non-numeric tail. Shorter is smaller, which is
/// what `Vec<u32>`'s lexicographic `Ord` already gives us (`[0,1] < [0,1,2]`).
///
/// `None` when there is no leading numeric segment at all — `parse_version` in
/// [`crate::claude_binary`] is the strict sibling of this: it rejects any
/// non-numeric segment outright, because Claude's versions never carry tags.
fn numeric_core(v: &str) -> Option<Vec<u32>> {
    let mut nums = Vec::new();
    for part in v.split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        let Ok(n) = digits.parse::<u32>() else { break };
        nums.push(n);
        if digits.len() != part.len() {
            // "2-rc" — took the 2, and everything past it is a prerelease tag.
            break;
        }
    }
    (!nums.is_empty()).then_some(nums)
}

/// Locate the `dsh` executable.
///
/// Scans the augmented PATH — the process PATH plus every dir an `npm i -g`
/// binary can land in (homebrew, `/usr/local`, `~/.npm-global`, `~/.local`,
/// nvm / fnm / volta, and the wizard's own `~/.fleet/node`). Not a plain
/// `which`: a GUI app's PATH is only the system dirs, so a dsh installed
/// under a version-managed Node would be invisible to it even though the
/// wizard had just installed it through the official channel
/// ([`crate::harness_install`] locates `npm` across the same dirs, and the two
/// must agree about what "installed" means).
///
/// Then the npx cache. dsh's own README says to run
/// `npx @deepseek-ai/dsh web`, so plenty of machines have a working dsh that
/// was never installed globally: `npx` puts nothing on PATH, it unpacks the
/// package at `<npm cache>/_npx/<content hash>/node_modules/.bin/dsh`. The
/// hash is unpredictable but the directory holding the hashes is not, so
/// those bin dirs are enumerable — see [`npx_cache_bin_dirs`].
///
/// What Fleet still refuses is `npx` as a *launcher*: shelling out to
/// `npx @deepseek-ai/dsh web` on a cold cache downloads ~300 MB before it
/// serves anything, turning "start a session" into a multi-minute stall with
/// no way to report progress. Reading a cache that already exists costs one
/// `read_dir` and downloads nothing.
pub fn discover() -> Option<PathBuf> {
    // Explicit override, same escape hatch `claude_binary` gives for a Claude
    // install Fleet cannot find. Also the only way to point at an `npx`-cached
    // copy, which is how this source is tested on a machine without a global
    // install.
    if let Some(p) = std::env::var_os("FLEET_DSH_BIN") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    crate::session_launch::find_in_dirs(&search_dirs(), binary_names())
}

/// Filenames an installed dsh can carry. npm's global bin on Windows is a
/// generated `dsh.cmd` shim next to the extensionless shell script; ordering
/// puts the runnable one first.
fn binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["dsh.cmd", "dsh.exe", "dsh"]
    }
    #[cfg(not(windows))]
    {
        &["dsh"]
    }
}

/// The directories [`discover`] scans, in order: real installs first, npx
/// cache last — a globally installed dsh is the one the user maintains, an
/// npx copy is whatever version they happened to run once.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = crate::session_launch::augmented_path_dirs();
    dirs.extend(npx_cache_bin_dirs());
    dirs
}

/// npm's cache root: `$npm_config_cache` when set, else the platform default
/// (`~/.npm` on unix, `%LocalAppData%\npm-cache` on Windows). Read rather than
/// asked for — `npm config get cache` is a Node startup per probe, and this
/// runs on every wizard-panel open.
fn npm_cache_root() -> Option<PathBuf> {
    for key in ["npm_config_cache", "NPM_CONFIG_CACHE"] {
        if let Some(v) = std::env::var_os(key) {
            if !v.is_empty() {
                return Some(PathBuf::from(v));
            }
        }
    }
    let home = crate::session::real_home_dir()?;
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        Some(local.join("npm-cache"))
    }
    #[cfg(not(windows))]
    {
        Some(home.join(".npm"))
    }
}

/// Every `_npx/<hash>/node_modules/.bin` that actually holds a dsh, newest
/// cache entry first.
///
/// Filtering by "contains dsh" here rather than letting the caller probe each
/// dir keeps unrelated cache entries (there are dozens on a busy machine) out
/// of the search list, and the mtime ordering means a user who re-ran `npx`
/// after a dsh release gets the copy they just used, not the stalest one.
fn npx_cache_bin_dirs() -> Vec<PathBuf> {
    let Some(root) = npm_cache_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root.join("_npx")) else {
        return Vec::new();
    };

    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path().join("node_modules").join(".bin"))
        .filter(|bin| binary_names().iter().any(|n| bin.join(n).is_file()))
        .map(|bin| {
            let mtime = std::fs::metadata(&bin)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (mtime, bin)
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Is dsh installed on this machine?
pub fn is_available() -> bool {
    discover().is_some()
}

/// The argv Fleet always launches the server with, after the binary itself.
///
/// One place, because two other things read it back: [`sweep_unregistered_orphans`]
/// matches leaked servers by this exact command line, and the startup contract
/// (`--port 0`, so the OS assigns the port) is what makes [`parse_launch_line`]
/// the only way to learn it.
///
/// `--no-open` last, so the sweep's `dsh web --port 0` signature still matches.
/// Without it `dsh web` hands its URL to the default browser on startup — dsh's
/// own default for a human running it in a terminal, but wrong here: Fleet
/// drives this server over RPC and renders its sessions in its own UI, so every
/// Fleet start (and every crash restart) popped a browser tab nobody asked for.
/// The flag arrives in dsh-web-app 0.1.0-rc.8 (verified against the published
/// tarballs); on 0.1.0-rc.7 and older it is an unknown option — those versions
/// never opened a browser either, and Fleet no longer supports them.
fn web_args() -> &'static [&'static str] {
    &["web", "--port", "0", "--no-open"]
}

/// Extract the listening port and launch token from one launcher stdout line.
///
/// `dsh web` prints exactly
/// `dsh web: http://127.0.0.1:<port>/?token=<launch token>` once the server is
/// up. With `--port 0` that port is OS-assigned, so parsing this line is the
/// only way to learn it — which is also why Fleet uses `--port 0`: it never has
/// to guess a free port or collide with another instance.
///
/// The token half is just as load-bearing and just as unrecoverable: dsh mints
/// it per process with `randomBytes` and never writes it anywhere, so this line
/// is its only exit. Miss it and every `/api` call is 401. That is why both
/// halves are required here — a line carrying a port but no token means we are
/// talking to a dsh older than 0.1.2, and reporting that as a healthy start
/// would only defer the failure to the first call.
fn parse_launch_line(line: &str) -> Option<(u16, String)> {
    let rest = line.split("http://127.0.0.1:").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let port: u16 = digits.parse().ok()?;
    let token: String = rest
        .split("?token=")
        .nth(1)?
        .trim()
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '&')
        .collect();
    (!token.is_empty()).then_some((port, token))
}

// ── Cross-process ownership registry ────────────────────────────────────────

/// Where the ownership records live, under `~/.fleet`.
const REGISTRY_FILE: &str = "dsh-server.json";

/// One `dsh web` a Fleet process on this machine started and has not stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerRecord {
    /// The `dsh web` launcher process.
    server: HolderEntry,
    /// The Fleet process that started it and is responsible for stopping it.
    owner: HolderEntry,
    /// The port it reported. Diagnostics only — the killer works by pid.
    port: u16,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    servers: Vec<ServerRecord>,
}

fn registry_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(REGISTRY_FILE))
}

/// Is the process behind this entry still the one that was recorded?
///
/// Delegates to [`prune_dead_holders`] rather than re-deriving the check, so the
/// pid-reuse defence (a live pid whose start time no longer matches is *not* the
/// same process) stays defined in exactly one place.
fn is_live(entry: &HolderEntry) -> bool {
    let mut one = vec![entry.clone()];
    prune_dead_holders(&mut one);
    !one.is_empty()
}

/// Read-modify-write the registry under an exclusive cross-process lock.
///
/// Returns `None` when the file could not be edited safely — either there is no
/// home directory, or the file exists but is unreadable this instant, in which
/// case overwriting it would drop records belonging to *other* live Fleet
/// processes.
fn edit_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> Option<R> {
    let path = registry_path()?;
    crate::atomic_json::with_file_lock(&path, || {
        let mut registry: Registry = match crate::atomic_json::load_preserving(&path) {
            JsonLoad::Loaded(r) => r,
            // Corrupt bytes have already been renamed aside for recovery.
            JsonLoad::Missing | JsonLoad::Corrupt => Registry::default(),
            JsonLoad::Unreadable => {
                crate::log_debug("dsh registry: unreadable, skipping this edit");
                return None;
            }
        };
        let out = f(&mut registry);
        match serde_json::to_vec_pretty(&registry) {
            Ok(bytes) => {
                if let Err(e) = crate::atomic_json::write_atomic(&path, &bytes) {
                    crate::log_debug(&format!("dsh registry: write: {e}"));
                }
            }
            Err(e) => crate::log_debug(&format!("dsh registry: serialize: {e}")),
        }
        Some(out)
    })
}

/// Record that this process owns the `dsh web` at `server_pid`.
///
/// Also drops records whose server has since died, so a machine that has run
/// Fleet for weeks does not accumulate them.
fn register(server_pid: u32, port: u16) {
    let record = ServerRecord {
        server: HolderEntry::capture(server_pid),
        owner: HolderEntry::capture(std::process::id()),
        port,
    };
    edit_registry(|registry| {
        registry.servers.retain(|r| is_live(&r.server));
        registry.servers.push(record);
    });
}

/// Drop the record for `server_pid`. Called when Fleet stops a server itself,
/// so the registry only ever describes servers that are still running.
fn deregister(server_pid: u32) {
    edit_registry(|registry| {
        registry.servers.retain(|r| r.server.pid != server_pid);
    });
}

/// Kill every recorded `dsh web` whose owning Fleet process is gone, and return
/// how many were killed.
///
/// This is the crash-recovery path: an owner that died without running
/// [`crate::dsh_source::shutdown`] left an unauthenticated server listening, and
/// nothing else on the machine will ever reclaim it. Safe to call at any time —
/// a server whose owner is still alive belongs to a running Fleet (possibly this
/// one) and is left strictly alone.
pub fn reap_orphans() -> usize {
    edit_registry(|registry| {
        let mut killed = 0;
        registry.servers.retain(|record| {
            if !is_live(&record.server) {
                // Already gone; the record is just stale bookkeeping.
                return false;
            }
            if is_live(&record.owner) {
                return true;
            }
            crate::log_debug(&format!(
                "dsh registry: reaping orphaned dsh web pid={} port={} (owner {} is gone)",
                record.server.pid, record.port, record.owner.pid
            ));
            crate::llm_provider::kill_process(record.server.pid);
            killed += 1;
            false
        });
        killed
    })
    .unwrap_or(0)
}

/// Kill leaked `dsh web` servers the registry never heard of.
///
/// The registry is blind to a whole class of leaks: a server spawned under a
/// throwaway `FLEET_HOME` (tests, harness scripts) records itself into a temp
/// registry that evaporates with the run, and a one-shot CLI whose record was
/// dropped without a kill leaves nothing behind either. The processes are
/// still identifiable: Fleet always launches `dsh web --port 0` (an
/// OS-assigned port is the spawn contract) and always keeps the server as a
/// direct child — so a `… dsh web --port 0` whose parent died (ppid 1) is
/// ownerless by construction, whoever started it. 13 such invisible orphans
/// accumulated on 2026-08-18 alone; this sweep is what makes the leak class
/// self-healing instead of hand-cleaned.
///
/// A sibling of [`reap_orphans`], not part of it: the registry reap has
/// count-exact unit tests and runs under the registry lock, while this sweep
/// is machine-global and non-deterministic there (it would also cross-kill
/// other tests' fixtures). Production calls both, side by side, at the one
/// place a server is first started (`DshSource::with_client`).
pub fn sweep_unregistered_orphans() -> usize {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sys = System::new();
    // cmd only, no cwd: refreshing cwd for every process on macOS triggers TCC
    // consent dialogs for unrelated apps (see `scan_codex_processes`).
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );

    let mut killed = 0;
    for (pid, process) in sys.processes() {
        let cmd = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        if !cmd.contains("dsh web --port 0") {
            continue;
        }
        // Parent alive = that process owns it (Fleet keeps its server as a
        // direct child); only the reparented-to-init ones are ownerless.
        let orphaned = process.parent().map(|p| p.as_u32() == 1).unwrap_or(false);
        if !orphaned {
            continue;
        }
        crate::log_debug(&format!(
            "dsh sweep: killing registry-invisible orphaned dsh web pid={pid} ({cmd})"
        ));
        crate::llm_provider::kill_process(pid.as_u32());
        killed += 1;
    }
    killed
}

/// A running `dsh web` instance owned by this process.
pub struct DshServer {
    child: Child,
    port: u16,
    /// The per-process launch token read off the announcement line. Every
    /// [`DshClient`] trades it for the session cookie `/api` demands, so it has
    /// to live exactly as long as the child that minted it — a restart mints a
    /// new one, alongside a new port.
    launch_token: String,
    binary: PathBuf,
    workspace: PathBuf,
}

impl DshServer {
    /// Start a server rooted at `workspace` and wait until it answers RPC.
    ///
    /// The invoking directory is dsh's default workspace root, so `workspace`
    /// decides which project new sessions belong to.
    pub fn start(binary: &Path, workspace: &Path) -> Result<Self, String> {
        if !workspace.is_dir() {
            return Err(format!("workspace does not exist: {}", workspace.display()));
        }

        let mut cmd = crate::process_util::command(binary);
        cmd.args(web_args())
            .current_dir(workspace)
            // `dsh` is a `#!/usr/bin/env node` script, so starting it needs
            // `node` on PATH — not just the script itself, which `discover()`
            // already finds by absolute path. A Tauri app launched by launchd
            // carries `/usr/bin:/bin:/usr/sbin:/sbin`, where there is no node, so
            // the child died instantly with `env: node: No such file or
            // directory` and Fleet only saw "exited before reporting a port" —
            // retried every poll, forever. Every dsh live test missed it because
            // `cargo test` inherits a developer shell's PATH.
            //
            // Same fix, same helper, as the Claude and Codex spawn paths
            // (`session_launch.rs` / `codex_launch.rs`); this was the one spawn
            // site that had not been wired to it.
            .env("PATH", crate::session_launch::augmented_path_with_front(&[]))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", binary.display()))?;

        let (port, launch_token) = match read_launch_line(&mut child) {
            Ok(parsed) => parsed,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };

        let mut server = Self {
            child,
            port,
            launch_token,
            binary: binary.to_path_buf(),
            workspace: workspace.to_path_buf(),
        };

        if let Err(e) = server.wait_healthy() {
            server.stop();
            return Err(e);
        }

        // Only a healthy server is worth recording: one that never answered has
        // already been killed above, and a record for it would just be noise the
        // next `reap_orphans` has to clear.
        register(server.pid(), server.port);
        Ok(server)
    }

    /// The OS-assigned port this instance listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The launcher process id.
    ///
    /// dsh has no per-session process — every session's turn runs inside this
    /// one server — so this is the only pid Fleet can report for a dsh session,
    /// and it is shared by all of them. Killing it would take down every dsh
    /// session at once, which is why [`crate::dsh_source::DshSource`] does not
    /// implement `kill_pid`.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The workspace root this instance was started in.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// An RPC client pointed at this instance.
    pub fn client(&self) -> Result<DshClient, String> {
        DshClient::new(self.port, &self.launch_token).map_err(Into::into)
    }

    /// The launch token this instance announced. Anything that builds its own
    /// [`DshClient`] (the decision bridge runs on its own thread) needs it —
    /// there is no way to re-derive it from the port.
    pub fn launch_token(&self) -> &str {
        &self.launch_token
    }

    /// Has the process exited? Reaps it when it has, so a crashed server does
    /// not linger as a zombie until Fleet quits.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Restart after a crash, replacing the child and the port.
    ///
    /// The port changes: the old one was OS-assigned and the new listener gets
    /// its own, so every cached [`DshClient`] must be rebuilt from [`client`].
    ///
    /// [`client`]: Self::client
    pub fn restart(&mut self) -> Result<(), String> {
        self.stop();
        let mut fresh = Self::start(&self.binary, &self.workspace)?;
        // Swap the handles rather than moving out of `fresh` (this type has a
        // Drop impl, so it cannot be destructured). After the swap `fresh` owns
        // the already-reaped dead child, and its Drop is a no-op.
        std::mem::swap(&mut self.child, &mut fresh.child);
        self.port = fresh.port;
        // The fresh child minted its own token; the old one dies with the old
        // process, so keeping it would 401 every call after a restart.
        self.launch_token = std::mem::take(&mut fresh.launch_token);
        Ok(())
    }

    /// Restart only if the process is gone. Cheap enough to call per poll.
    pub fn ensure_alive(&mut self) -> Result<(), String> {
        if self.is_alive() {
            return Ok(());
        }
        self.restart()
    }

    /// Terminate the server and reap it. Idempotent.
    pub fn stop(&mut self) {
        let pid = self.child.id();
        let _ = self.child.kill();
        let _ = self.child.wait();
        // After the wait, so the record never outlives a process this call has
        // already reaped — and never describes one still shutting down.
        deregister(pid);
    }

    /// Poll `host.describe` until it answers or the health budget runs out.
    fn wait_healthy(&mut self) -> Result<(), String> {
        let client = self.client()?;
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        let mut last = String::new();
        while Instant::now() < deadline {
            if !self.is_alive() {
                return Err("dsh web exited during startup".into());
            }
            match client.call(HEALTH_ENDPOINT, serde_json::json!({})) {
                Ok(_) => return Ok(()),
                Err(e) => last = e.to_string(),
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(format!("dsh web never answered {HEALTH_ENDPOINT}: {last}"))
    }
}

impl Drop for DshServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Read the launcher's stdout on a helper thread until it prints its URL.
///
/// The read must not happen on this thread: a server that fails to start (bad
/// profile, port refused, missing artifacts) prints nothing and never closes
/// stdout, so a direct `read_line` would block forever instead of timing out.
fn read_launch_line(child: &mut Child) -> Result<(u16, String), String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "dsh web produced no stdout pipe".to_string())?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(parsed) = parse_launch_line(&line) {
                let _ = tx.send(parsed);
                return;
            }
        }
        // stdout closed without a URL — let the receiver fail on disconnect
        // rather than wait out the whole timeout.
    });

    match rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(parsed) => Ok(parsed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "dsh web did not report a port within {STARTUP_TIMEOUT:?}"
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("dsh web exited before reporting a port".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fleet can only drive dsh 0.1.2 and newer, and the floor has to accept
    /// the *prerelease* line: the published stream is still on rc tags, so
    /// `0.1.2-rc.1` is the build every integration point here was verified
    /// against. Under strict semver that sorts *below* `0.1.2`, which would
    /// reject the only working version there is — hence prerelease tags are
    /// ignored and only the numeric core is compared.
    ///
    /// An unreadable version reads as acceptable, not as too old: the probe
    /// times out on a cold npm profile, and refusing to launch on a failed
    /// probe would turn a slow machine into a broken one.
    #[test]
    fn min_version_floor_ignores_prerelease_tags() {
        // Too old — the three real failure modes documented on MIN_VERSION.
        assert!(!meets_min_version(Some("0.1.1")), "0.1.1 must be rejected");
        assert!(
            !meets_min_version(Some("0.1.1-rc.2")),
            "0.1.1-rc.2 must be rejected"
        );
        assert!(
            !meets_min_version(Some("0.1.0-rc.7")),
            "0.1.0-rc.7 must be rejected"
        );
        assert!(!meets_min_version(Some("0.0.9")), "0.0.9 must be rejected");

        // Acceptable — including the rc line that is the only shipping build.
        assert!(meets_min_version(Some("0.1.2")), "0.1.2 must be accepted");
        assert!(
            meets_min_version(Some("0.1.2-rc.1")),
            "0.1.2-rc.1 must be accepted — it is the verified build"
        );
        assert!(meets_min_version(Some("0.1.3")), "0.1.3 must be accepted");
        assert!(meets_min_version(Some("0.2.0")), "0.2.0 must be accepted");
        assert!(meets_min_version(Some("1.0.0")), "1.0.0 must be accepted");

        // Unknown is not old.
        assert!(meets_min_version(None), "a missing version must not block");
        assert!(
            meets_min_version(Some("garbage")),
            "an unparseable version must not block"
        );
        assert!(meets_min_version(Some("")), "an empty version must not block");
    }

    /// dsh installs through `npm i -g`, so it lands in whatever bin dir the
    /// active Node.js runtime owns — and when Node comes from nvm / fnm /
    /// volta, or from the environment wizard's own bootstrap under
    /// `~/.fleet/node`, that dir is none of the four classic globals. A GUI
    /// app's PATH is only the system dirs, so the `which` step misses it too:
    /// the wizard would install dsh through its official channel and then
    /// still report it as not installed.
    ///
    /// The installer side already scans the augmented PATH
    /// (`harness_install::find_in_augmented_path`); discovery must search the
    /// same set, or the two disagree about what "installed" means.
    #[test]
    fn search_dirs_cover_the_runtime_managed_node_bin_dirs() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp.path()) };

        let dirs = search_dirs();

        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }

        let home = tmp.path();
        for expected in [
            home.join(".fleet").join("node").join("bin"),
            home.join(".volta").join("bin"),
            home.join("Library/Application Support/fnm/aliases/default/bin"),
            home.join(".local/share/fnm/aliases/default/bin"),
        ] {
            assert!(
                dirs.contains(&expected),
                "discover() does not search {}; dirs = {dirs:?}",
                expected.display()
            );
        }
    }

    /// dsh's own README tells people to run `npx @deepseek-ai/dsh web`, and
    /// that works — so a user who followed it has a perfectly good dsh on the
    /// machine and no reason to suspect Fleet cannot see it. npx installs
    /// nothing on PATH, but it does leave the package unpacked at a fixed
    /// shape: `<npm cache>/_npx/<content hash>/node_modules/.bin/dsh`. The
    /// hash is unpredictable; the directory holding the hashes is not, so the
    /// bin dirs are enumerable and discovery must include them.
    ///
    /// Scanning an existing cache costs a `read_dir` and downloads nothing —
    /// it is not the `npx`-as-launcher fallback `discover` still refuses.
    #[test]
    fn search_dirs_include_an_npx_cached_dsh() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();

        // One hash dir holding dsh, one holding something else — only the
        // former may be offered, and it must be the .bin dir, not the root.
        let dsh_bin = tmp
            .path()
            .join(".npm/_npx/deadbeef00000001/node_modules/.bin");
        std::fs::create_dir_all(&dsh_bin).unwrap();
        std::fs::write(dsh_bin.join("dsh"), b"#!/usr/bin/env node\n").unwrap();
        let other_bin = tmp
            .path()
            .join(".npm/_npx/deadbeef00000002/node_modules/.bin");
        std::fs::create_dir_all(&other_bin).unwrap();
        std::fs::write(other_bin.join("tsc"), b"#!/usr/bin/env node\n").unwrap();

        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp.path()) };
        let dirs = search_dirs();
        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }

        assert!(
            dirs.contains(&dsh_bin),
            "an npx-cached dsh is not discoverable; dirs = {dirs:?}"
        );
        assert!(
            !dirs.contains(&other_bin),
            "unrelated npx cache entries must not be searched; dirs = {dirs:?}"
        );
    }

    /// The same scan against a cache a real `npx` wrote, rather than a
    /// hand-built fixture — the fixture only proves the code does what I
    /// believe npx's layout to be.
    ///
    /// Populate one and run it (a plain `npx @deepseek-ai/dsh` on a machine
    /// that already has a global dsh installs nothing — npm exec puts the npm
    /// prefix's bin on PATH and runs that, so the prefix has to be isolated
    /// too):
    ///
    /// ```text
    /// env -i HOME=/tmp/fh PATH=/tmp/nodebin:/usr/bin:/bin \
    ///   npm_config_cache=/tmp/npxcache npm_config_prefix=/tmp/fp \
    ///   npx -y @deepseek-ai/dsh --version
    /// npm_config_cache=/tmp/npxcache cargo test -p claw-fleet-core --lib \
    ///   -- --ignored live_finds_a_real_npx_cached_dsh --nocapture
    /// ```
    #[test]
    #[ignore = "needs an npm cache a real npx populated; set npm_config_cache"]
    fn live_finds_a_real_npx_cached_dsh() {
        let dirs = npx_cache_bin_dirs();
        println!("npx cache bin dirs: {dirs:?}");
        let bin = dirs
            .first()
            .map(|d| d.join("dsh"))
            .expect("no npx-cached dsh found — is npm_config_cache pointing at a populated cache?");
        assert!(bin.is_file(), "{} is not a file", bin.display());
        assert!(
            bin.to_string_lossy().contains("_npx"),
            "{} is not in the npx cache",
            bin.display()
        );
    }

    /// The registry is blind to servers spawned under a throwaway `FLEET_HOME`
    /// (tests, harness scripts): their records evaporate with the temp dir and
    /// the real `dsh web` lives on forever — 13 such invisible orphans piled up
    /// on 2026-08-18 alone. `reap_orphans` must therefore ALSO sweep by
    /// signature: any `… dsh web --port 0` process whose parent died (ppid 1)
    /// is ownerless by construction — Fleet always keeps its server as a
    /// direct child — and gets killed even when no registry record names it.
    #[cfg(unix)]
    #[test]
    fn reap_sweeps_a_registry_invisible_orphan_by_signature() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        // A fake `dsh` that just sleeps, so the sweep's target exists without
        // touching a real dsh install.
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("dsh");
        {
            let mut f = std::fs::File::create(&fake).unwrap();
            writeln!(f, "#!/bin/sh\nsleep 300").unwrap();
        }
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Double-fork through `sh`: the intermediate shell prints the child's
        // pid and exits, so the fake server reparents to pid 1 — the exact
        // shape of a leaked orphan.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "\"{}\" web --port 0 >/dev/null 2>&1 & echo $!",
                fake.display()
            ))
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();

        // Wait until it is actually orphaned (ppid == 1).
        let orphaned = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            std::process::Command::new("ps")
                .args(["-o", "ppid=", "-p", &pid.to_string()])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
                .unwrap_or(false)
        });
        assert!(orphaned, "fake dsh web (pid {pid}) never reparented to 1");

        sweep_unregistered_orphans();

        // SIGKILL delivery is asynchronous; give it a moment.
        let dead = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            unsafe { libc::kill(pid as i32, 0) != 0 }
        });
        // Clean up on failure so a red run doesn't itself leak the fake.
        if !dead {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        assert!(
            dead,
            "reap_orphans left the registry-invisible orphan (pid {pid}) alive"
        );
    }

    /// `dsh web` hands the URL to the default browser unless told not to
    /// ("dsh web: opening the default browser; pass --no-open to disable",
    /// dsh-web-app 0.1.0-rc.8+). Fleet drives this server over RPC and renders
    /// dsh sessions in its own UI, so every Fleet start — and every crash
    /// restart — was popping a browser tab nobody asked for.
    #[test]
    fn launches_the_server_without_a_browser_handoff() {
        assert!(
            web_args().contains(&"--no-open"),
            "dsh web would open a browser tab on every Fleet start: {:?}",
            web_args()
        );
    }

    /// The sweep matches leaked servers by command line, so the flag must not
    /// break that signature by landing between `web` and `--port 0`.
    #[test]
    fn keeps_the_orphan_sweep_signature() {
        assert!(
            web_args().join(" ").starts_with("web --port 0"),
            "sweep_unregistered_orphans matches `dsh web --port 0`: {:?}",
            web_args()
        );
    }

    #[test]
    fn parses_the_launcher_url_line() {
        // Verbatim from a live `dsh web --port 0` run on 0.1.2-rc.1.
        assert_eq!(
            parse_launch_line(
                "dsh web: http://127.0.0.1:51813/?token=K2--JSbxHKelXA2nUsP43zD4GORrzMrzoKd1cnB_NVg"
            ),
            Some((
                51813,
                "K2--JSbxHKelXA2nUsP43zD4GORrzMrzoKd1cnB_NVg".to_string()
            ))
        );
    }

    #[test]
    fn tolerates_trailing_text_after_the_token() {
        assert_eq!(
            parse_launch_line("dsh web: http://127.0.0.1:3080/?token=abc123 (press ctrl-c to stop)"),
            Some((3080, "abc123".to_string()))
        );
    }

    /// dsh 0.1.1 printed the bare URL. Parsing that as a healthy start would
    /// leave `launch_token` empty and turn every later call into a 401, so the
    /// line is rejected here — where the error still names the startup step.
    #[test]
    fn rejects_a_line_without_a_token() {
        assert_eq!(parse_launch_line("dsh web: http://127.0.0.1:63234"), None);
        assert_eq!(parse_launch_line("dsh web: http://127.0.0.1:3080/"), None);
        assert_eq!(
            parse_launch_line("dsh web: http://127.0.0.1:3080/?token="),
            None
        );
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert_eq!(parse_launch_line("npm warn deprecated foo@1.0.0"), None);
        assert_eq!(parse_launch_line(""), None);
        // A non-loopback URL is not ours to talk to: the /api fence would 403 it.
        assert_eq!(
            parse_launch_line("serving http://0.0.0.0:3080/?token=abc"),
            None
        );
    }

    #[test]
    fn rejects_a_port_that_is_not_a_number() {
        assert_eq!(
            parse_launch_line("dsh web: http://127.0.0.1:abc/?token=xyz"),
            None
        );
    }

    // ── Registry ────────────────────────────────────────────────────────────

    /// Point `~/.fleet` at a throwaway directory for the body of `f`.
    ///
    /// Takes the same process-wide lock every other env-mutating test in this
    /// crate takes, because `FLEET_HOME` is global state.
    fn with_temp_fleet_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let base = std::env::temp_dir().join(format!(
            "fleet-dsh-registry-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &base);
        let out = f(&base);
        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        let _ = std::fs::remove_dir_all(&base);
        out
    }

    fn read_registry() -> Registry {
        let path = registry_path().unwrap();
        match crate::atomic_json::load_preserving::<Registry>(&path) {
            JsonLoad::Loaded(r) => r,
            _ => Registry::default(),
        }
    }

    /// A pid that is certainly not a live process, paired with a start time no
    /// live process could match. `u32::MAX` is above every platform's pid_max.
    fn dead() -> HolderEntry {
        HolderEntry {
            pid: u32::MAX,
            start_time_secs: 1,
        }
    }

    /// The registry has to survive being written by one process and read by
    /// another, so the on-disk shape is part of the contract.
    #[test]
    fn a_record_round_trips_through_its_file() {
        with_temp_fleet_home(|_| {
            register(std::process::id(), 51234);
            let registry = read_registry();
            assert_eq!(registry.servers.len(), 1);
            assert_eq!(registry.servers[0].port, 51234);
            assert_eq!(registry.servers[0].server.pid, std::process::id());
            assert_eq!(registry.servers[0].owner.pid, std::process::id());
            // The start times must be captured, not left at the "unknown"
            // sentinel — a 0 there disarms the pid-reuse defence.
            assert_ne!(registry.servers[0].server.start_time_secs, 0);
        });
    }

    #[test]
    fn deregister_removes_only_its_own_record() {
        with_temp_fleet_home(|_| {
            register(std::process::id(), 1);
            edit_registry(|r| {
                r.servers.push(ServerRecord {
                    server: dead(),
                    owner: HolderEntry::capture(std::process::id()),
                    port: 2,
                })
            });
            deregister(std::process::id());
            let left = read_registry();
            assert_eq!(left.servers.len(), 1);
            assert_eq!(left.servers[0].port, 2);
        });
    }

    /// The whole point: a server whose owner is gone gets killed. Here the
    /// "server" is a real sleeping child, so the kill is observable.
    #[test]
    fn an_orphan_whose_owner_died_is_killed() {
        with_temp_fleet_home(|_| {
            let mut child = crate::process_util::command("sleep")
                .arg("30")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn a stand-in server");
            let victim = child.id();

            edit_registry(|r| {
                r.servers.push(ServerRecord {
                    server: HolderEntry::capture(victim),
                    owner: dead(),
                    port: 4321,
                })
            });

            assert_eq!(reap_orphans(), 1, "the orphan should have been reaped");
            assert!(
                read_registry().servers.is_empty(),
                "its record must go with it"
            );
            // `wait` returns only because the process was signalled; a stale
            // record that killed nothing would leave this blocked for 30s.
            let status = child.wait().expect("wait on the stand-in");
            assert!(!status.success(), "expected a signalled exit, got {status:?}");
        });
    }

    /// A server whose owner is still running belongs to a live Fleet — possibly
    /// this very one. Reaping it would take down a working app.
    #[test]
    fn a_server_with_a_live_owner_is_left_alone() {
        with_temp_fleet_home(|_| {
            // Owner and server are both this process: alive by construction.
            register(std::process::id(), 7777);
            assert_eq!(reap_orphans(), 0);
            assert_eq!(read_registry().servers.len(), 1);
        });
    }

    /// A record for a server that is already gone is bookkeeping, not an
    /// orphan: dropping it must not be counted as a kill.
    #[test]
    fn a_record_for_a_dead_server_is_dropped_without_counting_as_a_kill() {
        with_temp_fleet_home(|_| {
            edit_registry(|r| {
                r.servers.push(ServerRecord {
                    server: dead(),
                    owner: dead(),
                    port: 1,
                })
            });
            assert_eq!(reap_orphans(), 0);
            assert!(read_registry().servers.is_empty());
        });
    }

    /// A pid recycled onto a different process must not be mistaken for the
    /// recorded one — otherwise a reap could kill an unrelated program.
    #[test]
    fn a_recycled_pid_is_not_treated_as_the_recorded_process() {
        with_temp_fleet_home(|_| {
            // This pid is alive, but the recorded start time is not its own.
            let impostor = HolderEntry {
                pid: std::process::id(),
                start_time_secs: 1,
            };
            assert!(!is_live(&impostor));
            edit_registry(|r| {
                r.servers.push(ServerRecord {
                    server: impostor,
                    owner: dead(),
                    port: 1,
                })
            });
            assert_eq!(
                reap_orphans(),
                0,
                "a mismatched start time means the recorded process is gone"
            );
        });
    }

    /// The real lifecycle registers on start and deregisters on stop, so a
    /// tidy exit leaves nothing behind for [`reap_orphans`] to consider.
    ///
    ///   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
    ///   cargo test -p claw-fleet-core --lib dsh_server -- --ignored --nocapture
    #[test]
    #[ignore = "starts a real `dsh web`; run manually with --ignored"]
    fn live_a_started_server_is_registered_and_a_stopped_one_is_not() {
        let binary = discover().expect("set FLEET_DSH_BIN to a dsh executable");
        with_temp_fleet_home(|base| {
            let mut server = DshServer::start(&binary, base).expect("start dsh web");
            let registry = read_registry();
            assert_eq!(registry.servers.len(), 1, "start must register");
            assert_eq!(registry.servers[0].server.pid, server.pid());
            assert_eq!(registry.servers[0].port, server.port());

            server.stop();
            assert!(
                read_registry().servers.is_empty(),
                "stop must deregister, or the next reap would chase a dead pid"
            );
        });
    }

    /// The unit tests above stand a `sleep` in for the server. This one reaps a
    /// real `dsh web` in the real shape of the bug: the process is started so
    /// that its parent exits immediately, leaving it orphaned onto init exactly
    /// as a SIGKILLed Fleet would.
    ///
    /// It is deliberately *not* started through [`DshServer`]: that would make
    /// this test process the parent, and a killed child of a live parent stays a
    /// zombie until someone waits on it — `kill(pid, 0)` still answers "alive"
    /// for a zombie, so the assertion would fail on an artefact of the test
    /// rather than on anything production can hit (a reap target's owner is
    /// dead by definition, so its server has already been re-parented).
    ///
    ///   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
    ///   cargo test -p claw-fleet-core --lib dsh_server -- --ignored --nocapture
    #[test]
    #[ignore = "starts a real `dsh web`; run manually with --ignored"]
    fn live_an_orphaned_dsh_web_is_reclaimed() {
        let binary = discover().expect("set FLEET_DSH_BIN to a dsh executable");
        with_temp_fleet_home(|_| {
            let launched = crate::process_util::command("sh")
                .arg("-c")
                .arg(format!(
                    "'{}' web --port 0 >/dev/null 2>&1 & echo $!",
                    binary.display()
                ))
                .output()
                .expect("launch an orphaned dsh web");
            let orphan: u32 = String::from_utf8_lossy(&launched.stdout)
                .trim()
                .parse()
                .expect("the launcher shell must print the background pid");
            assert!(
                crate::session::is_process_alive(orphan),
                "the orphan should be running"
            );

            edit_registry(|r| {
                r.servers.push(ServerRecord {
                    server: HolderEntry::capture(orphan),
                    owner: dead(),
                    port: 0,
                })
            });

            assert_eq!(reap_orphans(), 1, "the orphan should be reaped");
            let gone = (0..50).any(|_| {
                std::thread::sleep(Duration::from_millis(100));
                !crate::session::is_process_alive(orphan)
            });
            assert!(gone, "dsh web pid {orphan} survived the reap");
            assert!(read_registry().servers.is_empty());
        });
    }

    #[test]
    fn start_rejects_a_missing_workspace() {
        // `unwrap_err` would need DshServer: Debug; a running server is not a
        // thing this type should be able to print.
        let err = match DshServer::start(
            Path::new("/nonexistent/dsh"),
            Path::new("/nonexistent/workspace-dir"),
        ) {
            Err(e) => e,
            Ok(_) => panic!("a missing workspace must not start a server"),
        };
        assert!(err.contains("workspace does not exist"), "{err}");
    }
}
