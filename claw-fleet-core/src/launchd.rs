//! macOS LaunchAgent installer for `fleet serve`.
//!
//! Writes `~/Library/LaunchAgents/com.claudefleet.serve.plist` and loads it
//! via `launchctl bootstrap gui/<uid>` so `fleet serve` starts at login and
//! stays running independent of the Tauri GUI.
//!
//! Boss decided this auto-install path during P3 architecture alignment.
//! The plist runs `<fleet_path> serve --port <port> --token <token>
//! --port-file ~/.claude/fleet/port`.
//!
//! All functions return `Err` on non-macOS platforms so callers can present a
//! consistent UX.

use std::path::PathBuf;
use std::process::Command;

use crate::session::real_home_dir;

pub const PLIST_LABEL: &str = "com.claudefleet.serve";

/// Absolute path of the LaunchAgent plist.
/// Returns `None` if the home directory can't be resolved.
pub fn plist_path() -> Option<PathBuf> {
    real_home_dir().map(|h| {
        h.join("Library")
            .join("LaunchAgents")
            .join(format!("{PLIST_LABEL}.plist"))
    })
}

/// `~/.claude/fleet/port` — fleet serve writes its bound port here on startup.
/// fleet-cli (P6) reads it to discover the supervisor's HTTP endpoint.
pub fn port_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude").join("fleet").join("port"))
}

/// `~/.claude/fleet/token` — auth token shared between fleet serve and any
/// local clients (Tauri app, fleet-cli). Generated on first install.
pub fn token_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude").join("fleet").join("token"))
}

/// Whether the LaunchAgent plist is currently installed on disk.
pub fn is_installed() -> bool {
    plist_path().map(|p| p.exists()).unwrap_or(false)
}

/// Generate the plist XML for the given fleet binary path + port.
///
/// `port = 0` lets the OS assign a free ephemeral port (fleet-cli reads the
/// actual port from `port_file_path()`).
pub fn generate_plist(fleet_path: &str, port: u16, token: &str) -> String {
    let port_file = port_file_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/tmp/fleet-port".into());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{fleet}</string>
    <string>serve</string>
    <string>--port</string>
    <string>{port}</string>
    <string>--token</string>
    <string>{token}</string>
    <string>--port-file</string>
    <string>{port_file}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/tmp/{label}.out.log</string>
  <key>StandardErrorPath</key>
  <string>/tmp/{label}.err.log</string>
</dict>
</plist>
"#,
        label = PLIST_LABEL,
        fleet = xml_escape(fleet_path),
        port = port,
        token = xml_escape(token),
        port_file = xml_escape(&port_file),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Install the LaunchAgent: write the plist + `launchctl bootstrap gui/<uid>`.
/// Idempotent — calling twice replaces the prior plist and reloads it.
#[cfg(target_os = "macos")]
pub fn install(fleet_path: &str, port: u16, token: &str) -> Result<(), String> {
    let path = plist_path().ok_or("cannot resolve home dir")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir LaunchAgents: {e}"))?;
    }
    let xml = generate_plist(fleet_path, port, token);
    std::fs::write(&path, xml).map_err(|e| format!("write plist: {e}"))?;

    // bootout first (ignore error — may not be loaded yet)
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}"), path.to_string_lossy().as_ref()])
        .output();
    let out = Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{uid}"),
            path.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|e| format!("launchctl bootstrap: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("launchctl bootstrap failed: {stderr}"));
    }
    Ok(())
}

/// Uninstall the LaunchAgent: `launchctl bootout` + remove the plist.
#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<(), String> {
    let path = plist_path().ok_or("cannot resolve home dir")?;
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}"), path.to_string_lossy().as_ref()])
        .output();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("remove plist: {e}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install(_fleet_path: &str, _port: u16, _token: &str) -> Result<(), String> {
    Err("launchd auto-install is macOS-only in v1".into())
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall() -> Result<(), String> {
    Err("launchd auto-install is macOS-only in v1".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contains_label_and_args() {
        let xml = generate_plist("/usr/local/bin/fleet", 7007, "secret");
        assert!(xml.contains("<string>com.claudefleet.serve</string>"));
        assert!(xml.contains("<string>/usr/local/bin/fleet</string>"));
        assert!(xml.contains("<string>7007</string>"));
        assert!(xml.contains("<string>secret</string>"));
        assert!(xml.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn plist_escapes_xml_special_chars() {
        let xml = generate_plist("/path<&>x", 0, "tok&en");
        assert!(xml.contains("/path&lt;&amp;&gt;x"));
        assert!(xml.contains("tok&amp;en"));
    }
}
