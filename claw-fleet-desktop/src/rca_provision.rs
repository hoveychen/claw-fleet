//! rca_provision.rs — desktop-side ssh operations for rca remote workspaces.
//!
//! An rca remote workspace keeps the agent process on THIS machine and routes
//! only the workspace path's file I/O to a remote `rca serve` over ssh
//! (`claw_fleet_core::remote_workspace`). The pieces here are the one-off ssh
//! chores around that: install/upgrade rca on a host and register the
//! workspace, list the user's `~/.ssh/config` profiles for the host picker, and
//! probe/install claude/codex/dsh on the remote host. Every ssh here runs from
//! the desktop, which is also where a session spawns — so none of it goes
//! through `LocalBackend`; these are plain Tauri commands.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use claw_fleet_core::remote_host::{self, SshHost};
use claw_fleet_core::remote_workspace::{self, rca_release_slug, rca_release_url};

// ── Progress event emitted to the wizard ─────────────────────────────────────

/// One step of the rca install wizard, sent on `rca-install-progress`.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub step: String,
    pub done: bool,
    pub error: Option<String>,
    /// When true, the frontend should replace the last progress entry instead of appending.
    pub update_last: bool,
}

fn emit_install_progress(app: &AppHandle, step: &str, done: bool) {
    let _ = app.emit(
        "rca-install-progress",
        InstallProgress { step: step.to_string(), done, error: None, update_last: false },
    );
}

// ── ssh runners ──────────────────────────────────────────────────────────────

/// Run a remote command on a structured host record. The record is expanded
/// into the same `-p … -i … -J … user@host` fragment `wrap_launch` will later
/// bake into `rca --via`, so a host that cannot be addressed at spawn time
/// fails here, at install time, with the same message.
fn ssh_exec(conn: &SshHost, remote_cmd: &str) -> Result<String, String> {
    let target = remote_host::ssh_target_for(conn)?;
    ssh_exec_target(&target, remote_cmd)
}

/// Run a remote command over ssh addressing a raw ssh-target fragment — the
/// stored `ssh_target` (an ssh-config alias, or `-p … -i … -J … user@host`).
/// Core additionally metachar-validates the target, which is strictly a gain
/// here: this target comes from the registry, where `upsert` already applies
/// the same rule.
fn ssh_exec_target(ssh_target: &str, remote_cmd: &str) -> Result<String, String> {
    remote_host::ssh_exec(ssh_target, remote_cmd).map(|s| s.trim().to_string())
}

// ── rca stdio-over-ssh auto-installer ────────────────────────────────────────

/// Detect the remote platform, download + install rca into `~/.fleet/bin`, and
/// verify it supports `serve --stdio`. Returns the absolute remote rca path.
/// Shared by install (structured connection) and update (raw ssh target) via
/// the `ssh` runner closure. `$HOME` is resolved on the remote so the path
/// carries no `~` (a local `sh -c` in `--via` would expand it to the LOCAL home).
fn provision_rca(
    app: &AppHandle,
    ssh: impl Fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    emit_install_progress(app, "Detecting remote platform…", false);
    let uname = ssh("uname -sm").map_err(|e| format!("uname failed: {e}"))?;
    let slug = rca_release_slug(&uname)
        .ok_or_else(|| format!("unsupported remote platform for rca: {uname:?}"))?;

    let url = rca_release_url(slug);
    let install = format!(
        "set -e; mkdir -p \"$HOME/.fleet/bin\"; cd \"$HOME/.fleet/bin\"; \
         curl -fsSL {url} | tar xz; chmod +x rca; test -x \"$HOME/.fleet/bin/rca\"; \
         printf '%s\\n' \"$HOME/.fleet/bin/rca\""
    );
    emit_install_progress(app, &format!("Installing rca ({slug})…"), false);
    let remote_rca = ssh(&install)
        .map_err(|e| format!("rca install failed: {e}"))?
        .lines()
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    if remote_rca.is_empty() {
        return Err("rca install produced no remote path".to_string());
    }

    // Fail fast if the installed rca predates the stdio transport — the
    // published release can lag `rca serve --stdio` landing on rca main.
    emit_install_progress(app, "Verifying serve --stdio support…", false);
    let probe =
        format!("{remote_rca} serve -h 2>&1 | grep -qi stdio && echo STDIO_OK || echo STDIO_MISSING");
    let cap = ssh(&probe).unwrap_or_default();
    if !cap.contains("STDIO_OK") {
        return Err(format!(
            "the rca on the remote ({remote_rca}) has no `serve --stdio` support — its published \
             release predates the stdio-over-ssh transport. Wait for a newer remote-adapter \
             release (or install a build that includes stdio) and re-run."
        ));
    }
    Ok(remote_rca)
}

/// Ensure THIS machine has an rca that speaks the stdio transport.
///
/// `wrap_launch` runs rca locally (the agent process stays on this host), so a
/// workspace whose remote half is installed but whose local half is missing
/// fails at first spawn with "rca binary not found" — after a wizard that said
/// it succeeded. An already-resolvable rca is left alone: the user may have
/// pinned a build via `rcaPath`, and re-downloading over it would silently
/// undo that choice.
fn ensure_local_rca(app: &AppHandle) -> Result<(), String> {
    if let Some(existing) = remote_workspace::find_local_rca() {
        emit_install_progress(app, &format!("Local rca already present ({existing})."), false);
        return Ok(());
    }
    emit_install_progress(app, "Installing rca on this machine…", false);
    let local = remote_workspace::install_local_rca()
        .map_err(|e| format!("local rca install failed: {e}"))?;
    emit_install_progress(app, &format!("Local rca installed at {local}."), false);
    Ok(())
}

/// Install rca on `conn` and record it in the host book — no workspace.
///
/// Split out from [`install_rca_remote_impl`] because "make this host an rca
/// executor" and "register a workspace path on it" are two decisions, and
/// fusing them forced the user to name a path before they could set up a host
/// at all — the path being the hardest field on the form, since it has to exist
/// identically on both machines. Choosing a workspace is the composer's job now.
fn install_rca_on_host_impl(app: &AppHandle, conn: SshHost) -> Result<Vec<SshHost>, String> {
    emit_install_progress(app, "Connecting via SSH…", false);
    ssh_exec(&conn, "echo ok").map_err(|e| format!("SSH connection failed: {e}"))?;
    let remote_rca = provision_rca(app, |cmd| ssh_exec(&conn, cmd))?;
    ensure_local_rca(app)?;
    let host_id = remote_host::adopt_host(&conn)?;
    remote_host::set_host_rca_path(&host_id, &remote_rca)?;
    emit_install_progress(app, "Host is ready to run workspaces.", true);
    Ok(remote_host::load_hosts())
}

/// Tauri command — provision `conn` as an rca executor. Runs on the blocking
/// pool so the multi-second ssh work never freezes the UI thread.
#[tauri::command]
pub async fn install_rca_on_host(conn: SshHost, app: AppHandle) -> Result<Vec<SshHost>, String> {
    tauri::async_runtime::spawn_blocking(move || install_rca_on_host_impl(&app, conn))
        .await
        .map_err(|e| format!("install task join failed: {e}"))?
}

fn install_rca_remote_impl(
    app: &AppHandle,
    conn: SshHost,
    path: String,
    label: Option<String>,
) -> Result<remote_workspace::RemoteWorkspacesConfig, String> {
    // 1. Probe SSH connectivity.
    emit_install_progress(app, "Connecting via SSH…", false);
    ssh_exec(&conn, "echo ok").map_err(|e| format!("SSH connection failed: {e}"))?;

    // 1b. Verify the workspace path is creatable + writable ON THE REMOTE
    //     (D3). The same-absolute-path constraint means it must exist on both
    //     machines; `upsert` fails loudly for the local side, this covers the
    //     remote side up front instead of at first spawn. The path is
    //     single-quoted; a path containing a single quote is refused (paths
    //     never legitimately need one and it would break the quoting).
    emit_install_progress(app, "Checking workspace path on remote…", false);
    if path.contains('\'') {
        return Err("workspace path must not contain a single quote".to_string());
    }
    ssh_exec(&conn, &format!("mkdir -p '{path}' && test -w '{path}'")).map_err(|e| {
        format!(
            "remote workspace path '{path}' is not creatable/writable on the remote host: {e} — \
             pick an absolute path that exists (or can be made) identically on both machines"
        )
    })?;

    // 2-3. Detect platform, install rca, verify stdio support.
    let remote_rca = provision_rca(app, |cmd| ssh_exec(&conn, cmd))?;

    // 3b. The local half. Both halves must be good before anything is
    //     registered, so a failure here leaves no entry that would blow up at
    //     the user's first spawn.
    ensure_local_rca(app)?;

    // 4. Put the host in the book (or find the record that already IS this
    //    machine) and mark it as an rca executor. The workspace then references
    //    it by id, so editing the host's address later carries its workspaces
    //    along instead of silently orphaning them.
    let host_id = remote_host::adopt_host(&conn)?;
    remote_host::set_host_rca_path(&host_id, &remote_rca)?;

    // 5. Register the stdio-over-ssh workspace (validates transport + creates
    //    the local mirror directory at the identity-mapped path).
    emit_install_progress(app, "Registering workspace…", false);
    let cfg = remote_workspace::upsert(remote_workspace::RemoteWorkspace {
        path,
        host_id: Some(host_id),
        label: label.filter(|l| !l.trim().is_empty()),
        ..Default::default()
    })?;
    emit_install_progress(app, "Installed rca & registered workspace.", true);
    Ok(cfg)
}

/// Tauri command — install rca on a saved SSH host and register `path` as a
/// stdio-over-ssh remote workspace. Runs on the blocking pool so the
/// multi-second SSH work never freezes the UI thread, and streams
/// `rca-install-progress` events while it runs. The final config is returned
/// so the caller refreshes the list; errors reject the promise.
#[tauri::command]
pub async fn install_rca_remote(
    conn: SshHost,
    path: String,
    label: Option<String>,
    app: AppHandle,
) -> Result<remote_workspace::RemoteWorkspacesConfig, String> {
    tauri::async_runtime::spawn_blocking(move || install_rca_remote_impl(&app, conn, path, label))
        .await
        .map_err(|e| format!("install task join failed: {e}"))?
}

fn update_rca_remote_impl(
    app: &AppHandle,
    path: String,
) -> Result<remote_workspace::RemoteWorkspacesConfig, String> {
    let entry = remote_workspace::find_for_path(&path)
        .ok_or("no remote workspace is registered at this path")?;
    // Resolves either form — a `hostId` through the host book, or the
    // `sshTarget` baked into an entry written before the book existed.
    let ssh_target = entry
        .resolved_ssh_target()?
        .ok_or("this remote workspace is not a stdio-over-ssh entry — nothing to update")?;

    emit_install_progress(app, "Connecting via SSH…", false);
    ssh_exec_target(&ssh_target, "echo ok").map_err(|e| format!("SSH connection failed: {e}"))?;

    let remote_rca = provision_rca(app, |cmd| ssh_exec_target(&ssh_target, cmd))?;
    ensure_local_rca(app)?;

    emit_install_progress(app, "Updating registry…", false);
    // A book-backed entry keeps its `hostId` and the new rca path lands on the
    // host record, so every workspace on that host sees the update at once. A
    // legacy `sshTarget` entry is rewritten in place, unchanged in form.
    let mut updated = remote_workspace::RemoteWorkspace {
        path: entry.path.clone(),
        label: entry.label.clone(),
        ..Default::default()
    };
    match entry.host_id.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(id) => {
            remote_host::set_host_rca_path(id, &remote_rca)?;
            updated.host_id = Some(id.to_string());
        }
        None => {
            updated.ssh_target = Some(ssh_target);
            updated.remote_rca_path = Some(remote_rca);
        }
    }
    let cfg = remote_workspace::upsert(updated)?;
    emit_install_progress(app, "rca updated.", true);
    Ok(cfg)
}

/// Tauri command — re-run the rca install on an already-registered stdio
/// workspace (pull the latest release, re-verify `serve --stdio`, update the
/// entry's remote rca path). Streams the same `rca-install-progress` events.
#[tauri::command]
pub async fn update_rca_remote(
    path: String,
    app: AppHandle,
) -> Result<remote_workspace::RemoteWorkspacesConfig, String> {
    tauri::async_runtime::spawn_blocking(move || update_rca_remote_impl(&app, path))
        .await
        .map_err(|e| format!("update task join failed: {e}"))?
}

// ── Harness environment on remote workspace hosts (wizard phase 2) ───────────

/// Resolve a registered remote workspace's ssh target by workspace path.
/// Pairing-code (libp2p) entries carry no ssh route, so harness actions on
/// them are a structured refusal rather than a hang.
pub(crate) fn ssh_target_for_workspace(path: &str) -> Result<String, String> {
    let entry = remote_workspace::find_for_path(path)
        .ok_or("no remote workspace is registered at this path")?;
    entry
        .ssh_target
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            "this remote workspace uses the pairing-code transport — harness actions need an ssh entry".to_string()
        })
}

/// Probe claude/codex/dsh on a remote workspace's host (one ssh round trip).
#[tauri::command]
pub async fn remote_workspace_harness_statuses(
    path: String,
) -> Result<Vec<claw_fleet_core::harness_status::HarnessStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let ssh_target = ssh_target_for_workspace(&path)?;
        remote_host::remote_harness_statuses(&ssh_target)
    })
    .await
    .map_err(|e| format!("probe task join failed: {e}"))?
}

/// Install a harness on a remote workspace's host via its official installer
/// over ssh, streaming output on `harness-install-progress` with source
/// `remote:<path>:<source>`. Returns the host's fresh statuses (post-install
/// probe is the success criterion, same as locally).
#[tauri::command]
pub async fn install_harness_remote(
    path: String,
    source: String,
    app: AppHandle,
) -> Result<Vec<claw_fleet_core::harness_status::HarnessStatus>, claw_fleet_core::harness_install::InstallError>
{
    use claw_fleet_core::harness_install::{InstallError, InstallErrorCode, REMOTE_NODE_MISSING_EXIT};
    tauri::async_runtime::spawn_blocking(move || {
        let structured = |code: InstallErrorCode, message: String| InstallError { code, message };
        let ssh_target = ssh_target_for_workspace(&path)
            .map_err(|e| structured(InstallErrorCode::SpawnFailed, e))?;
        let plan = remote_host::ssh_harness_install_plan(&ssh_target, &source)
            .map_err(|e| structured(InstallErrorCode::SpawnFailed, e))?;

        let progress_key = format!("remote:{path}:{source}");
        let emitter = app.clone();
        let progress = move |line: &str| {
            let _ = emitter.emit(
                "harness-install-progress",
                crate::gui::HarnessInstallProgress::new(progress_key.clone(), line.to_string()),
            );
        };
        progress(&format!("$ ssh {ssh_target} <official installer>"));
        claw_fleet_core::harness_install::run_streaming(
            &plan,
            claw_fleet_core::harness_install::INSTALL_TIMEOUT,
            &progress,
        )
        .map_err(|e| {
            // The dsh remote script exits 42 for "no npm" — surface it as the
            // same structured NodeMissing the local path uses.
            if e.message.contains(&format!("exit status: {REMOTE_NODE_MISSING_EXIT}"))
                || e.message.contains("npm not found on the remote host")
            {
                structured(InstallErrorCode::NodeMissing, e.message)
            } else {
                e
            }
        })?;

        progress("verifying installation on the remote host…");
        let statuses = remote_host::remote_harness_statuses(&ssh_target)
            .map_err(|e| structured(InstallErrorCode::VerifyFailed, e))?;
        let installed = statuses.iter().any(|s| s.source == source && s.installed);
        if !installed {
            return Err(structured(
                InstallErrorCode::VerifyFailed,
                format!("installer finished but no runnable {source} was found on the remote host"),
            ));
        }
        progress("remote install verified.");
        Ok(statuses)
    })
    .await
    .map_err(|e| InstallError {
        code: InstallErrorCode::SpawnFailed,
        message: format!("install task join failed: {e}"),
    })?
}

// ── ~/.ssh/config profile listing ────────────────────────────────────────────

/// List SSH config profile (Host) names from ~/.ssh/config, following Include directives.
#[tauri::command]
pub fn list_ssh_profiles() -> Vec<String> {
    let Some(home) = crate::session::real_home_dir() else {
        return vec![];
    };
    let config_path = home.join(".ssh").join("config");
    let mut profiles = vec![];
    let mut visited = std::collections::HashSet::new();
    collect_ssh_hosts(&config_path, &home, &mut profiles, &mut visited);
    profiles
}

/// Recursively collect Host names from an SSH config file, resolving Include directives.
fn collect_ssh_hosts(
    path: &std::path::Path,
    home: &std::path::Path,
    profiles: &mut Vec<String>,
    visited: &mut std::collections::HashSet<PathBuf>,
) {
    let canonical = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return,
    };
    if !visited.insert(canonical) {
        return; // avoid cycles
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let ssh_dir = home.join(".ssh");

    for line in content.lines() {
        let bare = line.splitn(2, '#').next().unwrap_or("").trim();
        if bare.is_empty() {
            continue;
        }
        let lower = bare.to_ascii_lowercase();

        if lower.starts_with("host ") {
            let offset = "host ".len();
            for host in bare[offset..].split_whitespace() {
                if !host.contains('*') && !host.contains('?') {
                    profiles.push(host.to_string());
                }
            }
        } else if lower.starts_with("include ") {
            let offset = "include ".len();
            let pattern = bare[offset..].trim();
            // Resolve ~ and relative paths per OpenSSH rules:
            //   ~/ → user home;  relative (no /) → relative to ~/.ssh/
            let resolved = if let Some(rest) = pattern.strip_prefix("~/") {
                home.join(rest).to_string_lossy().into_owned()
            } else if !pattern.starts_with('/') {
                ssh_dir.join(pattern).to_string_lossy().into_owned()
            } else {
                pattern.to_string()
            };

            // Expand globs (e.g. "config.d/*").  We handle the common case
            // where the glob is in the filename component only.
            let resolved_path = std::path::Path::new(&resolved);
            if let Some(fname) = resolved_path.file_name().and_then(|f| f.to_str()) {
                if fname.contains('*') || fname.contains('?') {
                    // Read directory and match entries against the pattern
                    if let Some(parent) = resolved_path.parent() {
                        if let Ok(entries) = std::fs::read_dir(parent) {
                            for entry in entries.flatten() {
                                let name = entry.file_name();
                                let name_str = name.to_string_lossy();
                                if glob_match(fname, &name_str) {
                                    collect_ssh_hosts(&entry.path(), home, profiles, visited);
                                }
                            }
                        }
                    }
                } else {
                    // No glob characters — literal path
                    collect_ssh_hosts(resolved_path, home, profiles, visited);
                }
            }
        }
    }
}

/// Simple glob matching supporting `*` (any chars) and `?` (single char).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t)
}

fn glob_match_inner(pattern: &[char], text: &[char]) -> bool {
    let (mut pi, mut ti) = (0, 0);
    let (mut star_pi, mut star_ti) = (usize::MAX, 0);
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_star_and_question_mark() {
        assert!(glob_match("config.d/*", "config.d/x"));
        assert!(glob_match("*.conf", "work.conf"));
        assert!(glob_match("host?", "host1"));
        assert!(!glob_match("host?", "host12"));
        assert!(!glob_match("*.conf", "work.cfg"));
    }

    #[test]
    fn collect_ssh_hosts_follows_includes_and_skips_wildcards() {
        let home = tempfile::TempDir::new().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "Host alpha beta*\n  HostName a\nInclude config.d/*\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config.d").join("work"), "# comment\nHost gamma\n").unwrap();

        let mut profiles = vec![];
        let mut visited = std::collections::HashSet::new();
        collect_ssh_hosts(&ssh.join("config"), home.path(), &mut profiles, &mut visited);
        assert_eq!(profiles, vec!["alpha", "gamma"]);
    }
}
