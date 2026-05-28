//! Thin wrapper around the `claude plugin ...` subcommand.
//!
//! Centralizes invocation of the Claude Code CLI for plugin / marketplace
//! queries and mutations. Read paths use `--json` so callers get structured
//! data; mutation paths return stdout/stderr as-is so the UI can surface the
//! CLI's own error messages.
//!
//! The `parse_*` free functions are exposed for unit testing — they take a
//! raw stdout string and return parsed structures, decoupled from process
//! invocation.
//!
//! Note on terminology: the CLI's `installed[]` array means "enabled in the
//! current scope", *not* "physically downloaded under
//! ~/.claude/plugins/marketplaces/<mk>/...". A plugin can be downloaded
//! (because the marketplace shipped it) without being enabled. To produce a
//! "downloaded but not enabled" category, callers must combine this CLI
//! output with a filesystem scan (see `crate::plugins`).

use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::claude_binary;
use crate::process_ext::NoWindowExt;

#[derive(Debug, Clone)]
pub enum ClaudeCliError {
    /// `claude` binary not found on disk.
    BinaryNotFound,
    /// Process spawn failed (permissions, OS error).
    ExecFailed(String),
    /// CLI ran but exited non-zero.
    NonZeroExit {
        code: Option<i32>,
        stderr: String,
    },
    /// stdout did not parse as the expected JSON shape.
    InvalidJson { raw: String, error: String },
}

impl std::fmt::Display for ClaudeCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BinaryNotFound => write!(f, "claude binary not found"),
            Self::ExecFailed(e) => write!(f, "failed to spawn claude: {}", e),
            Self::NonZeroExit { code, stderr } => write!(
                f,
                "claude exited with code {:?}: {}",
                code,
                stderr.trim()
            ),
            Self::InvalidJson { error, .. } => write!(f, "invalid JSON from claude: {}", error),
        }
    }
}

impl std::error::Error for ClaudeCliError {}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginsListResponse {
    #[serde(default)]
    pub installed: Vec<CliPlugin>,
    #[serde(default)]
    pub available: Vec<CliPlugin>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CliPlugin {
    pub plugin_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub marketplace_name: String,
    /// `source` may be a string (local path) or an object (git/git-subdir/url).
    /// Kept as raw JSON so the UI can render whichever shape it wants.
    #[serde(default)]
    pub source: serde_json::Value,
    #[serde(default)]
    pub install_count: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CliMarketplace {
    pub name: String,
    /// "github", "git", "url", "local", etc. Optional because shape may vary.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub install_location: Option<String>,
}

// ── Process invocation ────────────────────────────────────────────────────────

pub fn list_plugins(include_available: bool) -> Result<PluginsListResponse, ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    let mut args: Vec<&str> = vec!["plugin", "list", "--json"];
    if include_available {
        args.push("--available");
    }
    run_claude_json(&bin.path, &args)
}

pub fn list_marketplaces() -> Result<Vec<CliMarketplace>, ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    run_claude_json(&bin.path, &["plugin", "marketplace", "list", "--json"])
}

/// Run `claude plugin enable <plugin_id>` or `claude plugin disable <plugin_id>`
/// against the locally-resolved claude binary. Returns `Ok(())` on success,
/// or surfaces the CLI's own stderr inside [`ClaudeCliError::NonZeroExit`].
pub fn set_plugin_enabled(plugin_id: &str, enabled: bool) -> Result<(), ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    let action = if enabled { "enable" } else { "disable" };
    run_claude_silent(&bin.path, &["plugin", action, plugin_id])
}

/// Run `claude plugin install <plugin_id>`. May take tens of seconds because
/// CC fetches the plugin contents (git/GCS) under the hood.
pub fn install_plugin(plugin_id: &str) -> Result<(), ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    run_claude_silent(&bin.path, &["plugin", "install", plugin_id])
}

/// Run `claude plugin uninstall <plugin_id>`. CC removes the on-disk
/// directory and prunes the plugin from `enabledPlugins`.
pub fn uninstall_plugin(plugin_id: &str) -> Result<(), ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    run_claude_silent(&bin.path, &["plugin", "uninstall", plugin_id])
}

/// Run `claude plugin marketplace add <source>`. `source` is passed through
/// verbatim — the CLI accepts a GitHub `owner/repo`, a git URL, or a local
/// path.
pub fn add_marketplace(source: &str) -> Result<(), ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    run_claude_silent(&bin.path, &["plugin", "marketplace", "add", source])
}

/// Run `claude plugin marketplace remove <name>`.
pub fn remove_marketplace(name: &str) -> Result<(), ClaudeCliError> {
    let bin = claude_binary::resolve(None).ok_or(ClaudeCliError::BinaryNotFound)?;
    run_claude_silent(&bin.path, &["plugin", "marketplace", "remove", name])
}

fn run_claude_silent(bin: &str, args: &[&str]) -> Result<(), ClaudeCliError> {
    let output = Command::new(bin)
        .no_window()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ClaudeCliError::ExecFailed(e.to_string()))?;
    if !output.status.success() {
        return Err(ClaudeCliError::NonZeroExit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(())
}

fn run_claude_json<T: serde::de::DeserializeOwned>(
    bin: &str,
    args: &[&str],
) -> Result<T, ClaudeCliError> {
    let output = Command::new(bin)
        .no_window()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ClaudeCliError::ExecFailed(e.to_string()))?;
    if !output.status.success() {
        return Err(ClaudeCliError::NonZeroExit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    parse_json(&stdout)
}

fn parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, ClaudeCliError> {
    serde_json::from_str::<T>(raw).map_err(|e| ClaudeCliError::InvalidJson {
        raw: raw.to_string(),
        error: e.to_string(),
    })
}

pub fn parse_plugins_list_response(raw: &str) -> Result<PluginsListResponse, ClaudeCliError> {
    parse_json(raw)
}

pub fn parse_marketplaces_list_response(raw: &str) -> Result<Vec<CliMarketplace>, ClaudeCliError> {
    parse_json(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plugins_full_shape() {
        let raw = r#"{
            "installed": [],
            "available": [
                {
                    "pluginId": "foo@mk",
                    "name": "foo",
                    "description": "a plugin",
                    "marketplaceName": "mk",
                    "source": {"source": "git-subdir", "url": "https://x.example/r.git", "path": "p"},
                    "installCount": 42
                }
            ]
        }"#;
        let parsed = parse_plugins_list_response(raw).unwrap();
        assert_eq!(parsed.installed.len(), 0);
        assert_eq!(parsed.available.len(), 1);
        let p = &parsed.available[0];
        assert_eq!(p.plugin_id, "foo@mk");
        assert_eq!(p.marketplace_name, "mk");
        assert_eq!(p.install_count, Some(42));
    }

    #[test]
    fn parse_plugins_handles_string_source() {
        // CC's marketplace.json sometimes encodes source as a bare path string.
        let raw = r#"{
            "available": [
                {
                    "pluginId": "bar@mk",
                    "name": "bar",
                    "marketplaceName": "mk",
                    "source": "./plugins/bar"
                }
            ]
        }"#;
        let parsed = parse_plugins_list_response(raw).unwrap();
        assert_eq!(parsed.available[0].plugin_id, "bar@mk");
        assert!(parsed.available[0].source.is_string());
    }

    #[test]
    fn parse_plugins_missing_install_count() {
        let raw = r#"{
            "available": [
                {"pluginId": "p@m", "name": "p", "marketplaceName": "m", "source": {}}
            ]
        }"#;
        let parsed = parse_plugins_list_response(raw).unwrap();
        assert!(parsed.available[0].install_count.is_none());
    }

    #[test]
    fn parse_plugins_invalid_json() {
        let err = parse_plugins_list_response("not json").unwrap_err();
        assert!(matches!(err, ClaudeCliError::InvalidJson { .. }));
    }

    #[test]
    fn parse_plugins_default_when_arrays_missing() {
        // CC may omit `installed` or `available` entirely.
        let raw = r#"{}"#;
        let parsed = parse_plugins_list_response(raw).unwrap();
        assert!(parsed.installed.is_empty());
        assert!(parsed.available.is_empty());
    }

    #[test]
    fn parse_marketplaces_full_shape() {
        let raw = r#"[
            {
                "name": "claude-plugins-official",
                "source": "github",
                "repo": "anthropics/claude-plugins-official",
                "installLocation": "/Users/x/.claude/plugins/marketplaces/claude-plugins-official"
            }
        ]"#;
        let parsed = parse_marketplaces_list_response(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "claude-plugins-official");
        assert_eq!(parsed[0].source.as_deref(), Some("github"));
        assert_eq!(parsed[0].repo.as_deref(), Some("anthropics/claude-plugins-official"));
    }

    #[test]
    fn parse_marketplaces_empty() {
        let parsed = parse_marketplaces_list_response("[]").unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_marketplaces_optional_fields() {
        // Local-source marketplace wouldn't have `repo`.
        let raw = r#"[{"name": "local-mk", "source": "local", "path": "/tmp/mk"}]"#;
        let parsed = parse_marketplaces_list_response(raw).unwrap();
        assert_eq!(parsed[0].name, "local-mk");
        assert_eq!(parsed[0].path.as_deref(), Some("/tmp/mk"));
        assert!(parsed[0].repo.is_none());
    }
}
