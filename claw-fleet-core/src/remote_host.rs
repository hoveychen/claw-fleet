//! The SSH host book, and the operations Fleet runs against a host.
//!
//! ## One host, two capabilities
//!
//! A machine reachable over ssh can serve Fleet in two unrelated ways:
//!
//! - **as the Fleet backend** — the whole app runs there and this desktop is a
//!   shell over its HTTP probe ([`crate::backend`]'s remote implementation);
//! - **as an rca executor** — the agent stays local and only the workspace's
//!   file I/O is routed there ([`crate::remote_workspace`]).
//!
//! They used to be modelled as two unrelated lists, both labelled "远端" in the
//! UI, so the same machine had to be configured twice and the word meant two
//! different things on one screen. [`SshHost`] is the single record; the two
//! capabilities are fields on it. The file is unchanged
//! (`~/.fleet/fleet-connections.json`) and the added fields are all optional,
//! so an existing book loads as-is.
//!
//! ## Two capabilities, both a single ssh round trip:
//!
//! - [`browse_remote_dir`] — list the directories one level under a remote
//!   path, so the session composer can walk to a repo instead of asking the
//!   user to type an absolute path they must get byte-identical on two
//!   machines. It returns [`crate::workspace_browse::BrowseDirResponse`], the
//!   SAME type the local picker returns, so one client component renders both.
//! - [`host_health`] — is the host reachable, is rca installed, does that rca
//!   speak `serve --stdio`. Today the only way to learn a registered workspace
//!   is dead is to start a session and watch it fail.
//!
//! ## Security boundary
//!
//! Deliberately different from [`crate::workspace_browse`], which confines the
//! phone to home + known workspaces. Here the boundary is **ssh itself**: the
//! caller already holds a key that opens a shell on that host, so listing
//! directory names grants nothing a plain `ssh host ls` would not. What IS
//! enforced: the ssh target is metachar-validated before it reaches a shell,
//! and paths carrying a single quote are refused rather than quoted heuristically.
//!
//! ## Why the remote side is one shell script per call
//!
//! An ssh handshake costs the better part of a second. Listing a directory as
//! `ls` + one `test -e .git` per entry over separate connections would be
//! unusable, so each capability ships one script and parses a tab-separated
//! reply. `ls -1ApL` is the portable spine of the listing: verified 2026-09-01
//! on BSD ls (macOS 15) and GNU coreutils ls (Linux) to append `/` to real
//! directories AND symlinks-to-directories, and to nothing else.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::workspace_browse::{BrowseDirResponse, BrowseEntry};

/// One SSH host Fleet knows about.
///
/// The serde shape is byte-compatible with the old desktop-only
/// `RemoteConnection` record — same file, same field names — so an existing
/// `fleet-connections.json` loads unchanged and a downgrade still reads it.
/// Everything added for the rca capability is `Option` + `skip_serializing_if`,
/// so a host that is only ever a Fleet backend serialises exactly as before.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    pub id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_file: Option<String>,
    /// Optional jump/bastion host (SSH ProxyJump, e.g. "user@bastion:22").
    pub jump_host: Option<String>,
    /// If set, use this SSH config profile name instead of manual
    /// host/user/port/key.
    pub ssh_profile: Option<String>,
    /// **rca capability.** Absolute path of the rca installed on this host.
    /// Its presence is what makes the host usable as an rca executor; the
    /// installer sets it. `None` = "this host is not (yet) an executor".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rca_path: Option<String>,
}

fn host_book_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("fleet-connections.json"))
}

/// Read the host book. A missing or corrupt file is an empty book, not an
/// error — the app must still start.
pub fn load_hosts() -> Vec<SshHost> {
    let Some(path) = host_book_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn save_hosts(hosts: &[SshHost]) -> Result<(), String> {
    let path = host_book_path().ok_or("cannot determine home dir")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(hosts).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())
}

pub fn find_host(id: &str) -> Option<SshHost> {
    load_hosts().into_iter().find(|h| h.id == id)
}

/// Insert or replace by `id`, preserving list order for an update.
pub fn upsert_host(host: SshHost) -> Result<Vec<SshHost>, String> {
    let mut hosts = load_hosts();
    match hosts.iter_mut().find(|h| h.id == host.id) {
        Some(slot) => *slot = host,
        None => hosts.push(host),
    }
    save_hosts(&hosts)?;
    Ok(hosts)
}

/// Record that this host now has an rca at `rca_path` — the one field the
/// installer owns. Read-modify-write of the single host rather than the whole
/// book, so it cannot clobber a concurrent edit to a different host's address.
pub fn set_host_rca_path(id: &str, rca_path: &str) -> Result<(), String> {
    let mut hosts = load_hosts();
    let slot = hosts
        .iter_mut()
        .find(|h| h.id == id)
        .ok_or_else(|| format!("no ssh host with id {id}"))?;
    slot.rca_path = Some(rca_path.to_string());
    save_hosts(&hosts)
}

/// A deterministic id for a host that arrived without one, derived from its ssh
/// target so the same machine always lands on the same id.
fn derive_host_id(ssh_target: &str) -> String {
    let slug: String = ssh_target
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    format!("ssh-{}", slug.trim_matches('-'))
}

/// Make sure `candidate` is a record in the host book and return the id to
/// reference it by.
///
/// The rca install wizard can hand us three kinds of candidate: a saved host
/// (already in the book), a `~/.ssh/config` alias, or a hand-typed
/// `user@host`. The latter two are synthesised client-side with placeholder ids
/// — notably the literal `"manual"`, which every hand-typed host would share.
/// Persisting those verbatim would either collide or scatter duplicates, so:
///
/// 1. an id already in the book is taken as-is;
/// 2. otherwise a host whose ssh target resolves identically IS this machine —
///    reuse that record, which is the whole point of one host list (setting up
///    rca on a box you already saved as a Fleet backend must not create a
///    second row for it);
/// 3. otherwise it is genuinely new: store it under a target-derived id.
pub fn adopt_host(candidate: &SshHost) -> Result<String, String> {
    let target = ssh_target_for(candidate)?;
    let hosts = load_hosts();
    if let Some(h) = hosts.iter().find(|h| h.id == candidate.id) {
        return Ok(h.id.clone());
    }
    if let Some(h) =
        hosts.iter().find(|h| ssh_target_for(h).ok().as_deref() == Some(target.as_str()))
    {
        return Ok(h.id.clone());
    }
    let id = derive_host_id(&target);
    if let Some(h) = hosts.iter().find(|h| h.id == id) {
        return Ok(h.id.clone());
    }
    let mut host = candidate.clone();
    host.id = id.clone();
    if host.label.trim().is_empty() {
        host.label = target;
    }
    upsert_host(host)?;
    Ok(id)
}

/// Build the ssh argument fragment for `host` — what goes into
/// `ssh <fragment> <remote-rca> serve --stdio`.
///
/// An ssh-config profile resolves host/user/port/key on its own, so it is just
/// the alias. A manual host is expanded into its option words — `-p <port>`
/// (when not 22), `-i <key>`, `-J <jump>`, then `user@host` — which `sh -c`
/// splits back into argv for ssh.
pub fn ssh_target_for(host: &SshHost) -> Result<String, String> {
    if let Some(profile) = host.ssh_profile.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(profile.to_string());
    }
    let h = host.host.trim();
    let u = host.username.trim();
    if h.is_empty() || u.is_empty() {
        return Err("connection is missing a username or host".to_string());
    }
    let mut parts: Vec<String> = Vec::new();
    if host.port != 22 {
        parts.push(format!("-p {}", host.port));
    }
    if let Some(key) = host.identity_file.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("-i {key}"));
    }
    if let Some(jump) = host.jump_host.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("-J {jump}"));
    }
    parts.push(format!("{u}@{h}"));
    Ok(parts.join(" "))
}

/// Matches [`crate::workspace_browse`]'s cap — a picker is for walking a tree
/// by hand, and the listing has to fit in one response frame.
const MAX_ENTRIES: usize = 500;

/// Reject the characters that would let a stored ssh target inject into the
/// `sh -c` that `rca --via` eventually wraps it in. Mirrors
/// `remote_workspace`'s validator (spaces ARE allowed — the target may be a
/// full `-p 2222 -i /key user@host` fragment).
const SHELL_METACHARS: &[char] =
    &[';', '&', '|', '$', '`', '"', '\'', '\\', '\n', '\r', '(', ')', '<', '>', '*', '?', '[', ']', '{', '}', '!', '#', '~'];

fn validate_ssh_target(target: &str) -> Result<(), String> {
    let t = target.trim();
    if t.is_empty() {
        return Err("ssh target must not be empty".to_string());
    }
    if let Some(c) = t.chars().find(|c| SHELL_METACHARS.contains(c)) {
        return Err(format!("ssh target contains an unsupported character {c:?}"));
    }
    Ok(())
}

/// Run `remote_cmd` on `ssh_target` and return its stdout.
///
/// `ssh_target` is split on whitespace into argv words — that is what lets it
/// carry `-p 2222 -i /key -J jump user@host` as well as a bare ssh-config
/// alias. `BatchMode=yes` means a host needing an interactive passphrase fails
/// fast instead of hanging on a prompt nobody can see.
pub fn ssh_exec(ssh_target: &str, remote_cmd: &str) -> Result<String, String> {
    validate_ssh_target(ssh_target)?;
    let mut args: Vec<String> = vec![
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "ConnectTimeout=15".into(),
        "-o".into(),
        "BatchMode=yes".into(),
    ];
    args.extend(ssh_target.split_whitespace().map(String::from));
    args.push(remote_cmd.to_string());

    let mut cmd = crate::process_util::command("ssh");
    cmd.args(&args);
    // ssh reads ~/.ssh/config from $HOME; a polluted $HOME would silently lose
    // every Host alias, so pin it to the real one.
    if let Some(home) = crate::session::real_home_dir() {
        cmd.env("HOME", home);
    }
    let out = cmd.output().map_err(|e| format!("cannot run ssh: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { "ssh failed with no output".to_string() } else { err })
    }
}

/// A path is embedded in the remote script inside single quotes; one containing
/// a single quote cannot be quoted that way and is refused rather than escaped
/// by hand. Absolute paths never legitimately need one.
fn check_quotable(path: &str) -> Result<(), String> {
    if path.contains('\'') {
        return Err("path must not contain a single quote".to_string());
    }
    Ok(())
}

/// The remote script for [`browse_remote_dir`]. Emits, tab-separated:
/// `HOME<TAB><remote home>`, `PWD<TAB><canonical dir>`, then one
/// `D<TAB>0|1<TAB><name>` per subdirectory (the flag is "has a .git").
///
/// `head -n <cap+1>` runs BEFORE the per-entry `.git` test so a directory with
/// 50k children costs 501 stats, not 50k.
fn browse_script(path: &str) -> String {
    let cap = MAX_ENTRIES + 1;
    format!(
        "d='{path}'; \
         printf 'HOME\\t%s\\n' \"$HOME\"; \
         cd \"${{d:-$HOME}}\" 2>/dev/null || {{ printf 'ERR\\t%s\\n' 'not a directory'; exit 0; }}; \
         printf 'PWD\\t%s\\n' \"$(pwd -P)\"; \
         ls -1ApL 2>/dev/null | grep '/$' | head -n {cap} | while IFS= read -r n; do \
           b=${{n%/}}; \
           if [ -e \"$b/.git\" ]; then printf 'D\\t1\\t%s\\n' \"$b\"; \
           else printf 'D\\t0\\t%s\\n' \"$b\"; fi; \
         done"
    )
}

/// Join a canonical parent with a child name. Plain string work rather than
/// `Path::join`: these are REMOTE paths, and on a Windows agent `Path` would
/// join them with a backslash.
fn remote_join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// The parent of a canonical remote path, or `None` at the root.
fn remote_parent(dir: &str) -> Option<String> {
    if dir == "/" || dir.is_empty() {
        return None;
    }
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => None,
    }
}

/// Parse the tab-separated reply of [`browse_script`].
///
/// Split out from the ssh call so the parsing — the part with the edge cases —
/// is unit-testable without a host.
fn parse_browse_reply(reply: &str) -> Result<BrowseDirResponse, String> {
    let mut home = String::new();
    let mut pwd: Option<String> = None;
    let mut entries: Vec<BrowseEntry> = Vec::new();
    let mut truncated = false;

    for line in reply.lines() {
        let mut parts = line.split('\t');
        match parts.next() {
            Some("HOME") => home = parts.next().unwrap_or_default().to_string(),
            Some("PWD") => pwd = Some(parts.next().unwrap_or_default().to_string()),
            Some("ERR") => {
                return Err(parts.next().unwrap_or("remote listing failed").to_string())
            }
            Some("D") => {
                let is_git_repo = parts.next() == Some("1");
                // The name is the REST of the line: a directory name may itself
                // contain a tab, and rejoining keeps it intact instead of
                // silently truncating at the first one.
                let name = parts.collect::<Vec<_>>().join("\t");
                if name.is_empty() {
                    continue;
                }
                if entries.len() >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }
                entries.push(BrowseEntry { name, path: String::new(), is_git_repo });
            }
            _ => {}
        }
    }

    let path = pwd.ok_or("remote listing returned no directory")?;
    for e in &mut entries {
        e.path = remote_join(&path, &e.name);
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(BrowseDirResponse {
        parent: remote_parent(&path),
        path,
        entries,
        truncated,
        roots: if home.is_empty() { Vec::new() } else { vec![home] },
    })
}

/// List the directories one level under `path` on `ssh_target`. `None` or an
/// empty path starts at the remote `$HOME`.
pub fn browse_remote_dir(
    ssh_target: &str,
    path: Option<&str>,
) -> Result<BrowseDirResponse, String> {
    let path = path.unwrap_or("").trim();
    check_quotable(path)?;
    let reply = ssh_exec(ssh_target, &browse_script(path))?;
    parse_browse_reply(&reply)
}

/// What a health probe learned about one host. Every field is optional-ish
/// because a probe that fails early still reports what it got: `ssh_ok: false`
/// with an `error` is a useful answer, not an exception.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostHealth {
    /// The ssh connection itself succeeded.
    pub ssh_ok: bool,
    /// Remote `$HOME`, useful for suggesting a workspace path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Absolute path of the rca found on the host, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rca_path: Option<String>,
    /// First line of `rca version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rca_version: Option<String>,
    /// That rca supports `serve --stdio` — the transport Fleet actually uses.
    /// An rca that is present but too old is a distinct failure from no rca.
    pub stdio_ok: bool,
    /// Why the probe is not fully green, in the user's terms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HostHealth {
    /// Everything a session launch needs is in place.
    pub fn is_ready(&self) -> bool {
        self.ssh_ok && self.rca_path.is_some() && self.stdio_ok
    }
}

/// Resolve rca the same way the installer places it (`~/.fleet/bin/rca`) with a
/// `$PATH` fallback, then report its version and whether it speaks `--stdio`.
fn health_script() -> String {
    "printf 'HOME\\t%s\\n' \"$HOME\"; \
     if [ -x \"$HOME/.fleet/bin/rca\" ]; then r=\"$HOME/.fleet/bin/rca\"; \
     else r=$(command -v rca 2>/dev/null); fi; \
     if [ -n \"$r\" ]; then \
       printf 'RCA\\t%s\\n' \"$r\"; \
       printf 'VERSION\\t%s\\n' \"$(\"$r\" version 2>&1 | head -n1)\"; \
       if \"$r\" serve -h 2>&1 | grep -qi stdio; then printf 'STDIO\\t1\\n'; \
       else printf 'STDIO\\t0\\n'; fi; \
     fi"
        .to_string()
}

/// Parse [`health_script`]'s reply into a [`HostHealth`]. Separate from the ssh
/// call so the "rca missing" / "rca too old" distinction is unit-testable.
fn parse_health_reply(reply: &str) -> HostHealth {
    let mut h = HostHealth { ssh_ok: true, ..Default::default() };
    for line in reply.lines() {
        let mut parts = line.split('\t');
        let val = |p: &mut std::str::Split<'_, char>| p.next().unwrap_or_default().to_string();
        match parts.next() {
            Some("HOME") => h.home = Some(val(&mut parts)).filter(|s| !s.is_empty()),
            Some("RCA") => h.rca_path = Some(val(&mut parts)).filter(|s| !s.is_empty()),
            Some("VERSION") => h.rca_version = Some(val(&mut parts)).filter(|s| !s.is_empty()),
            Some("STDIO") => h.stdio_ok = parts.next() == Some("1"),
            _ => {}
        }
    }
    h.error = match (&h.rca_path, h.stdio_ok) {
        (None, _) => Some(
            "rca is not installed on this host — run the installer to put it in ~/.fleet/bin"
                .to_string(),
        ),
        (Some(p), false) => Some(format!(
            "the rca at {p} has no `serve --stdio` support — its release predates the \
             stdio-over-ssh transport; re-run the installer to update it"
        )),
        (Some(_), true) => None,
    };
    h
}

/// Probe one host. Never returns `Err`: an unreachable host is a *result*
/// (`ssh_ok: false` plus the ssh error), because the caller is a status badge,
/// not a control-flow branch.
pub fn host_health(ssh_target: &str) -> HostHealth {
    match ssh_exec(ssh_target, &health_script()) {
        Ok(reply) => parse_health_reply(&reply),
        Err(e) => HostHealth { ssh_ok: false, error: Some(e), ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FLEET_HOME`-redirected temp home so the host book under test is never
    /// the developer's real `~/.fleet/fleet-connections.json`.
    struct TmpHome {
        dir: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-hostbook-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialized on the process-wide FLEET_HOME lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
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
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn host(id: &str, user: &str, h: &str) -> SshHost {
        SshHost {
            id: id.into(),
            label: String::new(),
            host: h.into(),
            port: 22,
            username: user.into(),
            ..Default::default()
        }
    }

    #[test]
    fn ssh_target_for_builds_a_full_arg_fragment() {
        let base = host("x", "dev", "box");
        // Profile alias wins — it resolves host/user/port/key on its own.
        let mut c = base.clone();
        c.ssh_profile = Some("gpu-box".into());
        assert_eq!(ssh_target_for(&c).unwrap(), "gpu-box");
        // Plain default-port host → bare user@host (no redundant -p 22).
        assert_eq!(ssh_target_for(&base).unwrap(), "dev@box");
        // Custom port / identity / jump expand into option words.
        let mut full = base.clone();
        full.port = 2222;
        full.identity_file = Some("/home/me/.ssh/id".into());
        full.jump_host = Some("bastion".into());
        assert_eq!(
            ssh_target_for(&full).unwrap(),
            "-p 2222 -i /home/me/.ssh/id -J bastion dev@box"
        );
        let mut p = base.clone();
        p.port = 2222;
        assert_eq!(ssh_target_for(&p).unwrap(), "-p 2222 dev@box");
        let mut bad = base.clone();
        bad.host = String::new();
        assert!(ssh_target_for(&bad).is_err());
    }

    /// The old desktop record had no rca fields. Loading one must not lose it,
    /// and re-saving must not sprout nulls into a file an older Fleet reads.
    #[test]
    fn a_pre_rca_host_record_round_trips_unchanged() {
        let json = r#"[{"id":"a","label":"box","host":"h","port":22,"username":"u",
                        "identityFile":null,"jumpHost":null,"sshProfile":null}]"#;
        let hosts: Vec<SshHost> = serde_json::from_str(json).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].rca_path, None);
        let back = serde_json::to_string(&hosts[0]).unwrap();
        assert!(!back.contains("rcaPath"), "absent capability must not serialise: {back}");
    }

    #[test]
    fn adopt_host_reuses_the_record_that_is_already_this_machine() {
        let _h = TmpHome::new("adopt-reuse");
        upsert_host(host("saved-1", "dev", "box")).unwrap();

        // The wizard's hand-typed candidate carries the placeholder id
        // "manual", but resolves to the same ssh target — it is the same box.
        let id = adopt_host(&host("manual", "dev", "box")).unwrap();
        assert_eq!(id, "saved-1", "must attach to the existing record, not add a row");
        assert_eq!(load_hosts().len(), 1);
    }

    /// Every hand-typed host arrives as id "manual"; persisting that verbatim
    /// would make the second one overwrite the first.
    #[test]
    fn adopt_host_gives_two_different_manual_hosts_two_records() {
        let _h = TmpHome::new("adopt-manual");
        let a = adopt_host(&host("manual", "dev", "box-a")).unwrap();
        let b = adopt_host(&host("manual", "dev", "box-b")).unwrap();
        assert_ne!(a, b);
        assert_eq!(load_hosts().len(), 2);
        assert!(a.starts_with("ssh-"), "{a}");
        // Same input again is the same record — adoption is idempotent.
        assert_eq!(adopt_host(&host("manual", "dev", "box-a")).unwrap(), a);
        assert_eq!(load_hosts().len(), 2);
    }

    #[test]
    fn adopt_host_labels_an_unlabelled_new_record_with_its_target() {
        let _h = TmpHome::new("adopt-label");
        let id = adopt_host(&host("manual", "dev", "box")).unwrap();
        assert_eq!(find_host(&id).unwrap().label, "dev@box");
    }

    #[test]
    fn set_host_rca_path_marks_only_that_host() {
        let _h = TmpHome::new("set-rca");
        upsert_host(host("a", "dev", "box-a")).unwrap();
        upsert_host(host("b", "dev", "box-b")).unwrap();
        set_host_rca_path("a", "/root/.fleet/bin/rca").unwrap();
        assert_eq!(find_host("a").unwrap().rca_path.as_deref(), Some("/root/.fleet/bin/rca"));
        assert_eq!(find_host("b").unwrap().rca_path, None);
        assert!(set_host_rca_path("nope", "/x").is_err());
    }

    #[test]
    fn ssh_target_rejects_shell_metacharacters_but_allows_arg_fragments() {
        assert!(validate_ssh_target("own-api-ko").is_ok());
        assert!(validate_ssh_target("-p 2222 -i /home/me/key me@box").is_ok());
        assert!(validate_ssh_target("box; rm -rf /").is_err());
        assert!(validate_ssh_target("box`whoami`").is_err());
        assert!(validate_ssh_target("   ").is_err());
    }

    #[test]
    fn a_path_with_a_single_quote_is_refused_not_escaped() {
        assert!(check_quotable("/srv/repo").is_ok());
        assert!(check_quotable("/srv/o'brien").is_err());
    }

    #[test]
    fn browse_script_caps_the_listing_before_the_per_entry_git_test() {
        let s = browse_script("/srv/repo");
        // The cap must sit between `ls` and the loop that stats each entry —
        // otherwise a huge directory costs one stat per child.
        let head = s.find("head -n 501").expect("cap present");
        let loop_at = s.find("while IFS=").expect("loop present");
        assert!(head < loop_at, "cap must precede the per-entry loop");
        assert!(s.contains("ls -1ApL"), "verified-portable listing flags");
    }

    #[test]
    fn remote_parent_walks_up_and_stops_at_the_root() {
        assert_eq!(remote_parent("/srv/git/repo").as_deref(), Some("/srv/git"));
        assert_eq!(remote_parent("/srv").as_deref(), Some("/"));
        assert_eq!(remote_parent("/srv/").as_deref(), Some("/"));
        assert_eq!(remote_parent("/"), None);
        assert_eq!(remote_parent(""), None);
    }

    #[test]
    fn remote_join_never_doubles_the_separator() {
        assert_eq!(remote_join("/srv", "repo"), "/srv/repo");
        assert_eq!(remote_join("/", "srv"), "/srv");
    }

    #[test]
    fn browse_reply_yields_sorted_absolute_entries_with_git_flags() {
        let reply = "HOME\t/root\nPWD\t/srv/git\nD\t0\tzeta\nD\t1\tAlpha\nD\t0\tmid\n";
        let got = parse_browse_reply(reply).unwrap();
        assert_eq!(got.path, "/srv/git");
        assert_eq!(got.parent.as_deref(), Some("/srv"));
        assert_eq!(got.roots, vec!["/root".to_string()]);
        assert!(!got.truncated);
        let names: Vec<_> = got.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "mid", "zeta"], "case-insensitive sort");
        let alpha = got.entries.iter().find(|e| e.name == "Alpha").unwrap();
        assert!(alpha.is_git_repo);
        assert_eq!(alpha.path, "/srv/git/Alpha");
    }

    /// The script emits cap+1 rows precisely so the parser can tell "exactly
    /// full" from "there was more".
    #[test]
    fn browse_reply_flags_truncation_at_the_cap() {
        let mut reply = String::from("HOME\t/root\nPWD\t/srv\n");
        for i in 0..(MAX_ENTRIES + 1) {
            reply.push_str(&format!("D\t0\td{i:05}\n"));
        }
        let got = parse_browse_reply(&reply).unwrap();
        assert!(got.truncated);
        assert_eq!(got.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn browse_reply_surfaces_the_remote_error_row() {
        let err = parse_browse_reply("HOME\t/root\nERR\tnot a directory\n").unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn browse_reply_without_a_pwd_row_is_an_error_not_an_empty_listing() {
        assert!(parse_browse_reply("HOME\t/root\n").is_err());
    }

    /// A directory name containing a tab must survive; splitting naively would
    /// silently hand back a truncated name that then fails to `cd`.
    #[test]
    fn browse_reply_keeps_a_tab_inside_a_directory_name() {
        let got = parse_browse_reply("HOME\t/root\nPWD\t/srv\nD\t0\tweird\tname\n").unwrap();
        assert_eq!(got.entries[0].name, "weird\tname");
        assert_eq!(got.entries[0].path, "/srv/weird\tname");
    }

    #[test]
    fn health_reply_ready_when_rca_present_and_speaks_stdio() {
        let h = parse_health_reply(
            "HOME\t/root\nRCA\t/root/.fleet/bin/rca\nVERSION\trca v0.2.0\nSTDIO\t1\n",
        );
        assert!(h.ssh_ok && h.stdio_ok && h.is_ready());
        assert_eq!(h.rca_path.as_deref(), Some("/root/.fleet/bin/rca"));
        assert_eq!(h.rca_version.as_deref(), Some("rca v0.2.0"));
        assert_eq!(h.error, None);
    }

    /// "rca is too old" must not collapse into "rca is missing" — they need
    /// different fixes (update vs install) and the message says which.
    #[test]
    fn health_reply_distinguishes_a_stale_rca_from_a_missing_one() {
        let stale = parse_health_reply("HOME\t/root\nRCA\t/usr/bin/rca\nSTDIO\t0\n");
        assert!(!stale.is_ready());
        assert!(stale.error.as_deref().unwrap().contains("serve --stdio"), "{stale:?}");

        let missing = parse_health_reply("HOME\t/root\n");
        assert!(!missing.is_ready());
        assert!(missing.error.as_deref().unwrap().contains("not installed"), "{missing:?}");
    }

    /// The scripts against a REAL host — the only thing that proves the shell
    /// is portable and the quoting holds. Ignored by default (needs network and
    /// a reachable box); point it anywhere with
    /// `FLEET_TEST_SSH_TARGET=<host> cargo test -p claw-fleet-core --lib
    /// remote_host::tests::live -- --ignored --nocapture`.
    fn live_target() -> String {
        std::env::var("FLEET_TEST_SSH_TARGET").unwrap_or_else(|_| "own-api-ko".to_string())
    }

    #[test]
    #[ignore = "needs a reachable ssh host"]
    fn live_browse_walks_a_real_remote_tree() {
        let t = live_target();
        let home = browse_remote_dir(&t, None).expect("browse home");
        eprintln!("home={} parent={:?} n={}", home.path, home.parent, home.entries.len());
        assert!(home.path.starts_with('/'), "canonical absolute path: {}", home.path);
        assert_eq!(home.roots.len(), 1, "remote $HOME reported as the root");

        // Walking into the first child must produce a listing whose parent
        // points back — that round trip is what the picker's ".." row rides on.
        if let Some(first) = home.entries.first() {
            let child = browse_remote_dir(&t, Some(&first.path)).expect("browse child");
            assert_eq!(child.path, first.path);
            assert_eq!(child.parent.as_deref(), Some(home.path.as_str()));
        }
    }

    #[test]
    #[ignore = "needs a reachable ssh host"]
    fn live_health_reports_a_ready_host() {
        let h = host_health(&live_target());
        eprintln!("{h:?}");
        assert!(h.ssh_ok, "ssh must connect: {:?}", h.error);
        assert!(h.home.is_some());
    }

    #[test]
    fn an_unreachable_host_is_a_result_not_an_error() {
        // A target that validates but cannot resolve — ssh fails, and the probe
        // must still hand back a renderable status.
        let h = host_health("no-such-host.invalid");
        assert!(!h.ssh_ok);
        assert!(h.error.is_some());
        assert!(!h.is_ready());
    }
}
