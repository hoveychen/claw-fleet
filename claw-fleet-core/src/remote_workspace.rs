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
    /// stdio-over-ssh transport: the ssh argument fragment Fleet drops into
    /// `ssh <ssh_target> <remote-rca> serve --stdio`. Either a bare ssh-config
    /// `alias`, or a full fragment carrying connection options —
    /// `-p 2222 -i /path/key -J jump user@host`. Spaces are allowed (they split
    /// into argv words for ssh); shell metacharacters are rejected. `None` for
    /// pairing-code entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// stdio-over-ssh transport, **preferred form**: the id of an entry in the
    /// ssh host book ([`crate::remote_host::SshHost`]).
    ///
    /// Why an id rather than the resolved `ssh_target`: a host's address is
    /// editable. With the target baked in, changing a host's port or user
    /// silently orphans every workspace registered against it — the entry keeps
    /// dialling the old address and only fails at the next session spawn. An id
    /// follows the edit. `ssh_target` stays supported for entries written
    /// before the host book existed, and as the escape hatch for a target that
    /// is not a book entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// stdio-over-ssh transport: the rca binary path ON THE REMOTE host.
    /// `None` = take it from the host book entry, else `rca` on the remote
    /// `$PATH`.
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

/// Shell metacharacters that, embedded into the `sh -c '<via>'` string rca
/// runs for `--via` (`cmd/rca/run.go`), could break argv splitting or inject a
/// command. Registration values come from saved SSH connections (trusted), but
/// reject these defensively. NOTE: a space is NOT here — see the two validators.
const SHELL_METACHARS: &[char] =
    &['\'', '"', ';', '&', '|', '$', '`', '\n', '\r', '<', '>', '(', ')', '\\', '*', '\t'];

/// A single unsplittable token (the remote rca path): reject spaces AND
/// metacharacters — a filesystem path passed as one argv word never needs them.
fn validate_shell_token(tok: &str, what: &str) -> Result<(), String> {
    if tok.is_empty() {
        return Err(format!("{what} must not be empty"));
    }
    if tok.contains(' ') {
        return Err(format!("{what} must not contain spaces"));
    }
    if let Some(c) = tok.chars().find(|c| SHELL_METACHARS.contains(c)) {
        return Err(format!("{what} contains an unsupported character {c:?}"));
    }
    Ok(())
}

/// The ssh target holds a full ssh argument fragment (e.g. `-p 2222 -i /key
/// -J jump user@host`, or just an ssh-config `alias`), so spaces ARE allowed —
/// `sh -c` splits them into separate argv words for ssh, which is exactly the
/// point. Shell metacharacters are still rejected (a space is a benign argv
/// separator; `;`/`$`/backtick/quotes would inject or misparse). This is what
/// lets a connection with a custom port / identity file / jump host be
/// expressed, instead of only a single bare host token.
fn validate_ssh_target(tok: &str) -> Result<(), String> {
    let tok = tok.trim();
    if tok.is_empty() {
        return Err("ssh target must not be empty".to_string());
    }
    if let Some(c) = tok.chars().find(|c| SHELL_METACHARS.contains(c)) {
        return Err(format!("ssh target contains an unsupported character {c:?}"));
    }
    Ok(())
}

impl RemoteWorkspace {
    /// Resolve which transport this entry describes, validating that exactly
    /// one mode's fields are set and well-formed. The remote rca path defaults
    /// to `rca` (on the remote `$PATH`) for stdio entries.
    /// Resolve this entry's ssh target, plus the rca path its host book entry
    /// declares (if any).
    ///
    /// `host_id` wins over a stored `ssh_target`: an entry written by the
    /// current installer carries both (the id for durability, the target so an
    /// older Fleet can still read the file), and the id is the authoritative
    /// one. A `host_id` naming a host that no longer exists is an error rather
    /// than a silent fall-through to the stale target — the user deleted that
    /// host, and quietly dialling its last known address is exactly the
    /// surprise this field exists to prevent.
    /// The ssh target this entry will actually dial, resolving `host_id`
    /// through the host book. `None` for a pairing-code entry. Public so the
    /// installer's update path can re-provision a host without caring which
    /// form the entry was written in.
    pub fn resolved_ssh_target(&self) -> Result<Option<String>, String> {
        Ok(self.resolve_ssh_target()?.0)
    }

    fn resolve_ssh_target(&self) -> Result<(Option<String>, Option<String>), String> {
        if let Some(id) = self.host_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let host = crate::remote_host::find_host(id).ok_or_else(|| {
                format!(
                    "{}: the host this workspace runs on ({id}) has been removed — pick it again \
                     from a host that still exists, or re-add that host in Settings",
                    codes::HOST_GONE
                )
            })?;
            let target = crate::remote_host::ssh_target_for(&host)?;
            return Ok((Some(target), host.rca_path));
        }
        Ok((
            self.ssh_target.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
            None,
        ))
    }

    fn transport(&self) -> Result<Transport, String> {
        let code = self.pairing_code.as_deref().map(str::trim).filter(|s| !s.is_empty());
        // A `host_id` resolves to an ssh target through the host book, so both
        // forms collapse to the same thing before the exactly-one check — an
        // entry carrying a host id and a pairing code is still a conflict.
        let (target, book_rca) = self.resolve_ssh_target()?;
        let target = target.as_deref();
        match (code, target) {
            (Some(code), None) => {
                validate_pairing_code(code)?;
                Ok(Transport::Pairing(code.to_string()))
            }
            (None, Some(target)) => {
                validate_ssh_target(target)?;
                let remote_rca = self
                    .remote_rca_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or(book_rca)
                    .unwrap_or_else(|| "rca".to_string());
                validate_shell_token(&remote_rca, "remote rca path")?;
                Ok(Transport::Stdio { ssh_target: target.to_string(), remote_rca })
            }
            (Some(_), Some(_)) => Err(
                "a remote workspace has both a pairing code and an ssh target — set exactly one \
                 transport"
                    .to_string(),
            ),
            (None, None) => Err(format!(
                "{}: this remote workspace has no host to run on — register it again from the \
                 new-session composer",
                codes::NO_TRANSPORT
            )),
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
        host_id: entry.host_id.map(|h| h.trim().to_string()).filter(|h| !h.is_empty()),
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
            return Err(format!(
                "{}: the configured rca path ({source}) points at {p}, which does not exist — \
                 clear the override, or re-run the host's rca install from Settings",
                codes::BAD_RCA_OVERRIDE
            ));
        }
    }
    if let Some(bin) = crate::fleet_cli::fleet_bin_dir().map(|d| d.join("rca")) {
        if bin.is_file() {
            return Ok(bin.to_string_lossy().into_owned());
        }
    }
    crate::process_util::which("rca").ok_or_else(|| {
        // The installer puts rca on this machine too, so by the time a user
        // meets this the actionable step is "run it again", not "go and edit
        // a JSON file by hand".
        format!(
            "{}: rca is not installed on this machine, so a remote workspace cannot start — \
             open Settings and install rca for this host again",
            codes::NO_LOCAL_RCA
        )
    })
}

/// Public form of [`resolve_rca_binary`] with no registry entry in hand: the
/// same override → `~/.fleet/bin/rca` → `$PATH` chain the installer wants to
/// consult before deciding whether a local download is needed.
pub fn find_local_rca() -> Option<String> {
    let cfg = load();
    resolve_rca_binary(&RemoteWorkspace::default(), &cfg).ok()
}

/// Map a `uname -sm` string to the rca release archive slug (`<os>_<arch>`),
/// matching the tarballs published at
/// `github.com/hoveychen/remote-adapter/releases` (Go arch naming: amd64 /
/// arm64, NOT x86_64 / aarch64). Shared by the remote installer (which reads a
/// real `uname -sm` over ssh) and [`local_release_slug`].
pub fn rca_release_slug(uname: &str) -> Option<&'static str> {
    let u = uname.to_lowercase();
    let os = if u.contains("linux") {
        "linux"
    } else if u.contains("darwin") {
        "darwin"
    } else {
        return None;
    };
    let arch = if u.contains("x86_64") || u.contains("amd64") {
        "amd64"
    } else if u.contains("aarch64") || u.contains("arm64") {
        "arm64"
    } else {
        return None;
    };
    Some(match (os, arch) {
        ("linux", "amd64") => "linux_amd64",
        ("linux", "arm64") => "linux_arm64",
        ("darwin", "amd64") => "darwin_amd64",
        ("darwin", "arm64") => "darwin_arm64",
        _ => return None,
    })
}

/// The release slug for THIS machine, from `std::env::consts` rather than a
/// `uname` subprocess. `consts::OS` is already "linux"/"macos" and `consts::ARCH`
/// "x86_64"/"aarch64" — fed through [`rca_release_slug`] so both sides of the
/// installer share one mapping table. `None` on any platform rca does not
/// publish (Windows, FreeBSD, 32-bit).
pub fn local_release_slug() -> Option<&'static str> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    rca_release_slug(&format!("{os} {}", std::env::consts::ARCH))
}

/// The rca release tarball URL for a slug. `releases/latest` follows a 302 to
/// whatever the newest published tag is, so Fleet never pins a version.
pub fn rca_release_url(slug: &str) -> String {
    format!("https://github.com/hoveychen/remote-adapter/releases/latest/download/rca_{slug}.tar.gz")
}

/// Download rca for this machine into `~/.fleet/bin/rca` and verify it supports
/// the stdio transport. Returns the installed absolute path.
///
/// The LOCAL half of the installer. `wrap_launch` runs rca on the machine that
/// spawns the agent, so a workspace whose remote rca is installed but whose
/// local rca is missing fails at first spawn with "rca binary not found" — the
/// wizard's success message would be a lie. Mirrors the remote install exactly
/// (same `curl | tar xz`, same `serve --stdio` probe) so the two halves cannot
/// drift; `curl` and `tar` are in `/usr/bin`, which is on the minimal PATH a
/// GUI-launched app inherits.
pub fn install_local_rca() -> Result<String, String> {
    let slug = local_release_slug().ok_or_else(|| {
        format!(
            "rca publishes no release for this platform ({} {}) — remote workspaces are \
             unavailable here",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let bin_dir = crate::fleet_cli::fleet_bin_dir().ok_or("cannot resolve ~/.fleet/bin")?;
    fs::create_dir_all(&bin_dir).map_err(|e| format!("create {}: {e}", bin_dir.display()))?;
    let dir = bin_dir.to_string_lossy();
    let url = rca_release_url(slug);

    // Unpack into the real destination in one shot, as the remote side does.
    let script = format!(
        "set -e; cd '{dir}'; curl -fsSL '{url}' | tar xz; chmod +x rca; test -x '{dir}/rca'"
    );
    let out = crate::process_util::shell_command(&script)
        .output()
        .map_err(|e| format!("cannot run the rca download ({e})"))?;
    if !out.status.success() {
        return Err(format!(
            "downloading rca for {slug} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let rca = bin_dir.join("rca");
    let rca_path = rca.to_string_lossy().into_owned();
    verify_local_rca_stdio(&rca_path)?;

    // A configured-but-missing `rcaPath` override short-circuits the whole
    // chain ahead of `~/.fleet/bin`, so a stale override would shadow the copy
    // we just installed and the spawn would still fail. Say so here rather
    // than letting the wizard report success.
    let cfg = load();
    if let Err(e) = resolve_rca_binary(&RemoteWorkspace::default(), &cfg) {
        return Err(format!(
            "rca was installed at {rca_path}, but it is shadowed by a stale override: {e}"
        ));
    }
    Ok(rca_path)
}

/// Fail fast when the installed rca predates the stdio-over-ssh transport —
/// the published release has lagged `serve --stdio` landing on rca main before
/// (see the guard on the remote side). Checking the same `serve -h` text keeps
/// both halves honest about the same capability.
fn verify_local_rca_stdio(rca_path: &str) -> Result<(), String> {
    let out = crate::process_util::command(rca_path)
        .args(["serve", "-h"])
        .output()
        .map_err(|e| format!("cannot run {rca_path} ({e})"))?;
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !help.to_lowercase().contains("stdio") {
        return Err(format!(
            "the rca at {rca_path} has no `serve --stdio` support — its published release \
             predates the stdio-over-ssh transport. Wait for a newer remote-adapter release \
             and re-run."
        ));
    }
    Ok(())
}

/// Stable machine-readable prefixes on the errors a *session launch* can fail
/// with, so the UI can render them in the user's language instead of pasting an
/// English sentence written for a developer.
///
/// Only launch-path failures carry a code: those are the ones a user meets
/// without having asked for anything technical (they pressed "start session").
/// Validation errors raised while registering are already next to the field
/// that caused them. The prose after the code stays meaningful on its own —
/// anything that does not recognise the code shows the sentence unchanged.
pub mod codes {
    /// No rca on this machine at all.
    pub const NO_LOCAL_RCA: &str = "rca:no-local-rca";
    /// A configured `rcaPath` override points at a file that is not there.
    pub const BAD_RCA_OVERRIDE: &str = "rca:bad-rca-override";
    /// The entry's `hostId` names a host that has been deleted.
    pub const HOST_GONE: &str = "rca:host-gone";
    /// The entry has no usable transport (neither host/ssh target nor code).
    pub const NO_TRANSPORT: &str = "rca:no-transport";
    /// The transport died *mid-session*: rca lost its link to the remote host
    /// (ssh dropped, remote rebooted, network went away). Unlike the four above
    /// this is not a launch failure — it is raised by the stderr monitor in
    /// [`crate::remote_disconnect`] after the session was already running, and
    /// travels on `SessionInfo.remote_disconnect` rather than on a command's
    /// `Err`.
    pub const TRANSPORT_LOST: &str = "rca:transport-lost";
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
/// Put rca's own transport flags where rca will actually read them.
///
/// rca's flag parsing stops at the first bare `--` (Go's `flag` package), so
/// flags appended after one are simply never seen — rca exits with
/// "exactly one of --code, --peer, --sock or --via is required (got 0)".
/// Codex's argv always carries such a separator (`… -- "<prompt>"`, see
/// `codex_launch::build_codex_spawn_args`), which is why appending was fine for
/// claude and fatal for codex.
///
/// The duplicated `--` is load-bearing, not belt-and-braces: rca **consumes**
/// the separator it stops at, so inserting the flags before a lone `--` would
/// hand codex its prompt with no separator left — and codex puts one there
/// precisely so a prompt beginning with `-` isn't parsed as a flag. Adding one
/// back means the child receives its original argv byte for byte.
///
/// An argv with no separator (claude's) is appended to exactly as before, and
/// no separator is invented.
fn splice_transport_flags(argv: &mut Vec<String>, flags: Vec<String>) {
    // Skip index 0 — that's the wrapped program's own path, never a separator.
    match argv.iter().skip(1).position(|a| a == "--").map(|i| i + 1) {
        Some(sep) => {
            let mut splice = flags;
            splice.push("--".to_string());
            argv.splice(sep..sep, splice);
        }
        None => argv.extend(flags),
    }
}

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
    let transport_flags: Vec<String> = match transport {
        Transport::Pairing(code) => vec!["--code".to_string(), code],
        Transport::Stdio { ssh_target, remote_rca } => {
            // `--via '<shell cmd>'` — rca runs it under `sh -c` and speaks a
            // single yamux stream over its stdin/stdout. ServerAliveInterval
            // keeps the ssh tunnel from silently half-dying on an idle
            // session; both fields are shell-token-validated in `transport()`.
            vec![
                "--via".to_string(),
                format!(
                    "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 {ssh_target} \
                     {remote_rca} serve --stdio"
                ),
            ]
        }
    };
    let mut wrapped = Vec::with_capacity(args.len() + transport_flags.len() + 2);
    wrapped.push(program.to_string());
    wrapped.extend_from_slice(args);
    splice_transport_flags(&mut wrapped, transport_flags);
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

    /// Codex's argv always ends with `-- "<prompt>"`, and rca's flag parsing
    /// stops dead at a bare `--`. Appending the transport flags after it means
    /// rca never sees them and refuses to start:
    ///
    /// ```text
    /// rca: exactly one of --code, --peer, --sock or --via is required (got 0)
    /// ```
    ///
    /// Measured against the real rca v0.2.0 with Fleet's own argv shape, so this
    /// is the whole feature broken, not a cosmetic ordering nit — no codex
    /// session on a remote workspace could ever launch.
    ///
    /// The extra `--` is not decoration: rca **consumes** the separator it stops
    /// at (Go's `flag` package drops it), so a single `--` would leave codex
    /// receiving the prompt bare. Codex puts it there precisely so a prompt
    /// starting with `-` isn't parsed as a flag; the duplicate keeps one for
    /// codex after rca has eaten its own. Both halves verified end to end
    /// against the real binary.
    #[test]
    fn wrap_launch_puts_transport_flags_before_a_bare_separator() {
        let home = TmpHome::new("sep-wrap");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = stdio_entry(&ws, "gpu-box");
        e.rca_path = Some(fake_rca.clone());
        upsert(e).unwrap();

        // The shape `codex_launch::build_codex_*_args` produces.
        let args: Vec<String> = ["exec", "--json", "-c", "x=y", "--", "写点什么"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = wrap_launch(&ws, "/usr/local/bin/codex", &args).unwrap().unwrap();

        let via = got.args.iter().position(|a| a == "--via").expect("must carry --via");
        let sep = got.args.iter().position(|a| a == "--").expect("must keep a separator");
        assert!(
            via < sep,
            "rca stops parsing at the bare `--`, so its flags must come first: {:?}",
            got.args
        );
        // What the child ends up with after rca eats one `--`: codex's original
        // argv, separator and all.
        assert_eq!(
            got.args,
            vec![
                "/usr/local/bin/codex",
                "exec",
                "--json",
                "-c",
                "x=y",
                "--via",
                "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 gpu-box rca serve --stdio",
                "--",
                "--",
                "写点什么",
            ],
            "the child must still receive its own `--` after rca consumes one",
        );
    }

    /// The pairing-code transport inserts at the same position, so it breaks and
    /// is fixed the same way.
    #[test]
    fn wrap_launch_pairing_code_also_lands_before_the_separator() {
        let home = TmpHome::new("sep-code");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        upsert(RemoteWorkspace {
            path: ws.clone(),
            pairing_code: Some("rca1.CODE".into()),
            rca_path: Some(fake_rca),
            ..Default::default()
        })
        .unwrap();

        let args: Vec<String> =
            ["exec", "--", "hi"].iter().map(|s| s.to_string()).collect();
        let got = wrap_launch(&ws, "/bin/codex", &args).unwrap().unwrap();
        assert_eq!(
            got.args,
            vec!["/bin/codex", "exec", "--code", "rca1.CODE", "--", "--", "hi"],
        );
    }

    /// Claude's argv has no bare `--`, so nothing about its launch may change —
    /// the flags stay appended at the end and no separator is invented.
    #[test]
    fn wrap_launch_without_a_separator_is_untouched() {
        let home = TmpHome::new("sep-none");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = stdio_entry(&ws, "gpu-box");
        e.rca_path = Some(fake_rca);
        upsert(e).unwrap();

        let args: Vec<String> = ["-p", "hi", "--session-id", "s1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = wrap_launch(&ws, "/usr/local/bin/claude", &args).unwrap().unwrap();
        assert_eq!(
            got.args,
            vec![
                "/usr/local/bin/claude",
                "-p",
                "hi",
                "--session-id",
                "s1",
                "--via",
                "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 gpu-box rca serve --stdio",
            ],
        );
        assert!(!got.args.contains(&"--".to_string()), "must not invent a separator");
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
    /// shell metacharacters must be rejected at registration. Spaces in the ssh
    /// target are NOT rejected — they carry ssh option words (see D1).
    #[test]
    fn upsert_rejects_shell_metachars() {
        let home = TmpHome::new("shellmeta");
        for bad in ["host; rm -rf /", "host$(whoami)", "host`id`", "h|p", "a\"b", "x&y"] {
            assert!(
                upsert(stdio_entry(&home.path("proj"), bad)).is_err(),
                "ssh target {bad:?} must be refused"
            );
        }
        let mut e = stdio_entry(&home.path("proj"), "safe-host");
        e.remote_rca_path = Some("/opt/rca; evil".into());
        assert!(upsert(e).is_err(), "remote rca path with metachars must be refused");
        // A remote rca path with a space is also refused (single argv word).
        let mut sp = stdio_entry(&home.path("proj"), "safe-host");
        sp.remote_rca_path = Some("/opt/my rca".into());
        assert!(upsert(sp).is_err(), "remote rca path with a space must be refused");
    }

    /// D1: an ssh target carrying full connection options (custom port /
    /// identity / jump host) round-trips and slots verbatim into the `--via`
    /// command — spaces split into ssh argv words.
    #[test]
    fn wrap_launch_stdio_ssh_target_with_full_args() {
        let home = TmpHome::new("stdio-fullargs");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = stdio_entry(&ws, "-p 2222 -i /home/me/.ssh/id_ed25519 -J bastion me@gpu-box");
        e.remote_rca_path = Some("/opt/rca".into());
        e.rca_path = Some(fake_rca);
        upsert(e).unwrap();

        let saved = load().workspaces.into_iter().find(|w| w.path == ws).unwrap();
        assert_eq!(
            saved.ssh_target.as_deref(),
            Some("-p 2222 -i /home/me/.ssh/id_ed25519 -J bastion me@gpu-box")
        );
        let got = wrap_launch(&ws, "claude", &[]).unwrap().unwrap();
        assert_eq!(
            got.args.last().unwrap(),
            "ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 \
             -p 2222 -i /home/me/.ssh/id_ed25519 -J bastion me@gpu-box /opt/rca serve --stdio"
        );
    }

    // ── host-book-backed entries ─────────────────────────────────────────

    fn book_host(id: &str, user: &str, h: &str) -> crate::remote_host::SshHost {
        crate::remote_host::SshHost {
            id: id.into(),
            label: String::new(),
            host: h.into(),
            port: 22,
            username: user.into(),
            ..Default::default()
        }
    }

    /// The whole reason `host_id` exists: editing a host's address must carry
    /// its workspaces along instead of leaving them dialling the old one.
    #[test]
    fn editing_a_hosts_address_follows_through_to_its_workspaces() {
        let home = TmpHome::new("hostid-follow");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        crate::remote_host::upsert_host(book_host("h1", "dev", "box")).unwrap();
        upsert(RemoteWorkspace {
            path: ws.clone(),
            host_id: Some("h1".into()),
            rca_path: Some(fake_rca),
            ..Default::default()
        })
        .unwrap();

        let before = wrap_launch(&ws, "claude", &[]).unwrap().unwrap();
        assert!(before.args.last().unwrap().contains("dev@box"), "{:?}", before.args);

        // The user edits the host: new port, new user.
        let mut moved = book_host("h1", "ops", "box");
        moved.port = 2222;
        crate::remote_host::upsert_host(moved).unwrap();

        let after = wrap_launch(&ws, "claude", &[]).unwrap().unwrap();
        assert!(
            after.args.last().unwrap().contains("-p 2222 ops@box"),
            "the workspace must follow its host: {:?}",
            after.args
        );
    }

    /// Deleting the host must fail loudly at launch, not quietly dial whatever
    /// address happened to be cached — that surprise is what `host_id` is for.
    #[test]
    fn a_workspace_whose_host_was_deleted_fails_with_a_pointed_message() {
        let home = TmpHome::new("hostid-gone");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        crate::remote_host::upsert_host(book_host("h1", "dev", "box")).unwrap();
        upsert(RemoteWorkspace {
            path: ws.clone(),
            host_id: Some("h1".into()),
            rca_path: Some(fake_rca),
            ..Default::default()
        })
        .unwrap();
        crate::remote_host::save_hosts(&[]).unwrap();

        let err = wrap_launch(&ws, "claude", &[]).unwrap_err();
        // Assert the CODE, not the prose: the sentence is meant to be reworded
        // (and localised in the UI), the code is the contract.
        assert!(err.contains(codes::HOST_GONE), "{err}");
    }

    /// The remote rca path now lives on the host record, so one install serves
    /// every workspace on that host.
    #[test]
    fn the_remote_rca_path_comes_from_the_host_record() {
        let home = TmpHome::new("hostid-rca");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut h = book_host("h1", "dev", "box");
        h.rca_path = Some("/root/.fleet/bin/rca".into());
        crate::remote_host::upsert_host(h).unwrap();
        upsert(RemoteWorkspace {
            path: ws.clone(),
            host_id: Some("h1".into()),
            rca_path: Some(fake_rca),
            ..Default::default()
        })
        .unwrap();

        let got = wrap_launch(&ws, "claude", &[]).unwrap().unwrap();
        assert!(
            got.args.last().unwrap().contains("/root/.fleet/bin/rca serve --stdio"),
            "{:?}",
            got.args
        );
    }

    /// Entries written before the host book existed keep working verbatim.
    #[test]
    fn a_legacy_ssh_target_entry_still_launches() {
        let home = TmpHome::new("hostid-legacy");
        let ws = home.path("proj");
        let fake_rca = home.path("rca-bin");
        fs::write(&fake_rca, "").unwrap();
        let mut e = stdio_entry(&ws, "own-api-ko");
        e.rca_path = Some(fake_rca);
        upsert(e).unwrap();
        let got = wrap_launch(&ws, "claude", &[]).unwrap().unwrap();
        assert!(got.args.last().unwrap().contains("own-api-ko rca serve --stdio"), "{:?}", got.args);
    }

    // ── local installer ──────────────────────────────────────────────────

    #[test]
    fn rca_release_slug_maps_uname_to_go_arch_names() {
        assert_eq!(rca_release_slug("Linux x86_64"), Some("linux_amd64"));
        assert_eq!(rca_release_slug("Linux aarch64"), Some("linux_arm64"));
        assert_eq!(rca_release_slug("Darwin arm64"), Some("darwin_arm64"));
        assert_eq!(rca_release_slug("Darwin x86_64"), Some("darwin_amd64"));
        assert_eq!(rca_release_slug("FreeBSD amd64"), None);
        assert_eq!(rca_release_slug("Linux riscv64"), None);
    }

    /// The local slug must come out of the same table the remote side uses —
    /// `std::env::consts` spells the arch "aarch64"/"x86_64", the releases
    /// spell it "arm64"/"amd64", and getting that backwards 404s the download.
    #[test]
    fn local_release_slug_translates_std_consts_to_a_published_slug() {
        let got = local_release_slug();
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => assert_eq!(got, Some("darwin_arm64")),
            ("macos", "x86_64") => assert_eq!(got, Some("darwin_amd64")),
            ("linux", "aarch64") => assert_eq!(got, Some("linux_arm64")),
            ("linux", "x86_64") => assert_eq!(got, Some("linux_amd64")),
            // Windows and friends publish nothing — the installer must refuse
            // rather than build a URL that 404s.
            _ => assert_eq!(got, None),
        }
    }

    #[test]
    fn rca_release_url_points_at_latest_not_a_pinned_tag() {
        let url = rca_release_url("darwin_arm64");
        assert!(url.contains("/releases/latest/download/"), "{url}");
        assert!(url.ends_with("rca_darwin_arm64.tar.gz"), "{url}");
    }

    /// A stub standing in for an rca binary, so the stdio probe is exercised
    /// without a network download.
    #[cfg(unix)]
    fn stub_rca(dir: &Path, name: &str, help_text: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\ncat <<'EOF'\n{help_text}\nEOF\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn stdio_probe_accepts_a_release_that_has_the_transport() {
        let home = TmpHome::new("probe-ok");
        let rca = stub_rca(
            &home.dir,
            "rca-new",
            "  -stdio ssh host rca serve --stdio\n\tserve over stdin/stdout via yamux",
        );
        assert!(verify_local_rca_stdio(&rca).is_ok());
    }

    /// The published release has lagged `serve --stdio` landing on rca main
    /// before; installing that build must fail loudly here instead of at the
    /// user's first session spawn.
    /// The real thing: download the published release for this machine into
    /// `~/.fleet/bin/rca` and probe it. Ignored by default — it hits the
    /// network and writes to the real home, which no unit test should do
    /// unasked. Run with `cargo test -p claw-fleet-core --lib
    /// install_local_rca_really_installs -- --ignored --nocapture` to verify
    /// the installer against the live release.
    #[test]
    #[ignore = "hits the network and writes to the real ~/.fleet/bin"]
    fn install_local_rca_really_installs_a_working_binary() {
        let path = install_local_rca().expect("install");
        assert!(Path::new(&path).is_file(), "{path} was not created");
        verify_local_rca_stdio(&path).expect("installed rca must speak --stdio");
        assert_eq!(find_local_rca().as_deref(), Some(path.as_str()));
        eprintln!("installed rca at {path}");
    }

    #[cfg(unix)]
    #[test]
    fn stdio_probe_rejects_a_release_predating_the_transport() {
        let home = TmpHome::new("probe-stale");
        let rca = stub_rca(&home.dir, "rca-old", "Serve flags: --listen, --sock, --relays");
        let err = verify_local_rca_stdio(&rca).unwrap_err();
        assert!(err.contains("serve --stdio"), "{err}");
    }
}
