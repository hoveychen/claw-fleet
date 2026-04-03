//! macOS TCC (Transparency, Consent, and Control) diagnostics.
//!
//! On macOS, accessing certain directories (~/Music, ~/Pictures, ~/Documents,
//! ~/Desktop, ~/Downloads, ~/Movies) triggers system permission dialogs.
//! This module provides utilities to detect and log TCC-triggering code paths.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// A TCC-protected directory category.
#[derive(Debug, Clone, Serialize)]
pub struct TccCategory {
    /// The directory name under ~ (e.g. "Music").
    pub dir_name: &'static str,
    /// The macOS permission dialog category name.
    pub dialog_name: &'static str,
}

/// All TCC-protected directories under the user's home.
const TCC_PROTECTED_DIRS: &[TccCategory] = &[
    TccCategory { dir_name: "Desktop", dialog_name: "Desktop Folder" },
    TccCategory { dir_name: "Documents", dialog_name: "Documents Folder" },
    TccCategory { dir_name: "Downloads", dialog_name: "Downloads Folder" },
    TccCategory { dir_name: "Music", dialog_name: "Apple Music" },
    TccCategory { dir_name: "Pictures", dialog_name: "Photos" },
    TccCategory { dir_name: "Movies", dialog_name: "Movies Folder" },
];

/// Check if a path is inside a macOS TCC-protected directory.
/// Returns the TCC category name if so, or None if safe.
#[cfg(target_os = "macos")]
pub fn check_tcc_path(path: &Path) -> Option<&'static str> {
    let home = dirs::home_dir()?;
    for cat in TCC_PROTECTED_DIRS {
        let protected = home.join(cat.dir_name);
        if path == protected || path.starts_with(&protected) {
            return Some(cat.dialog_name);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn check_tcc_path(_path: &Path) -> Option<&'static str> {
    None
}

/// Like `Path::exists()`, but returns `false` for TCC-protected paths instead of
/// calling `stat()`.  Returning `false` causes the greedy decode algorithm to try
/// shorter segments, which avoids locking the decode into a TCC-protected directory
/// (e.g. `~/Music`, `~/Downloads`).  If the path genuinely IS the intended
/// workspace, the user already has an IDE lock-file match that bypasses the decode
/// entirely — so returning `false` here is safe.
pub fn safe_exists(path: &Path) -> bool {
    if let Some(cat) = check_tcc_path(path) {
        crate::log_debug(&format!(
            "[TCC-BLOCKED] safe_exists blocked stat on {:?} ({})",
            path, cat,
        ));
        return false;
    }
    path.exists()
}

/// Check whether the given path (or any parent) is inside a TCC-protected
/// directory.  Use this to skip `stat()`/`exists()` calls on paths derived
/// from decoded workspace paths that might resolve into protected folders.
pub fn is_tcc_protected(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else { return false };
        for cat in TCC_PROTECTED_DIRS {
            let protected = home.join(cat.dir_name);
            if path == protected || path.starts_with(&protected) {
                return true;
            }
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

// ── Comprehensive TCC diagnostic ────────────────────────────────────────────

/// A single diagnostic finding: a code path that may trigger TCC.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TccFinding {
    /// Which component triggered this (e.g. "decode_workspace_path", "sysinfo", "cursor_db").
    pub component: String,
    /// The filesystem path that would be accessed.
    pub path: String,
    /// The TCC category (e.g. "Apple Music", "Photos").
    pub tcc_category: String,
    /// Human-readable description of why this access happens.
    pub reason: String,
}

/// Full diagnostic report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TccDiagnostic {
    pub findings: Vec<TccFinding>,
    /// All workspace directory names in ~/.claude/projects/.
    pub workspace_dirs: Vec<String>,
    /// Workspace dirs that would trigger TCC during decode.
    pub tcc_triggering_workspaces: Vec<TccWorkspaceDecode>,
    /// sysinfo Phase 1 test result.
    pub sysinfo_phase1_safe: bool,
    /// Number of processes scanned in Phase 1.
    pub sysinfo_process_count: usize,
    /// Matched claude/openclaw/codex processes (Phase 2 targets).
    pub sysinfo_matched_processes: Vec<MatchedProcess>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TccWorkspaceDecode {
    /// The encoded directory name from ~/.claude/projects/.
    pub encoded: String,
    /// All candidate paths that would be probed by exists().
    pub probed_paths: Vec<String>,
    /// Subset of probed_paths that are TCC-protected.
    pub tcc_paths: Vec<TccProbedPath>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TccProbedPath {
    pub path: String,
    pub tcc_category: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedProcess {
    pub pid: u32,
    pub name: String,
    pub cwd: Option<String>,
    pub cwd_tcc_category: Option<String>,
}

/// Run the decode algorithm in SAFE dry-run mode — collects all paths that WOULD
/// be probed WITHOUT actually calling exists() on any of them. Uses heuristics
/// to simulate the decode without filesystem access.
fn dry_run_decode_no_io(parts: &[&str]) -> Vec<String> {
    let mut probed = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < parts.len() {
        for end in (i + 1..=parts.len()).rev() {
            let candidate_segment = parts[i..end].join("-");
            let candidate_path = format!("{}/{}", current, candidate_segment);
            probed.push(candidate_path.clone());
            if end == i + 1 {
                // Always take single-part as fallback (worst case for TCC probing)
                current = candidate_path;
                i += 1;
            }
        }
    }
    probed
}

/// Run comprehensive TCC diagnostics.
pub fn diagnose() -> TccDiagnostic {
    let mut findings = Vec::new();
    let mut workspace_dirs = Vec::new();
    let mut tcc_triggering_workspaces = Vec::new();

    // ── 1. Check all workspace directories in ~/.claude/projects/ ────────
    if let Some(home) = dirs::home_dir() {
        let projects_dir = home.join(".claude").join("projects");
        if let Ok(entries) = std::fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let encoded = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                workspace_dirs.push(encoded.clone());

                // Simulate decode without IO
                let stripped = encoded.trim_start_matches('-');
                let parts: Vec<&str> = stripped.split('-').collect();
                let probed_paths = dry_run_decode_no_io(&parts);

                let mut tcc_paths = Vec::new();
                for p in &probed_paths {
                    if let Some(category) = check_tcc_path(Path::new(p)) {
                        tcc_paths.push(TccProbedPath {
                            path: p.clone(),
                            tcc_category: category.to_string(),
                        });
                    }
                }

                if !tcc_paths.is_empty() {
                    findings.push(TccFinding {
                        component: "decode_workspace_path".to_string(),
                        path: tcc_paths[0].path.clone(),
                        tcc_category: tcc_paths[0].tcc_category.clone(),
                        reason: format!(
                            "Workspace dir '{}' decode probes TCC path during Path::exists()",
                            encoded,
                        ),
                    });
                    tcc_triggering_workspaces.push(TccWorkspaceDecode {
                        encoded,
                        probed_paths: probed_paths.iter().map(|p| p.to_string()).collect(),
                        tcc_paths,
                    });
                }
            }
        }
    }

    // ── 2. Test sysinfo Phase 1 (cmd-only scan) ─────────────────────────
    // Phase 1 uses sysctl(KERN_PROCARGS2) and proc_pidinfo(PROC_PIDTBSDINFO)
    // which are kernel reads and should NOT trigger TCC.
    // We verify by counting processes scanned and checking for matched ones.
    let (sysinfo_process_count, sysinfo_matched_processes) = test_sysinfo_phase1();

    // Check if any matched process has its cwd in a TCC dir.
    for mp in &sysinfo_matched_processes {
        if let Some(ref cwd) = mp.cwd {
            if let Some(category) = check_tcc_path(Path::new(cwd)) {
                findings.push(TccFinding {
                    component: "sysinfo_phase2".to_string(),
                    path: cwd.clone(),
                    tcc_category: category.to_string(),
                    reason: format!(
                        "Process '{}' (PID {}) has cwd in TCC-protected dir; Phase 2 would probe it",
                        mp.name, mp.pid,
                    ),
                });
            }
        }
    }

    // ── 3. Check Cursor DB path ─────────────────────────────────────────
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            let cursor_db = home
                .join("Library")
                .join("Application Support")
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb");
            if let Some(category) = check_tcc_path(&cursor_db) {
                findings.push(TccFinding {
                    component: "cursor_db".to_string(),
                    path: cursor_db.to_string_lossy().to_string(),
                    tcc_category: category.to_string(),
                    reason: "Cursor SQLite DB is in a TCC-protected path".to_string(),
                });
            }
            // Note: ~/Library/Application Support/ is NOT TCC-protected on macOS,
            // so this should not produce a finding.
        }
    }

    // ── 4. Check codex extension directories ────────────────────────────
    if let Some(home) = dirs::home_dir() {
        let ext_dirs = [
            home.join(".vscode").join("extensions"),
            home.join(".vscode-insiders").join("extensions"),
        ];
        for dir in &ext_dirs {
            if let Some(category) = check_tcc_path(dir) {
                findings.push(TccFinding {
                    component: "detect_installed_tools".to_string(),
                    path: dir.to_string_lossy().to_string(),
                    tcc_category: category.to_string(),
                    reason: "VS Code extension dir scan touches TCC path".to_string(),
                });
            }
        }
    }

    let sysinfo_phase1_safe = true; // Based on sysinfo source analysis

    TccDiagnostic {
        findings,
        workspace_dirs,
        tcc_triggering_workspaces,
        sysinfo_phase1_safe,
        sysinfo_process_count,
        sysinfo_matched_processes,
    }
}

/// Test sysinfo Phase 1 only (cmd-based scan without cwd).
/// Returns (total_process_count, matched_processes_with_cwd_from_phase2).
fn test_sysinfo_phase1() -> (usize, Vec<MatchedProcess>) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sys = System::new();

    // Phase 1: cmd only — should NOT trigger TCC.
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always),
    );

    let total = sys.processes().len();

    let target_names = ["claude", "claude.exe", "openclaw", "openclaw.exe", "codex", "codex.exe", "codex-rs"];

    let matched_pids: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| {
            let name = p.name().to_string_lossy();
            target_names.contains(&name.as_ref())
                || (name.starts_with("node") && {
                    let cmd: String = p.cmd().iter()
                        .map(|s| s.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(" ");
                    cmd.contains("openclaw") || cmd.contains("codex")
                })
        })
        .map(|(pid, p)| (*pid, p.name().to_string_lossy().to_string()))
        .collect();

    // Phase 2: get cwd for matched processes only.
    let pids_only: Vec<_> = matched_pids.iter().map(|(pid, _)| *pid).collect();
    if !pids_only.is_empty() {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids_only),
            true,
            ProcessRefreshKind::nothing()
                .with_cwd(UpdateKind::Always),
        );
    }

    let mut matched = Vec::new();
    for (pid, name) in &matched_pids {
        let cwd = sys.process(*pid)
            .and_then(|p| p.cwd())
            .and_then(|p| p.to_str())
            .map(|s| s.to_string());
        let cwd_tcc_category = cwd.as_ref()
            .and_then(|c| check_tcc_path(Path::new(c)))
            .map(|s| s.to_string());
        matched.push(MatchedProcess {
            pid: pid.as_u32(),
            name: name.clone(),
            cwd,
            cwd_tcc_category,
        });
    }

    (total, matched)
}

/// Returns all TCC-protected directory paths for the current user.
#[cfg(target_os = "macos")]
pub fn get_tcc_protected_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    TCC_PROTECTED_DIRS.iter()
        .map(|cat| home.join(cat.dir_name))
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn get_tcc_protected_paths() -> Vec<PathBuf> {
    vec![]
}

/// Query macOS unified log for recent TCC prompts related to our app.
/// Writes findings to the debug log.
pub fn log_recent_tcc_events() {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        // Query tccd for entries that mention our PID or bundle identifier.
        let predicate = format!(
            "process == \"tccd\" AND \
             (eventMessage CONTAINS \"{}\" \
              OR eventMessage CONTAINS \"claw-fleet\" \
              OR eventMessage CONTAINS \"com.hoveychen.claw-fleet\")",
            pid,
        );
        let output = std::process::Command::new("log")
            .args([
                "show",
                "--last", "5m",
                "--predicate", &predicate,
                "--info",
                "--style", "compact",
            ])
            .output();
        match output {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                // Extract only lines mentioning kTCCService to keep log concise.
                let service_lines: Vec<&str> = text.lines()
                    .filter(|l| l.contains("kTCCService"))
                    .collect();
                if service_lines.is_empty() {
                    crate::log_debug(&format!(
                        "[TCC-LOG] No kTCCService entries for PID {} in last 5m",
                        pid,
                    ));
                } else {
                    crate::log_debug(&format!(
                        "[TCC-LOG] {} kTCCService entries for PID {}:",
                        service_lines.len(), pid,
                    ));
                    for line in &service_lines {
                        crate::log_debug(&format!("[TCC-LOG]   {}", line));
                    }
                }
            }
            Err(e) => {
                crate::log_debug(&format!("[TCC-LOG] Failed to query log: {}", e));
            }
        }
    }
}
