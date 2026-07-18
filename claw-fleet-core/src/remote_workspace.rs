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
//! `rca <claude-path> <args> --code <pairing-code>`.
//!
//! Registry file: `~/.fleet/remote-workspaces.json`.
//!
//! Hard constraints inherited from rca (verified against its source):
//! - **Identity path mapping** (`internal/routing/routing.go` has no
//!   translation layer): the local mirror directory must exist at the SAME
//!   absolute path as the remote repo. An empty dir suffices —
//!   [`wrap_launch`] `create_dir_all`s it before every spawn, and
//!   [`upsert`] does so at registration so an uncreatable path (e.g.
//!   `/home/...` on macOS, an automount) fails loudly at registration time,
//!   not at first spawn.
//! - The **pairing code** is the self-contained `rca1.…` string printed by
//!   `rca serve` on the remote host (`internal/paircode`, `Prefix = "rca1."`).
//!   It changes when the remote serve restarts with a fresh identity; the
//!   registry entry is then updated via [`upsert`].
//! - rca-owned flags (`--code`) are extracted from anywhere after the wrapped
//!   command token and are chosen upstream to not collide with claude/codex
//!   flags, so the original argv is passed through verbatim.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session_launch::normalize_workspace_path;

/// One registered remote workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspace {
    /// Absolute workspace path — identical on both machines (identity mapping).
    pub path: String,
    /// The `rca1.…` pairing code printed by `rca serve` on the remote host.
    pub pairing_code: String,
    /// Display label for the UI (e.g. the remote host's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Per-workspace override of the rca binary path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rca_path: Option<String>,
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
    /// `[<agent-cli-path>, <original args...>, --code, <pairing-code>]`.
    pub args: Vec<String>,
    /// Extra env for the rca process (inherited by the agent CLI under it).
    pub envs: Vec<(String, String)>,
}

/// Substrings (':'-joined into `RCC_LOCAL_BINS`) that rca's interceptor
/// matches against a spawned binary's path to force it LOCAL even under a
/// remote-routed cwd. Fleet's own binary must stay local: claude spawns it
/// directly as the `fleet` MCP server (`~/.claude.json` mcpServers), and
/// routed remote it would not exist — every decision card / permission prompt
/// would break. `sh -c` hook commands are NOT covered (the spawned binary is
/// `sh`, matched by cwd): they route remote and no-op behind their own `-x`
/// guards — a documented v1 degradation.
const LOCAL_BIN_MARKS: &[&str] = &["Claw Fleet.app/Contents/MacOS/fleet", ".fleet/bin/fleet"];

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

/// Register (or update) a remote workspace. Normalizes the path, validates the
/// pairing code, and creates the local mirror directory — so an uncreatable
/// path fails here, with the OS error, instead of at first spawn.
pub fn upsert(entry: RemoteWorkspace) -> Result<RemoteWorkspacesConfig, String> {
    let path = normalize_workspace_path(&entry.path)?;
    validate_pairing_code(&entry.pairing_code)?;
    fs::create_dir_all(&path).map_err(|e| {
        format!(
            "cannot create local mirror directory {path}: {e} — the workspace must live at a path \
             creatable on this machine (identical to its remote path)"
        )
    })?;
    let entry = RemoteWorkspace {
        path: path.clone(),
        pairing_code: entry.pairing_code.trim().to_string(),
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
    fs::create_dir_all(workspace_path)
        .map_err(|e| format!("create local mirror directory {workspace_path}: {e}"))?;
    let mut wrapped = Vec::with_capacity(args.len() + 3);
    wrapped.push(program.to_string());
    wrapped.extend_from_slice(args);
    wrapped.push("--code".to_string());
    wrapped.push(entry.pairing_code.clone());
    Ok(Some(WrappedLaunch {
        program: rca,
        args: wrapped,
        envs: vec![("RCC_LOCAL_BINS".to_string(), LOCAL_BIN_MARKS.join(":"))],
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

    fn entry(path: &str, code: &str) -> RemoteWorkspace {
        RemoteWorkspace {
            path: path.to_string(),
            pairing_code: code.to_string(),
            label: None,
            rca_path: None,
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
            cfg.workspaces.iter().find(|w| w.path == a).unwrap().pairing_code,
            "rca1.CCC"
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
}
