//! Remote-workspace registry: workspaces whose repo lives on another machine,
//! reached through rca (remote-adapter, `~/workspace/remote-cc-adapter`).
//!
//! Topology: the agent CLI (claude / codex) still runs on THIS machine — its
//! credentials, transcript JSONL and therefore Fleet's whole monitoring
//! surface (tail, decision cards, usage) stay local — while rca's syscall
//! interception routes file I/O and subprocesses under the workspace path to a
//! remote `rca serve` executor. Fleet's part is deliberately small: keep a
//! registry of which workspace paths are remote, and wrap the launch at the
//! two spawn chokepoints ([`crate::session_launch`] for claude,
//! [`crate::codex_launch`] for codex) so `claude <args>` becomes
//! `rca <claude-path> <args> <transport-flags>`.
//!
//! Registry file: `~/.fleet/remote-workspaces.json`.
//!
//! ## Two transports
//!
//! A registry entry carries exactly one of two transports ([`Transport`],
//! resolved by [`RemoteWorkspace::transport`]):
//!
//! - **Pairing code** (libp2p): the self-contained `rca1.…` string printed by
//!   `rca serve` on the remote host (`internal/paircode`, `Prefix = "rca1."`).
//!   It changes when the remote serve restarts with a fresh identity; the
//!   registry entry is then updated via [`upsert`]. [`wrap_launch`] appends
//!   `--code <pairing-code>`.
//! - **stdio-over-ssh**: the entry stores an `ssh_target` (+ optional remote
//!   rca path). No pairing code, no relay, no long-lived remote serve —
//!   [`wrap_launch`] appends `--via 'ssh <target> <remote-rca> serve --stdio'`,
//!   and rca spawns that ssh command per run, speaking a single yamux-muxed
//!   byte stream over ssh's stdin/stdout. The serve dies when the ssh child
//!   exits (session end). See wiki `rca/stdio-transport-design`.
//!
//! Hard constraints inherited from rca (verified against its source):
//! - **Identity path mapping** (`internal/routing/routing.go` has no
//!   translation layer): the local mirror directory must exist at the SAME
//!   absolute path as the remote repo. An empty dir suffices —
//!   [`wrap_launch`] `create_dir_all`s it before every spawn, and
//!   [`upsert`] does so at registration so an uncreatable path (e.g.
//!   `/home/...` on macOS, an automount) fails loudly at registration time,
//!   not at first spawn.
//! - rca-owned flags (`--code` / `--via`) are extracted from anywhere after
//!   the wrapped command token and are chosen upstream to not collide with
//!   claude/codex flags, so the original argv is passed through verbatim.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session_launch::normalize_workspace_path;

/// One registered remote workspace. Exactly one transport is set: either
/// `pairing_code` (libp2p) or `ssh_target` (stdio-over-ssh); see
/// [`RemoteWorkspace::transport`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspace {
    /// Absolute workspace path — identical on both machines (identity mapping).
    pub path: String,
    /// Pairing-code (libp2p) transport: the `rca1.…` code printed by
    /// `rca serve` on the remote host. `None` for stdio-over-ssh entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
    /// stdio-over-ssh transport: the ssh target (host alias or `user@host`)
    /// Fleet runs `ssh <target> <remote-rca> serve --stdio` against. `None`
    /// for pairing-code entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// stdio-over-ssh transport: the rca binary path ON THE REMOTE host.
    /// `None` = `rca` on the remote `$PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_rca_path: Option<String>,
    /// Display label for the UI (e.g. the remote host's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Per-workspace override of the LOCAL rca binary path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rca_path: Option<String>,
}

/// The resolved transport for a registry entry, after validating that exactly
/// one mode's fields are set and well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Transport {
    /// libp2p pairing code (`rca1.…`).
    Pairing(String),
    /// stdio-over-ssh: ssh target + rca binary path on the remote host.
    Stdio { ssh_target: String, remote_rca: String },
}

/// The registry file: `~/.fleet/remote-workspaces.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspacesConfig {
    /// Global rca binary path, used when an entry has no own `rca_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rca_path: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<RemoteWorkspace>,
}

/// A spawn launch rewritten to run through rca.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedLaunch {
    /// The rca binary to exec instead of the agent CLI.
    pub program: String,
    /// `[<agent-cli-path>, <original args...>, <transport-flags>]` — the
    /// transport flags are `--code <pairing-code>` or `--via '<ssh cmd>'`.
    pub args: Vec<String>,
    /// Extra env for the rca process (inherited by the agent CLI under it).
    pub envs: Vec<(String, String)>,
}

/// Substrings (':'-joined into `RCC_LOCAL_BINS`) that rca's interceptor
/// matches against a spawned binary's path to force it LOCAL even under a
/// remote-routed cwd. Fleet's own binary must stay local: claude spawns it
/// directly as the `fleet` MCP server (`~/.claude.json` mcpServers), and
/// routed remote it would not exist — every decision card / permission prompt
/// would break.
const LOCAL_BIN_MARKS: &[&str] = &["Claw Fleet.app/Contents/MacOS/fleet", ".fleet/bin/fleet"];

/// Substrings (':'-joined into `RCC_LOCAL_ARGV_MARKS`) rca matches against
/// every argv token to force a spawn LOCAL. Fleet's hooks arrive as
/// `sh -c '… "/Applications/Claw Fleet.app/…/fleet" …'` — the same `/bin/sh`
/// a routed user command uses, so `RCC_LOCAL_BINS` (binary path) cannot tell
/// them apart; the hook *script text* can. Covers the guard / idle /
/// prd-context hooks (they name the app bundle path) and the hooks.jsonl
/// observability append. Marks are deliberately long and harness-specific so
/// real user commands never contain them.
const LOCAL_ARGV_MARKS: &[&str] =
    &["Claw Fleet.app/Contents/MacOS/fleet", ".fleet/bin/fleet", ".fleet/hooks.jsonl"];

/// The pairing-code prefix (`internal/paircode/paircode.go` `Prefix`).
const PAIRING_CODE_PREFIX: &str = "rca1.";

fn config_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("remote-workspaces.json"))
}

/// Read the registry; a missing or unreadable file is an empty registry.
pub fn load() -> RemoteWorkspacesConfig {
    let Some(path) = config_path() else {
        return RemoteWorkspacesConfig::default();
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(cfg: &RemoteWorkspacesConfig) -> Result<(), String> {
    let path = config_path().ok_or("cannot resolve ~/.fleet")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("serialize remote-workspaces: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))
}

fn validate_pairing_code(code: &str) -> Result<(), String> {
    let code = code.trim();
    if !code.starts_with(PAIRING_CODE_PREFIX) || code.len() <= PAIRING_CODE_PREFIX.len() {
        return Err(format!(
            "pairing code must be the '{PAIRING_CODE_PREFIX}…' string printed by `rca serve` on the remote host"
        ));
    }
    Ok(())
}

/// The ssh target and remote rca path are embedded verbatim into the
/// `sh -c '<via>'` string rca runs for `--via` (`cmd/rca/run.go`), so a space,
/// quote, or shell metacharacter would break argv splitting or inject.
/// Registration values come from saved SSH connections (trusted), but reject
/// them defensively — an ssh host alias / `user@host` / rca path never needs
/// these characters.
fn validate_shell_token(tok: &str, what: &str) -> Result<(), String> {
    if tok.is_empty() {
        return Err(format!("{what} must not be empty"));
    }
    const BAD: &[char] =
        &[' ', '\t', '\'', '"', ';', '&', '|', '$', '`', '\n', '\r', '<', '>', '(', ')', '\\', '*'];
    if let Some(c) = tok.chars().find(|c| BAD.contains(c)) {
        return Err(format!("{what} contains an unsupported character {c:?}"));
    }
    Ok(())
}

impl RemoteWorkspace {
    /// Resolve which transport this entry describes, validating that exactly
    /// one mode's fields are set and well-formed. The remote rca path defaults
    /// to `rca` (on the remote `$PATH`) for stdio entries.
    fn transport(&self) -> Result<Transport, String> {
        let code = self.pairing_code.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let target = self.ssh_target.as_deref().map(str::trim).filter(|s| !s.is_empty());
        match (code, target) {
            (Some(code), None) => {
                validate_pairing_code(code)?;
                Ok(Transport::Pairing(code.to_string()))
            }
            (None, Some(target)) => {
                validate_shell_token(target, "ssh target")?;
                let remote_rca = self
                    .remote_rca_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("rca")
                    .to_string();
                validate_shell_token(&remote_rca, "remote rca path")?;
                Ok(Transport::Stdio { ssh_target: target.to_string(), remote_rca })
            }
            (Some(_), Some(_)) => Err(
                "a remote workspace has both a pairing code and an ssh target — set exactly one \
                 transport"
                    .to_string(),
            ),
            (None, None) => Err(
                "a remote workspace needs either a pairing code (libp2p) or an ssh target \
                 (stdio-over-ssh)"
                    .to_string(),
            ),
        }
    }
}

/// Register (or update) a remote workspace. Normalizes the path, validates the
/// transport (exactly one of pairing code / ssh target, well-formed), and
/// creates the local mirror directory — so an uncreatable path fails here,
/// with the OS error, instead of at first spawn.
pub fn upsert(entry: RemoteWorkspace) -> Result<RemoteWorkspacesConfig, String> {
    let path = normalize_workspace_path(&entry.path)?;
    entry.transport()?;
    fs::create_dir_all(&path).map_err(|e| {
        format!(
            "cannot create local mirror directory {path}: {e} — the workspace must live at a path \
             creatable on this machine (identical to its remote path)"
        )
    })?;
    let entry = RemoteWorkspace {
        path: path.clone(),
        pairing_code: entry.pairing_code.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        ssh_target: entry.ssh_target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        remote_rca_path: entry
            .remote_rca_path
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
        label: entry.label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()),
        rca_path: entry.rca_path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
    };
    let mut cfg = load();
    match cfg.workspaces.iter_mut().find(|w| w.path == path) {
        Some(existing) => *existing = entry,
        None => cfg.workspaces.push(entry),
    }
    save(&cfg)?;
    Ok(cfg)
}

/// Remove a remote workspace by path. Unknown paths are a no-op.
pub fn remove(path: &str) -> Result<RemoteWorkspacesConfig, String> {
    let path = normalize_workspace_path(path)?;
    let mut cfg = load();
    cfg.workspaces.retain(|w| w.path != path);
    save(&cfg)?;
    Ok(cfg)
}

/// The registered remote workspace covering `path`: an exact match, or the
/// entry `path` is nested under (same equal-or-under-prefix semantics as rca's
/// own routing table), so a spawn in a subdirectory of a registered workspace
/// still routes through rca.
pub fn find_for_path(path: &str) -> Option<RemoteWorkspace> {
    let clean = Path::new(path.trim_end_matches('/'));
    load()
        .workspaces
        .into_iter()
        .find(|w| clean == Path::new(&w.path) || clean.starts_with(Path::new(&w.path)))
}

/// Resolve the rca binary for `entry`: per-workspace override → global
/// override → `~/.fleet/bin/rca` → `$PATH`. A configured-but-missing override
/// is an error, not a silent fallthrough.
fn resolve_rca_binary(entry: &RemoteWorkspace, cfg: &RemoteWorkspacesConfig) -> Result<String, String> {
    for (source, configured) in
        [("workspace rcaPath", &entry.rca_path), ("global rcaPath", &cfg.rca_path)]
    {
        if let Some(p) = configured {
            if Path::new(p).is_file() {
                return Ok(p.clone());
            }
            return Err(format!("{source} points to {p}, which does not exist"));
        }
    }
    if let Some(bin) = crate::fleet_cli::fleet_bin_dir().map(|d| d.join("rca")) {
        if bin.is_file() {
            return Ok(bin.to_string_lossy().into_owned());
        }
    }
    crate::process_util::which("rca").ok_or_else(|| {
        "rca binary not found — install it on PATH or into ~/.fleet/bin, or set rcaPath in \
         ~/.fleet/remote-workspaces.json"
            .to_string()
    })
}

/// Create the local mirror directory when `path` is (under) a registered
/// remote workspace — for callers whose `is_dir` spawn gate runs before the
/// launch is wrapped. No-op for local workspaces.
pub fn ensure_local_mirror(path: &str) -> Result<(), String> {
    if find_for_path(path).is_some() {
        fs::create_dir_all(path)
            .map_err(|e| format!("create local mirror directory {path}: {e}"))?;
    }
    Ok(())
}

/// If `workspace_path` is (under) a registered remote workspace, rewrite the
/// agent launch `program args…` into an rca run-mode launch and ensure the
/// local mirror directory exists. `Ok(None)` for plain local workspaces —
/// the caller spawns unchanged.
///
/// The caller keeps setting the child's cwd to `workspace_path`: rca defaults
/// its remote routing prefix to its working directory, which is exactly the
/// registered workspace (or a subdirectory of it, still under the prefix the
/// remote serves).
pub fn wrap_launch(
    workspace_path: &str,
    program: &str,
    args: &[String],
) -> Result<Option<WrappedLaunch>, String> {
    let Some(entry) = find_for_path(workspace_path) else {
        return Ok(None);
    };
    let cfg = load();
    let rca = resolve_rca_binary(&entry, &cfg)?;
    let transport = entry.transport()?;
    fs::create_dir_all(workspace_path)
        .map_err(|e| format!("create local mirror directory {workspace_path}: {e}"))?;
    let mut wrapped = Vec::with_capacity(args.len() + 3);
    wrapped.push(program.to_string());
    wrapped.extend_from_slice(args);
    match transport {
        Transport::Pairing(code) => {
            wrapped.push("--code".to_string());
            wrapped.push(code);
        }
        Transport::Stdio { ssh_target, remote_rca } => {
            // `--via '<shell cmd>'` — rca runs it under `sh -c` and speaks a
            // single yamux stream over its stdin/stdout. ServerAliveInterval
            // keeps the ssh tunnel from silently half-dying on an idle
            // session; both fields are shell-token-validated in `transport()`.
            wrapped.push("--via".to_string());
            wrapped.push(format!(
                "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 {ssh_target} {remote_rca} \
                 serve --stdio"
            ));
        }
    }
    Ok(Some(WrappedLaunch {
        program: rca,
        args: wrapped,
        envs: vec![
            ("RCC_LOCAL_BINS".to_string(), LOCAL_BIN_MARKS.join(":")),
            ("RCC_LOCAL_ARGV_MARKS".to_string(), LOCAL_ARGV_MARKS.join(":")),
        ],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FLEET_HOME`-redirected temp home, serialized on the process-wide lock
    /// (same pattern as `launch_spec::tests`).
    struct TmpHome {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-remotews-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialized on the process-wide FLEET_HOME lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }

        fn path(&self, rel: &str) -> String {
            self.dir.join(rel).to_string_lossy().into_owned()
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A pairing-code (libp2p) entry.
    fn entry(path: &str, code: &str) -> RemoteWorkspace {
        RemoteWorkspace {
            path: path.to_string(),
            pairing_code: Some(code.to_string()),
            ..Default::default()
        }
    }

    /// A stdio-over-ssh entry.
    fn stdio_entry(path: &str, ssh_target: &str) -> RemoteWorkspace {
        RemoteWorkspace {
            path: path.to_string(),
            ssh_target: Some(ssh_target.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn registry_roundtrip_upsert_update_remove() {
        let home = TmpHome::new("roundtrip");
        let a = home.path("repo-a");
        let b = home.path("repo-b");
        upsert(entry(&a, "rca1.AAA")).unwrap();
        upsert(entry(&b, "rca1.BBB")).unwrap();
        assert_eq!(load().workspaces.len(), 2);

        // Same path again = update in place, not a duplicate.
        upsert(entry(&a, "rca1.CCC")).unwrap();
        let cfg = load();
        assert_eq!(cfg.workspaces.len(), 2);
        assert_eq!(
            cfg.workspaces.iter().find(|w| w.path == a).unwrap().pairing_code.as_deref(),
            Some("rca1.CCC")
        );

        remove(&a).unwrap();
        let cfg = load();
        assert_eq!(cfg.workspaces.len(), 1);
        assert_eq!(cfg.workspaces[0].path, b);
    }

    #[test]
    fn upsert_rejects_a_non_pairing_code() {
        let home = TmpHome::new("badcode");
        for bad in ["", "rca1.", "ssh://host", "  "] {
            assert!(
                upsert(entry(&home.path("repo"), bad)).is_err(),
                "code {bad:?} must be refused"
            );
        }
    }

    #[test]
    fn upsert_creates_the_local_mirror_directory() {
        let home = TmpHome::new("mkdir");
        let path = home.path("deep/nested/repo");
        upsert(entry(&path, "rca1.AAA")).unwrap();
        assert!(Path::new(&path).is_dir());
    }

    /// The whole point of prefix matching: a session spawned in a subdirectory
    /// of a registered workspace still routes through rca — but a sibling that
    /// merely shares the string prefix must not.
    #[test]
    fn find_for_path_matches_exact_and_nested_but_not_string_prefix() {
        let home = TmpHome::new("find");
        let ws = home.path("proj");
        upsert(entry(&ws, "rca1.AAA")).unwrap();

        assert!(find_for_path(&ws).is_some());
        assert!(find_for_path(&format!("{ws}/")).is_some());
        assert!(find_for_path(&format!("{ws}/sub/dir")).is_some());
        assert!(find_for_path(&format!("{ws}ect")).is_none(), "sibling 'project' must not match");
        assert!(find_for_path(&home.path("other")).is_none());
    }

    #[test]
    fn wrap_launch_is_none_for_an_unregistered_workspace() {
        let home = TmpHome::new("local");
        let got = wrap_launch(&home.path("plain"), "claude", &["-p".into()]).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn wrap_launch_rewrites_the_program_and_appends_the_code() {
        let home = TmpHome::new("wrap");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = entry(&ws, "rca1.AAA");
        e.rca_path = Some(fake_rca.clone());
        upsert(e).unwrap();

        let args: Vec<String> = ["-p", "hi", "--session-id", "s1"].map(String::from).into();
        let got = wrap_launch(&ws, "/usr/local/bin/claude", &args).unwrap().unwrap();

        assert_eq!(got.program, fake_rca);
        assert_eq!(
            got.args,
            ["/usr/local/bin/claude", "-p", "hi", "--session-id", "s1", "--code", "rca1.AAA"]
                .map(String::from)
                .to_vec()
        );
        assert!(
            got.envs.iter().any(|(k, v)| k == "RCC_LOCAL_BINS" && v.contains("MacOS/fleet")),
            "must pin the fleet binary local for the MCP decision-card bridge"
        );
        assert!(
            got.envs.iter().any(|(k, v)| {
                k == "RCC_LOCAL_ARGV_MARKS"
                    && v.contains("MacOS/fleet")
                    && v.contains(".fleet/hooks.jsonl")
            }),
            "must pin the sh -c hook commands local by argv marks"
        );
    }

    /// A registered workspace whose rca override vanished must fail the spawn
    /// loudly — falling through to PATH would run a different binary than the
    /// operator pinned.
    #[test]
    fn wrap_launch_errors_when_the_configured_rca_is_missing() {
        let home = TmpHome::new("norca");
        let ws = home.path("proj");
        let mut e = entry(&ws, "rca1.AAA");
        e.rca_path = Some(home.path("gone"));
        upsert(e).unwrap();
        let err = wrap_launch(&ws, "claude", &[]).unwrap_err();
        assert!(err.contains("does not exist"), "got: {err}");
    }

    /// A stdio-over-ssh entry appends `--via 'ssh … serve --stdio'` instead of
    /// `--code`, and round-trips through the registry unchanged.
    #[test]
    fn wrap_launch_stdio_appends_via_not_code() {
        let home = TmpHome::new("stdio-wrap");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = stdio_entry(&ws, "gpu-box");
        e.remote_rca_path = Some("/opt/rca".into());
        e.rca_path = Some(fake_rca.clone());
        upsert(e).unwrap();

        // Round-trips: ssh target + remote rca path survive, no pairing code.
        let saved = load().workspaces.into_iter().find(|w| w.path == ws).unwrap();
        assert_eq!(saved.ssh_target.as_deref(), Some("gpu-box"));
        assert_eq!(saved.remote_rca_path.as_deref(), Some("/opt/rca"));
        assert_eq!(saved.pairing_code, None);

        let got = wrap_launch(&ws, "/usr/local/bin/claude", &["-p".into()]).unwrap().unwrap();
        assert_eq!(got.program, fake_rca);
        assert_eq!(got.args[0], "/usr/local/bin/claude");
        assert_eq!(got.args[1], "-p");
        assert_eq!(got.args[2], "--via");
        assert_eq!(
            got.args[3],
            "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 gpu-box /opt/rca serve --stdio"
        );
        assert!(!got.args.contains(&"--code".to_string()), "stdio must not append --code");
        // The fleet-local pins still apply regardless of transport.
        assert!(got.envs.iter().any(|(k, _)| k == "RCC_LOCAL_BINS"));
    }

    /// A stdio entry with no explicit remote rca path defaults to `rca`.
    #[test]
    fn wrap_launch_stdio_defaults_remote_rca_to_rca() {
        let home = TmpHome::new("stdio-default");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = stdio_entry(&ws, "user@host");
        e.rca_path = Some(fake_rca);
        upsert(e).unwrap();
        let got = wrap_launch(&ws, "claude", &[]).unwrap().unwrap();
        assert_eq!(
            got.args.last().unwrap(),
            "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 user@host rca serve --stdio"
        );
    }

    /// Exactly one transport: neither set, or both set, is refused at upsert.
    #[test]
    fn upsert_requires_exactly_one_transport() {
        let home = TmpHome::new("onetransport");
        // Neither.
        let bare = RemoteWorkspace { path: home.path("a"), ..Default::default() };
        assert!(upsert(bare).is_err(), "no transport must be refused");
        // Both.
        let mut both = stdio_entry(&home.path("b"), "host");
        both.pairing_code = Some("rca1.AAA".into());
        assert!(upsert(both).is_err(), "both transports must be refused");
    }

    /// The ssh target / remote rca path are embedded into a `sh -c` string, so
    /// shell metacharacters must be rejected at registration.
    #[test]
    fn upsert_rejects_shell_metachars() {
        let home = TmpHome::new("shellmeta");
        for bad in ["host; rm -rf /", "host$(whoami)", "a host", "host`id`", "h|p"] {
            assert!(
                upsert(stdio_entry(&home.path("proj"), bad)).is_err(),
                "ssh target {bad:?} must be refused"
            );
        }
        let mut e = stdio_entry(&home.path("proj"), "safe-host");
        e.remote_rca_path = Some("/opt/rca; evil".into());
        assert!(upsert(e).is_err(), "remote rca path with metachars must be refused");
    }
}
