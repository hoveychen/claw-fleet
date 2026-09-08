//! Lifecycle of the `dsh web` server Fleet talks to over [`crate::dsh_client`].
//!
//! Unlike Claude Code and Codex — whose sessions are files on disk Fleet reads
//! directly — dsh exposes its sessions only through a running server. Fleet
//! therefore owns that process: it starts one machine-level `dsh web`, learns
//! the port and launch token it announced, and lets later Fleet processes adopt
//! that authenticated service without interrupting its in-flight turns.
//!
//! dsh 0.1.2 protects the API with a per-process launch token. Fleet persists
//! that token in an owner-only registry and deliberately lets the service
//! outlive a desktop/CLI process. Direct `DshServer` users still get stop-on-drop
//! cleanup unless they call [`DshServer::detach`].
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
//! live server is one record pairing the *server* process with its most recent
//! Fleet client, both as [`HolderEntry`]s (pid plus start time, so pid reuse
//! cannot fool the liveness check). The same record carries the launch token
//! needed to adopt a current server; [`reap_orphans`] kills only legacy records
//! that cannot be authenticated.

use std::collections::HashSet;
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

/// The npm command that upgrades dsh, quoted in every too-old message.
///
/// Same package and channel [`crate::harness_install::update_plan`] runs for
/// the desktop's one-click upgrade, so the text a CLI user is told to type and
/// the button the wizard offers do the same thing.
pub const UPGRADE_COMMAND: &str = "npm i -g @deepseek-ai/dsh@latest";

/// The refusal a too-old dsh gets, carrying all three things needed to act on
/// it: the version found, the version required, and the fix.
pub fn too_old_message(found: Option<&str>) -> String {
    let found = found.unwrap_or("unknown");
    format!(
        "this dsh is {found}, but Fleet needs dsh {MIN_VERSION} or newer \
         (older builds speak a different wire protocol). Upgrade with: {UPGRADE_COMMAND}"
    )
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

/// The argv Fleet launches the server with, after the binary itself.
///
/// One place, because two other things read it back: [`sweep_unregistered_orphans`]
/// matches leaked servers by this command-line shape ([`is_fleet_launch_cmd`]),
/// and whatever port dsh actually binds is learned back from
/// [`parse_launch_line`] — the only way to learn it when `port` is `0`.
///
/// `port` is [`preferred_port`]'s answer: the port the previous server on this
/// machine listened on when it is still free, else `0` for an OS-assigned one.
/// **Why a stable port matters:** dsh writes its own GUI URL,
/// `http://127.0.0.1:<port>`, into the *system prompt* of every request
/// (`dsh-web-app`'s `app:web-surface` section). A new port therefore changes
/// the first bytes of every session's prompt, and the provider's prefix cache
/// misses on the whole history: measured 2026-09-07, three sessions' next
/// turns after Fleet restarts each re-billed 80–190k tokens as uncached
/// (`cacheReadTokens` fell to 0), ~1M tokens in one evening, while the one
/// turn whose port had not changed hit 238k cached tokens. `--port 0` was
/// chosen so Fleet never had to pick a free port or collide with another
/// instance; remembering the last port and checking it is free keeps both
/// properties and adds the one that was missing.
///
/// `--no-open` last, and always. Without it `dsh web` hands its URL to the
/// default browser on startup — dsh's own default for a human running it in a
/// terminal, but wrong here: Fleet drives this server over RPC and renders its
/// sessions in its own UI, so every Fleet start (and every crash restart)
/// popped a browser tab nobody asked for. The flag arrives in dsh-web-app
/// 0.1.0-rc.8 (verified against the published tarballs); on 0.1.0-rc.7 and
/// older it is an unknown option — those versions never opened a browser
/// either, and Fleet no longer supports them. It is also half of the sweep
/// signature: a human's `dsh web --port 3080` opens a browser and carries no
/// `--no-open`, which is what keeps [`sweep_unregistered_orphans`] off servers
/// the user started by hand.
fn web_args(port: u16) -> Vec<String> {
    vec![
        "web".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--no-open".to_string(),
    ]
}

/// Is this command line one Fleet's [`web_args`] produced?
///
/// `… dsh web --port <digits> --no-open` — the port is whatever
/// [`preferred_port`] chose that day, so it is matched as a number, not a
/// literal `0`. The trailing `--no-open` is load-bearing: a `dsh web --port
/// 3080` a user started from a terminal (dsh's documented default port) must
/// never match, even once the terminal is gone and the process is reparented
/// to init.
fn is_fleet_launch_cmd(cmd: &str) -> bool {
    let Some(rest) = cmd.split("dsh web --port ").nth(1) else {
        return false;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    rest[digits..].starts_with(" --no-open")
}

/// The port a fresh server should ask for: the one the previous server on this
/// machine bound, if it is still free; else `0`.
///
/// Free is checked by binding it here, in this process, for an instant. A
/// port another program has taken since (or that a user's own `dsh web` sits
/// on) falls straight through to OS assignment instead of a failed spawn and a
/// 120 s startup timeout. The check is a race against the next binder, which
/// is why [`DshServer::start`] also falls back to `0` when the remembered port
/// cannot be started on.
fn preferred_port() -> u16 {
    let remembered = edit_registry(|registry| registry.preferred_port).flatten();
    match remembered {
        Some(port) if port != 0 && port_is_free(port) => port,
        _ => 0,
    }
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok()
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
    /// The most recent Fleet process that connected (diagnostics/migration).
    owner: HolderEntry,
    /// The port it reported. Diagnostics only — the killer works by pid.
    port: u16,
    /// Secret minted by dsh 0.1.2. Missing means a legacy, non-adoptable record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch_token: Option<String>,
    /// Spawn inputs retained so an adopted handle can restart after a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binary: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registry {
    #[serde(default)]
    servers: Vec<ServerRecord>,
    /// The port the most recently started server bound. Outlives the server
    /// record itself, which is dropped once that process dies — this is what
    /// lets the *next* server ask for the same port and keep every session's
    /// system prompt (and so its prefix cache) intact across a Fleet restart.
    /// See [`web_args`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferred_port: Option<u16>,
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
                } else {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        let _ =
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                    }
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
fn register(server_pid: u32, port: u16, launch_token: &str, binary: &Path, workspace: &Path) {
    let record = ServerRecord {
        server: HolderEntry::capture(server_pid),
        owner: HolderEntry::capture(std::process::id()),
        port,
        launch_token: Some(launch_token.to_string()),
        binary: Some(binary.to_path_buf()),
        workspace: Some(workspace.to_path_buf()),
    };
    edit_registry(|registry| {
        registry.servers.retain(|r| is_live(&r.server));
        registry.servers.push(record);
        registry.preferred_port = Some(port);
    });
}

/// Drop the record for `server_pid`. Called when Fleet stops a server itself,
/// so the registry only ever describes servers that are still running.
fn deregister(server_pid: u32) {
    edit_registry(|registry| {
        registry.servers.retain(|r| r.server.pid != server_pid);
    });
}

/// Kill every legacy recorded `dsh web` whose owning Fleet process is gone.
///
/// Current records carry the launch token and are retained for adoption even
/// after their diagnostic owner exits. Legacy token-less records retain the old
/// cleanup rule because no later Fleet can authenticate to them.
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
            if record.launch_token.is_some() {
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
/// still identifiable: Fleet always launches `dsh web --port <n> --no-open`
/// ([`is_fleet_launch_cmd`]) and always keeps the server as a direct child —
/// so one whose parent died (ppid 1) is ownerless by construction, whoever
/// started it. 13 such invisible orphans
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

    let registered: HashSet<u32> = edit_registry(|registry| {
        registry
            .servers
            .iter()
            .filter(|record| is_live(&record.server))
            .map(|record| record.server.pid)
            .collect()
    })
    .unwrap_or_default();

    let mut killed = 0;
    for (pid, process) in sys.processes() {
        let cmd = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        if !is_fleet_launch_cmd(&cmd) {
            continue;
        }
        if registered.contains(&pid.as_u32()) {
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

/// A running `dsh web` instance connected by this process.
pub struct DshServer {
    child: Option<Child>,
    server: HolderEntry,
    port: u16,
    /// The per-process launch token read off the announcement line. Every
    /// [`DshClient`] trades it for the session cookie `/api` demands, so it has
    /// to live exactly as long as the child that minted it — a restart mints a
    /// new one, alongside a new port.
    launch_token: String,
    binary: PathBuf,
    workspace: PathBuf,
    /// Direct test/server handles clean up by default. A persisted/adopted
    /// service must survive this client's process and therefore opts out.
    preserve_on_drop: bool,
}

impl DshServer {
    /// Adopt the machine service or start it exactly once across Fleet
    /// processes. The outer start lock spans health-check through registration,
    /// closing the race where two fresh clients both observed an empty registry.
    pub fn connect_or_start(binary: &Path, workspace: &Path) -> Result<Self, String> {
        let start_lock = crate::session::get_fleet_dir()
            .ok_or_else(|| "cannot determine Fleet home".to_string())?
            .join("dsh-server-start");
        crate::atomic_json::with_file_lock(&start_lock, || {
            if let Some(server) = Self::adopt_existing(binary, workspace)? {
                return Ok(server);
            }

            // Legacy token-less and registry-invisible orphans cannot be
            // authenticated or adopted; remove those before a fresh start.
            reap_orphans();
            sweep_unregistered_orphans();
            let mut server = Self::start(binary, workspace)?;
            server.detach();
            Ok(server)
        })
    }

    /// Start a server rooted at `workspace` and wait until it answers RPC.
    ///
    /// The invoking directory is dsh's default workspace root, so `workspace`
    /// decides which project new sessions belong to.
    pub fn start(binary: &Path, workspace: &Path) -> Result<Self, String> {
        if !workspace.is_dir() {
            return Err(format!("workspace does not exist: {}", workspace.display()));
        }

        // One `--version` before the spawn, so an unsupported build says so
        // instead of failing three different ways downstream. See
        // [`MIN_VERSION`]; the cost is one Node startup per server start (not
        // per session), and an unreadable version is allowed through.
        let version = crate::harness_status::probe_version(&binary.to_string_lossy());
        if !meets_min_version(version.as_deref()) {
            return Err(too_old_message(version.as_deref()));
        }

        // Ask for last time's port first (see `web_args` for why). The free
        // check inside `preferred_port` is racy by nature, and a dsh that
        // cannot bind exits without ever printing its URL — so a failure on
        // the remembered port costs one retry on an OS-assigned one, never the
        // start itself.
        let preferred = preferred_port();
        match Self::spawn_on(binary, workspace, preferred) {
            Ok(server) => Ok(server),
            Err(e) if preferred != 0 => {
                crate::log_debug(&format!(
                    "dsh web: could not come up on remembered port {preferred} ({e}); \
                     falling back to an OS-assigned port"
                ));
                Self::spawn_on(binary, workspace, 0)
            }
            Err(e) => Err(e),
        }
    }

    /// Spawn `dsh web --port <port> --no-open`, wait for its URL line and its
    /// first RPC answer, and record it. The port actually bound is read back
    /// from the launch line, so `port == 0` is fine here.
    fn spawn_on(binary: &Path, workspace: &Path, port: u16) -> Result<Self, String> {
        let mut cmd = crate::process_util::command(binary);
        cmd.args(web_args(port))
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

        let server_entry = HolderEntry::capture(child.id());
        let mut server = Self {
            child: Some(child),
            server: server_entry,
            port,
            launch_token,
            binary: binary.to_path_buf(),
            workspace: workspace.to_path_buf(),
            preserve_on_drop: false,
        };

        if let Err(e) = server.wait_healthy() {
            server.stop();
            return Err(e);
        }

        // Only a healthy server is worth recording: one that never answered has
        // already been killed above, and a record for it would just be noise the
        // next `reap_orphans` has to clear.
        register(
            server.pid(),
            server.port,
            &server.launch_token,
            &server.binary,
            &server.workspace,
        );
        Ok(server)
    }

    /// Adopt a healthy authenticated service left by an earlier Fleet process.
    /// Invalid/stale credentials remove only their exact recorded server.
    pub fn adopt_existing(binary: &Path, workspace: &Path) -> Result<Option<Self>, String> {
        let records = edit_registry(|registry| {
            registry.servers.retain(|record| is_live(&record.server));
            registry.servers.clone()
        })
        .unwrap_or_default();

        for record in records {
            let Some(token) = record.launch_token.clone() else {
                continue;
            };
            let healthy = DshClient::new(record.port, &token)
                .and_then(|client| client.call(HEALTH_ENDPOINT, serde_json::json!({})))
                .is_ok();
            if !healthy {
                if is_live(&record.server) {
                    crate::llm_provider::kill_process(record.server.pid);
                }
                deregister(record.server.pid);
                continue;
            }

            edit_registry(|registry| {
                if let Some(current) = registry
                    .servers
                    .iter_mut()
                    .find(|item| item.server == record.server)
                {
                    current.owner = HolderEntry::capture(std::process::id());
                }
            });
            return Ok(Some(Self {
                child: None,
                server: record.server,
                port: record.port,
                launch_token: token,
                binary: record.binary.unwrap_or_else(|| binary.to_path_buf()),
                workspace: record.workspace.unwrap_or_else(|| workspace.to_path_buf()),
                preserve_on_drop: true,
            }));
        }
        Ok(None)
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
        self.server.pid
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
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => is_live(&self.server),
        }
    }

    /// Restart after a crash, replacing the child.
    ///
    /// The port is normally kept (`start` asks for the remembered one — see
    /// [`web_args`] for why that matters), but the token never is: dsh mints
    /// one per process. So every cached [`DshClient`] is stale after a restart
    /// and must be rebuilt from [`client`], port reuse or not.
    ///
    /// [`client`]: Self::client
    pub fn restart(&mut self) -> Result<(), String> {
        self.stop();
        let mut fresh = Self::start(&self.binary, &self.workspace)?;
        // Swap the handles rather than moving out of `fresh` (this type has a
        // Drop impl, so it cannot be destructured). After the swap `fresh` owns
        // the already-reaped dead child, and its Drop is a no-op.
        self.child = fresh.child.take();
        self.server = fresh.server.clone();
        self.port = fresh.port;
        // The fresh child minted its own token; the old one dies with the old
        // process, so keeping it would 401 every call after a restart.
        self.launch_token = std::mem::take(&mut fresh.launch_token);
        self.preserve_on_drop = false;
        fresh.preserve_on_drop = true;
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
        let pid = self.pid();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        } else if is_live(&self.server) {
            crate::llm_provider::kill_process(pid);
        }
        // After the wait, so the record never outlives a process this call has
        // already reaped — and never describes one still shutting down.
        deregister(pid);
    }

    /// Let this authenticated service outlive the current Fleet process.
    pub fn detach(&mut self) {
        self.preserve_on_drop = true;
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
        if !self.preserve_on_drop {
            self.stop();
        }
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
    /// signature: any `… dsh web --port <n> --no-open` process whose parent
    /// died (ppid 1) is ownerless by construction — Fleet always keeps its
    /// server as a direct child — and gets killed even when no registry record
    /// names it.
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
                "\"{}\" web --port 0 --no-open >/dev/null 2>&1 & echo $!",
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
        for port in [0, 57695] {
            assert!(
                web_args(port).iter().any(|a| a == "--no-open"),
                "dsh web would open a browser tab on every Fleet start: {:?}",
                web_args(port)
            );
        }
    }

    /// The sweep matches leaked servers by command line, so whatever port
    /// `start` asks for — remembered or `0` — the argv must keep the shape the
    /// matcher expects, and the matcher must accept every port it can emit.
    #[test]
    fn keeps_the_orphan_sweep_signature() {
        assert_eq!(web_args(0).join(" "), "web --port 0 --no-open");
        for port in [0, 3080, 57695, 65535] {
            let cmd = format!("/opt/homebrew/bin/dsh {}", web_args(port).join(" "));
            assert!(
                is_fleet_launch_cmd(&cmd),
                "sweep would not recognise Fleet's own launch: {cmd}"
            );
        }
        // `node` in front, as sysinfo reports a shebang script.
        assert!(is_fleet_launch_cmd(
            "node /opt/homebrew/bin/dsh web --port 57695 --no-open"
        ));
    }

    /// Matching by port number instead of the literal `0` widened the sweep,
    /// so `--no-open` has to carry the "Fleet started this" meaning: a user's
    /// own `dsh web --port 3080` (dsh's documented default), left running
    /// after their terminal closed, is orphaned too — and must survive.
    #[test]
    fn sweep_signature_spares_a_hand_started_dsh_web() {
        for hand_started in [
            "node /opt/homebrew/bin/dsh web --port 3080",
            "node /opt/homebrew/bin/dsh web",
            "node /opt/homebrew/bin/dsh web --no-open",
        ] {
            assert!(
                !is_fleet_launch_cmd(hand_started),
                "sweep must spare a user's own server: {hand_started}"
            );
        }
        assert!(!is_fleet_launch_cmd("dsh web --port abc --no-open"));
        assert!(!is_fleet_launch_cmd("dsh web --port 3080 --open"));
        assert!(!is_fleet_launch_cmd(""));
    }

    /// With no server ever recorded there is nothing to prefer: OS assignment,
    /// exactly the old `--port 0` behaviour.
    #[test]
    fn preferred_port_is_zero_on_a_fresh_machine() {
        with_temp_fleet_home(|_| {
            assert_eq!(preferred_port(), 0);
        });
    }

    /// The point of the whole change: a Fleet restart brings dsh back on the
    /// port every session's system prompt already names, so the provider's
    /// prefix cache survives it. Exercised through `restart`, the path
    /// `ensure_alive` takes after a crash, which used to guarantee a *new*
    /// port.
    #[test]
    fn a_restarted_server_comes_back_on_the_same_port() {
        with_temp_fleet_home(|base| {
            let mut server = DshServer::start(&fake_dsh(), base).expect("start fake dsh");
            let first = server.port();
            assert_ne!(first, 0);
            assert_eq!(
                read_registry().preferred_port,
                Some(first),
                "start must remember the port it was given"
            );

            server.stop();
            assert!(
                read_registry().servers.is_empty(),
                "the server record goes with the process"
            );
            assert_eq!(
                read_registry().preferred_port,
                Some(first),
                "but the remembered port must outlive it — that is what the next start reads"
            );

            server.restart().expect("restart");
            assert_eq!(
                server.port(),
                first,
                "a restart on a free remembered port must reuse it"
            );
            server
                .client()
                .expect("client")
                .call(HEALTH_ENDPOINT, serde_json::json!({}))
                .expect("the restarted server answers on the reused port");
            server.stop();
        });
    }

    /// A remembered port someone else has since taken — another program, or a
    /// user's own `dsh web` — must not cost a failed spawn: the free check
    /// falls through to OS assignment, and the port actually bound becomes the
    /// new remembered one.
    #[test]
    fn a_busy_remembered_port_falls_back_to_os_assignment() {
        with_temp_fleet_home(|base| {
            let blocker =
                std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            let busy = blocker.local_addr().unwrap().port();
            edit_registry(|registry| registry.preferred_port = Some(busy));
            assert_eq!(preferred_port(), 0, "a busy port must not be preferred");

            let mut server = DshServer::start(&fake_dsh(), base).expect("start fake dsh");
            assert_ne!(server.port(), busy);
            assert_eq!(read_registry().preferred_port, Some(server.port()));
            server.stop();
            drop(blocker);
        });
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

    fn fake_dsh() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fake-dsh.js")
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
            register(
                std::process::id(),
                51234,
                "secret",
                Path::new("/bin/dsh"),
                Path::new("/tmp"),
            );
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

    /// Regression for the 2026-09-06 incident: replacing the Fleet app ended
    /// three unrelated dsh turns in the same 21-second window because the old
    /// GUI process killed their shared server. A current authenticated server
    /// is a machine service; an owner exit makes it adoptable, not killable.
    #[test]
    fn authenticated_server_survives_owner_exit_and_is_adopted() {
        with_temp_fleet_home(|base| {
            let mut first = DshServer::start(&fake_dsh(), base).expect("start fake dsh");
            let pid = first.pid();

            // Model the old desktop disappearing without terminating the dsh
            // service. `detach` preserves the child and its authenticated
            // registry record so another Fleet process can connect to it.
            first.detach();
            edit_registry(|registry| registry.servers[0].owner = dead());

            assert_eq!(
                reap_orphans(),
                0,
                "an authenticated service must not be killed with its GUI owner"
            );
            assert!(crate::session::is_process_alive(pid));

            let mut adopted = DshServer::adopt_existing(&fake_dsh(), base)
                .expect("read registry")
                .expect("adopt the surviving authenticated service");
            assert_eq!(adopted.pid(), pid, "restart must reuse the same dsh pid");
            adopted
                .client()
                .expect("authenticate with persisted launch token")
                .call(HEALTH_ENDPOINT, serde_json::json!({}))
                .expect("adopted service answers RPC");

            adopted.stop();
        });
    }

    /// The adoption token grants the full dsh API, so persisting it in the
    /// world-readable 0644 registry would trade availability for a local
    /// privilege leak.
    #[cfg(unix)]
    #[test]
    fn authenticated_registry_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt as _;

        with_temp_fleet_home(|base| {
            let mut server = DshServer::start(&fake_dsh(), base).expect("start fake dsh");
            let mode = std::fs::metadata(registry_path().unwrap())
                .expect("registry metadata")
                .permissions()
                .mode()
                & 0o777;
            server.stop();
            assert_eq!(mode, 0o600, "launch-token registry must not be 0644");
        });
    }

    #[test]
    fn concurrent_clients_share_one_persistent_server() {
        with_temp_fleet_home(|base| {
            let binary = fake_dsh();
            let workspace = base.to_path_buf();
            let a_binary = binary.clone();
            let a_workspace = workspace.clone();
            let b_binary = binary.clone();
            let b_workspace = workspace.clone();

            let a = std::thread::spawn(move || {
                DshServer::connect_or_start(&a_binary, &a_workspace).expect("client a")
            });
            let b = std::thread::spawn(move || {
                DshServer::connect_or_start(&b_binary, &b_workspace).expect("client b")
            });
            let mut a = a.join().unwrap();
            let b = b.join().unwrap();

            assert_eq!(a.pid(), b.pid(), "the start lock must prevent duplicates");
            assert_eq!(read_registry().servers.len(), 1);
            a.stop();
        });
    }

    #[test]
    fn deregister_removes_only_its_own_record() {
        with_temp_fleet_home(|_| {
            register(
                std::process::id(),
                1,
                "secret",
                Path::new("/bin/dsh"),
                Path::new("/tmp"),
            );
            edit_registry(|r| {
                r.servers.push(ServerRecord {
                    server: dead(),
                    owner: HolderEntry::capture(std::process::id()),
                    port: 2,
                    launch_token: None,
                    binary: None,
                    workspace: None,
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
                    launch_token: None,
                    binary: None,
                    workspace: None,
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
            register(
                std::process::id(),
                7777,
                "secret",
                Path::new("/bin/dsh"),
                Path::new("/tmp"),
            );
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
                    launch_token: None,
                    binary: None,
                    workspace: None,
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
                    launch_token: None,
                    binary: None,
                    workspace: None,
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
                    "'{}' web --port 0 --no-open >/dev/null 2>&1 & echo $!",
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
                    launch_token: None,
                    binary: None,
                    workspace: None,
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

    /// A too-old dsh must be refused *before* the spawn, with a message that
    /// names the problem.
    ///
    /// Without the gate an old binary is launched anyway and fails somewhere
    /// downstream — an unknown `--no-open`, a launch line with no token, a 401
    /// on the first call — and every one of those reaches the user as the same
    /// unhelpful "dsh web exited before reporting a port". The message has to
    /// carry all three things needed to act: the version found, the version
    /// required, and the command that fixes it.
    #[cfg(unix)]
    #[test]
    fn start_refuses_a_dsh_older_than_the_minimum() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("dsh");
        {
            let mut f = std::fs::File::create(&fake).unwrap();
            // Answers --version like a 0.1.1 build; exits immediately for
            // anything else, so a missing gate fails fast rather than sitting
            // out the 120s startup timeout.
            writeln!(
                f,
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 0.1.1; exit 0; fi\nexit 1"
            )
            .unwrap();
        }
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = match DshServer::start(&fake, dir.path()) {
            Err(e) => e,
            Ok(_) => panic!("a dsh older than {MIN_VERSION} must not start"),
        };
        assert!(err.contains("0.1.1"), "must name the version found: {err}");
        assert!(
            err.contains(MIN_VERSION),
            "must name the version required: {err}"
        );
        assert!(
            err.contains("@deepseek-ai/dsh"),
            "must name the upgrade command: {err}"
        );
    }

    /// The gate must not fire on the version that actually ships. `0.1.2-rc.1`
    /// is below `0.1.2` under strict semver, so a semver-shaped check here
    /// would refuse to launch on the boss's own machine.
    ///
    /// Stops at the launch line rather than a real server: the fake exits
    /// after `--version`, so reaching "exited before reporting a port" proves
    /// the gate passed and the spawn was attempted.
    #[cfg(unix)]
    #[test]
    fn start_accepts_the_shipping_prerelease_and_proceeds_to_spawn() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("dsh");
        {
            let mut f = std::fs::File::create(&fake).unwrap();
            writeln!(
                f,
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 0.1.2-rc.1; exit 0; fi\nexit 1"
            )
            .unwrap();
        }
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = match DshServer::start(&fake, dir.path()) {
            Err(e) => e,
            Ok(_) => panic!("the fake dsh cannot actually serve"),
        };
        assert!(
            !err.contains(&format!("needs dsh {MIN_VERSION}")),
            "0.1.2-rc.1 must pass the version gate, got: {err}"
        );
    }
}
