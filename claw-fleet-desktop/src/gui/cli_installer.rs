use super::*;
use serde::Serialize;

// ── CLI installer (macOS only) ───────────────────────────────────────────────

/// Create a symlink at /usr/local/bin/fleet pointing to the bundled fleet binary.
/// Requires the user to approve via osascript (admin password prompt).
#[tauri::command]
pub(crate) fn install_fleet_cli(app: tauri::AppHandle) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        // Tauri places externalBin sidecars next to the main executable
        let exe_dir = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("no parent dir")?
            .to_path_buf();
        let fleet_bin = exe_dir.join("fleet");
        if !fleet_bin.exists() {
            // `not_found` code + path detail (see osascript_failure_code docs).
            return Err(format!("not_found\n{}", fleet_bin.display()));
        }

        let target = "/usr/local/bin/fleet";
        let src = fleet_bin.to_string_lossy().to_string();

        // Use osascript to run with admin privileges
        let script = format!(
            r#"do shell script "mkdir -p /usr/local/bin && ln -sf '{}' '{}'" with administrator privileges"#,
            src, target
        );
        let output = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(target.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(osascript_failure_code(&stderr))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("not_macos".to_string())
    }
}

/// Classify an osascript admin-privilege failure into a stable error payload.
///
/// The frontend localizes `install_fleet_cli` errors, so this returns a
/// machine-readable code (NOT prose) on the first line, optionally followed by
/// a `\n` and a human detail string. Recognised codes (kept in sync with
/// `AccountInfo.tsx`'s `installCLI` catch handler):
///   - `cancelled` — user dismissed the macOS password dialog
///     (osascript exits non-zero with `User canceled. (-128)`); just retry.
///   - `failed`    — a genuine failure; when osascript printed anything, its
///     raw stderr follows on the next line so it can actually be diagnosed.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn osascript_failure_code(stderr: &str) -> String {
    let s = stderr.trim();
    let lower = s.to_lowercase();
    if s.contains("-128") || lower.contains("user canceled") || lower.contains("user cancelled") {
        "cancelled".to_string()
    } else if s.is_empty() {
        "failed".to_string()
    } else {
        format!("failed\n{s}")
    }
}

#[cfg(test)]
mod install_cli_tests {
    use super::osascript_failure_code;

    #[test]
    fn user_cancel_minus_128_maps_to_cancelled_code() {
        let payload = osascript_failure_code("0:50: execution error: User canceled. (-128)");
        assert_eq!(
            payload, "cancelled",
            "a cancelled password dialog must map to the bare `cancelled` code, got: {payload}"
        );
    }

    #[test]
    fn real_failure_carries_failed_code_and_raw_stderr_detail() {
        let payload = osascript_failure_code("ln: /usr/local/bin/fleet: Permission denied");
        let mut lines = payload.splitn(2, '\n');
        assert_eq!(lines.next(), Some("failed"), "genuine failures use the `failed` code");
        assert_eq!(
            lines.next(),
            Some("ln: /usr/local/bin/fleet: Permission denied"),
            "the real osascript stderr must ride along as the detail line"
        );
    }

    #[test]
    fn empty_stderr_maps_to_bare_failed_code() {
        let payload = osascript_failure_code("   ");
        assert_eq!(
            payload, "failed",
            "empty stderr should map to the bare `failed` code with no detail, got: {payload}"
        );
    }
}


#[derive(Serialize, Clone)]
pub(crate) struct DetectedTool {
    name: String,
    skill_path: String,
}

#[derive(Serialize)]
pub(crate) struct SkillInstallResult {
    installed: Vec<DetectedTool>,
    errors: Vec<String>,
}

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .map_err(|_| "Cannot determine home directory".to_string())
}

/// Detect which AI tools are installed (by checking their home directories).
#[tauri::command]
pub(crate) fn detect_ai_tools() -> Result<Vec<DetectedTool>, String> {
    let home = home_dir()?;
    let detected = SKILL_TARGETS
        .iter()
        .filter(|(_, dir)| home.join(dir).exists())
        .map(|(name, dir)| DetectedTool {
            name: name.to_string(),
            skill_path: home
                .join(dir)
                .join("skills")
                .join("fleet")
                .join("SKILL.md")
                .to_string_lossy()
                .to_string(),
        })
        .collect();
    Ok(detected)
}

/// Open a native file-open dialog and return the chosen path.
#[tauri::command]
pub(crate) async fn pick_file(title: String) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title(&title)
        .pick_file()
        .await
        .map(|f| f.path().to_string_lossy().to_string())
}

/// Open a native save dialog and write SKILL.md to the chosen path.
#[tauri::command]
pub(crate) async fn save_skill_file() -> Result<String, String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_file_name("SKILL.md")
        .set_title("Save Fleet Skill")
        .save_file()
        .await;

    match handle {
        Some(file) => {
            file.write(FLEET_SKILL_MD.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            Ok(file.path().to_string_lossy().to_string())
        }
        None => Err("cancelled".to_string()),
    }
}

/// Install the fleet skill to all detected AI tool directories.
#[tauri::command]
pub(crate) fn install_fleet_skill() -> Result<SkillInstallResult, String> {
    let home = home_dir()?;
    let mut installed = vec![];
    let mut errors = vec![];

    for (name, dir) in SKILL_TARGETS {
        let tool_home = home.join(dir);
        if !tool_home.exists() {
            continue;
        }
        let skill_dir = tool_home.join("skills").join("fleet");
        let skill_path = skill_dir.join("SKILL.md");
        match std::fs::create_dir_all(&skill_dir)
            .and_then(|_| std::fs::write(&skill_path, FLEET_SKILL_MD))
        {
            Ok(_) => installed.push(DetectedTool {
                name: name.to_string(),
                skill_path: skill_path.to_string_lossy().to_string(),
            }),
            Err(e) => errors.push(format!("{}: {}", name, e)),
        }
    }

    if installed.is_empty() && errors.is_empty() {
        errors.push(
            "No supported AI tools detected. Install Claude Code, Codex, GitHub Copilot, or Gemini CLI first.".to_string(),
        );
    }

    Ok(SkillInstallResult { installed, errors })
}

