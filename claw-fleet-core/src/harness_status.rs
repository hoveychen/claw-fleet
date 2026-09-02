//! Unified per-harness environment probe.
//!
//! One symmetric answer to "is this agent harness usable on this machine?"
//! for every source Fleet can drive (claude-code / codex / dsh). The existing
//! `SetupStatus`/`DetectedTools` shape grew Claude-first (booleans, no dsh, no
//! versions); the environment wizard needs the same five facts per harness —
//! installed, where, which version, through which install channel, logged in
//! as what — so it can decide which remediation button to show.
//!
//! Probes are read-only and offline: filesystem + keychain reads plus one
//! `<bin> --version` subprocess per installed harness. Nothing here touches
//! the network or mutates harness state, so the wizard can re-probe freely.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a `<bin> --version` probe may run before we kill it. dsh is a
/// node script whose interpreter cold-start dominates; 10s is far above any
/// healthy run while still bounding a wedged binary.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// One agent harness's install/auth state on this machine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessStatus {
    /// Agent source id, matching `agent_source` naming: "claude-code" |
    /// "codex" | "dsh".
    pub source: String,
    pub installed: bool,
    pub path: Option<String>,
    /// Best-effort output of `<bin> --version` (normalized to the bare
    /// version token, e.g. "2.1.246"). `None` when the binary is missing or
    /// the probe failed — absence of a version is not absence of the binary.
    pub version: Option<String>,
    /// Install-channel key (stable kebab-case, i18n lookup on the frontend):
    /// "native-installer", "standalone", "npm-global", "vscode-extension", …
    pub channel: Option<String>,
    /// `None` means the harness has no login concept (dsh is bring-your-own-key);
    /// `Some(false)` means it does and nobody is logged in.
    pub logged_in: Option<bool>,
    /// Human-oriented auth detail: Claude subscription type ("max"), Codex
    /// auth mode ("chatgpt" / "api-key"), …
    pub auth_detail: Option<String>,
}

/// Probe every harness Fleet knows how to drive, in stable order.
pub fn probe_all() -> Vec<HarnessStatus> {
    vec![probe_claude(), probe_codex(), probe_dsh()]
}

/// Probe a single harness by its source id; `None` for an unknown id.
pub fn probe_source(source: &str) -> Option<HarnessStatus> {
    match source {
        "claude-code" => Some(probe_claude()),
        "codex" => Some(probe_codex()),
        "dsh" => Some(probe_dsh()),
        _ => None,
    }
}

// ── claude-code ───────────────────────────────────────────────────────────────

fn probe_claude() -> HarnessStatus {
    let override_path = crate::claude_binary::ClaudeBinaryConfig::load().override_path;
    let resolved = crate::claude_binary::resolve(override_path.as_deref());

    let (installed, path, channel, dir_version) = match &resolved {
        Some(bin) => (
            true,
            Some(bin.path.clone()),
            Some(claude_channel(bin).to_string()),
            bin.version.clone(),
        ),
        None => (false, None, None, None),
    };

    // A live `--version` beats the version parsed out of an extension dir
    // name: the dir name is the bundle's version, not necessarily what the
    // binary reports after a self-update.
    let version = path
        .as_deref()
        .and_then(probe_version)
        .or(dir_version);

    let (logged_in, auth_detail) = match crate::account::read_keychain_credentials() {
        Ok((_token, subscription)) => (Some(true), Some(subscription)),
        // The wizard's setup-token flow stores a long-lived token under
        // ~/.fleet and injects it at spawn time — that IS a working login for
        // Fleet-driven sessions, so report it as one.
        Err(_) if crate::harness_login::stored_claude_token().is_some() => {
            (Some(true), Some("fleet-token".to_string()))
        }
        Err(_) => (Some(false), None),
    };

    HarnessStatus {
        source: "claude-code".to_string(),
        installed,
        path,
        version,
        channel,
        logged_in,
        auth_detail,
    }
}

/// Install-channel key for a resolved claude binary. `discover()` labels a
/// PATH hit as `Path` even when the file is the native installer's symlink
/// (`~/.local/bin/claude` → `~/.local/share/claude/versions/<ver>`), but the
/// wizard's upgrade action branches on the real channel — so reclassify a
/// `path` hit by its canonical location before giving up.
fn claude_channel(bin: &crate::claude_binary::ClaudeBinary) -> String {
    let key = bin.source.key();
    if key != "path" {
        return key.to_string();
    }
    claude_channel_for_path_hit(Path::new(&bin.path)).to_string()
}

/// Pure classification of a PATH-resolved claude binary, split for testing.
fn claude_channel_for_path_hit(path: &Path) -> &'static str {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let canon = canon.to_string_lossy().replace('\\', "/");
    if canon.contains(".local/share/claude") {
        // Native installer: versioned real binary behind a ~/.local/bin symlink.
        "native-installer"
    } else if canon.contains("node_modules") {
        "npm-global"
    } else if canon.contains("/Cellar/") || canon.contains("/Caskroom/") || canon.contains("/homebrew/") {
        "homebrew"
    } else {
        "path"
    }
}

// ── codex ─────────────────────────────────────────────────────────────────────

fn probe_codex() -> HarnessStatus {
    let found = crate::codex_source::find_codex_binary();
    let (installed, path, channel) = match &found {
        Some(p) => {
            let s = p.to_string_lossy().to_string();
            let channel = codex_channel_for_path(&s);
            (true, Some(s), Some(channel.to_string()))
        }
        None => (false, None, None),
    };

    let version = path.as_deref().and_then(probe_version);
    let (logged_in, auth_detail) =
        codex_auth_state(crate::codex_launch::codex_home().as_deref());

    HarnessStatus {
        source: "codex".to_string(),
        installed,
        path,
        version,
        channel,
        logged_in,
        auth_detail,
    }
}

/// Install-channel key from the resolved codex binary path. Mirrors the
/// resolution order in `codex_source::find_codex_binary`.
fn codex_channel_for_path(path: &str) -> &'static str {
    if path.contains("packages/standalone") || path.contains("packages\\standalone") {
        // Codex's own self-updating install (`current` → newest release).
        "standalone"
    } else if path.contains(".vscode-insiders") {
        "vscode-insiders-extension"
    } else if path.contains(".vscode") {
        "vscode-extension"
    } else {
        "path"
    }
}

/// Login state from `<CODEX_HOME>/auth.json` — a read, not a `codex login
/// status` subprocess: the probe may run on every wizard-panel open, and the
/// file is the same store the CLI itself reads (shape verified against
/// codex-rs: `auth_mode` + `tokens{access_token,…}` or `OPENAI_API_KEY`).
fn codex_auth_state(codex_home: Option<&Path>) -> (Option<bool>, Option<String>) {
    let Some(home) = codex_home else {
        return (Some(false), None);
    };
    let Ok(raw) = std::fs::read_to_string(home.join("auth.json")) else {
        return (Some(false), None);
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return (Some(false), None);
    };
    codex_auth_state_from_json(&v)
}

/// Pure predicate over a parsed auth.json, split out for unit testing.
fn codex_auth_state_from_json(v: &Value) -> (Option<bool>, Option<String>) {
    let has_api_key = v
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    let has_tokens = v
        .get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(Value::as_str)
        .map(|t| !t.is_empty())
        .unwrap_or(false);
    let auth_mode = v.get("auth_mode").and_then(Value::as_str);

    if has_tokens {
        // ChatGPT-style OAuth session; auth_mode names the variant.
        (Some(true), Some(auth_mode.unwrap_or("chatgpt").to_string()))
    } else if has_api_key {
        (Some(true), Some("api-key".to_string()))
    } else {
        (Some(false), None)
    }
}

// ── dsh ───────────────────────────────────────────────────────────────────────

fn probe_dsh() -> HarnessStatus {
    let found = crate::dsh_server::discover();
    let (installed, path, channel) = match &found {
        Some(p) => {
            let s = p.to_string_lossy().to_string();
            let channel = dsh_channel_for_path(p);
            (true, Some(s), Some(channel.to_string()))
        }
        None => (false, None, None),
    };

    let version = path.as_deref().and_then(probe_version);

    HarnessStatus {
        source: "dsh".to_string(),
        installed,
        path,
        version,
        channel,
        // dsh has no account layer at all (bring-your-own-key harness); the
        // wizard's dsh card shows provider-credential state instead, which is
        // the login-flows plan's concern, not this probe's.
        logged_in: None,
        auth_detail: None,
    }
}

/// dsh only installs through npm; the on-PATH file is a symlink into the npm
/// prefix's `node_modules`, so resolve the link before classifying.
fn dsh_channel_for_path(path: &Path) -> &'static str {
    if std::env::var_os("FLEET_DSH_BIN")
        .map(|v| PathBuf::from(v) == *path)
        .unwrap_or(false)
    {
        return "override";
    }
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let canon = canon.to_string_lossy();
    if canon.contains("node_modules") {
        "npm-global"
    } else {
        "path"
    }
}

// ── version probe ─────────────────────────────────────────────────────────────

/// Run `<bin> --version` with the same augmented PATH Fleet gives spawned
/// agents (a GUI app's own PATH is only the four system dirs, which would
/// strand dsh's `#!/usr/bin/env node` shebang), bounded by
/// [`VERSION_PROBE_TIMEOUT`], and normalize the output to a bare version
/// token.
fn probe_version(bin: &str) -> Option<String> {
    let mut cmd = crate::process_util::command(bin);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd.env("PATH", crate::session_launch::augmented_path_with_front(&[]));

    let child = cmd.spawn().ok()?;
    let child_id = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(VERSION_PROBE_TIMEOUT) {
        Ok(Ok(out)) if out.status.success() => {
            parse_version_token(&String::from_utf8_lossy(&out.stdout))
        }
        Ok(_) => None,
        Err(_) => {
            crate::llm_provider::kill_process(child_id);
            None
        }
    }
}

/// Extract the version token from a `--version` line. Handles the three real
/// shapes: `2.1.246 (Claude Code)`, `codex-cli 0.148.0`, `0.1.1-rc.2` — the
/// first whitespace-separated token that starts with a digit and contains a
/// dot.
pub(crate) fn parse_version_token(output: &str) -> Option<String> {
    output
        .lines()
        .find(|l| !l.trim().is_empty())?
        .split_whitespace()
        .find(|tok| {
            tok.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && tok.contains('.')
        })
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_version_token_handles_all_three_harness_shapes() {
        assert_eq!(
            parse_version_token("2.1.246 (Claude Code)").as_deref(),
            Some("2.1.246")
        );
        assert_eq!(
            parse_version_token("codex-cli 0.148.0").as_deref(),
            Some("0.148.0")
        );
        assert_eq!(parse_version_token("0.1.1-rc.2").as_deref(), Some("0.1.1-rc.2"));
    }

    #[test]
    fn parse_version_token_skips_leading_blank_lines_and_rejects_garbage() {
        assert_eq!(
            parse_version_token("\n\ncodex-cli 0.148.0\n").as_deref(),
            Some("0.148.0")
        );
        assert_eq!(parse_version_token("no version here"), None);
        assert_eq!(parse_version_token(""), None);
        // A bare integer is not a version; require a dot.
        assert_eq!(parse_version_token("exit 1"), None);
    }

    #[test]
    fn codex_auth_chatgpt_tokens_read_as_logged_in() {
        let v = json!({
            "auth_mode": "chatgpt",
            "tokens": { "access_token": "tok", "refresh_token": "r", "account_id": "a" },
            "last_refresh": "2026-09-01T00:00:00Z"
        });
        assert_eq!(
            codex_auth_state_from_json(&v),
            (Some(true), Some("chatgpt".to_string()))
        );
    }

    #[test]
    fn codex_auth_api_key_reads_as_logged_in() {
        let v = json!({ "auth_mode": "apikey", "OPENAI_API_KEY": "sk-test" });
        assert_eq!(
            codex_auth_state_from_json(&v),
            (Some(true), Some("api-key".to_string()))
        );
    }

    #[test]
    fn codex_auth_empty_or_blank_reads_as_logged_out() {
        assert_eq!(codex_auth_state_from_json(&json!({})), (Some(false), None));
        let blank = json!({ "OPENAI_API_KEY": "", "tokens": { "access_token": "" } });
        assert_eq!(codex_auth_state_from_json(&blank), (Some(false), None));
    }

    #[test]
    fn codex_auth_missing_home_or_file_reads_as_logged_out() {
        assert_eq!(codex_auth_state(None), (Some(false), None));
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(codex_auth_state(Some(tmp.path())), (Some(false), None));
    }

    #[test]
    fn codex_channel_classifies_resolution_order() {
        let home = "/Users/u";
        assert_eq!(
            codex_channel_for_path(&format!(
                "{home}/.codex/packages/standalone/current/bin/codex"
            )),
            "standalone"
        );
        assert_eq!(
            codex_channel_for_path(&format!(
                "{home}/.vscode-insiders/extensions/openai.chatgpt-1.0/bin/macos-aarch64/codex"
            )),
            "vscode-insiders-extension"
        );
        assert_eq!(
            codex_channel_for_path(&format!(
                "{home}/.vscode/extensions/openai.chatgpt-1.0/bin/macos-aarch64/codex"
            )),
            "vscode-extension"
        );
        assert_eq!(codex_channel_for_path("codex"), "path");
        // Windows-style separators classify the same.
        assert_eq!(
            codex_channel_for_path(r"C:\Users\u\.codex\packages\standalone\current\bin\codex.exe"),
            "standalone"
        );
    }

    #[test]
    fn claude_path_hit_reclassifies_by_canonical_location() {
        let tmp = tempfile::tempdir().unwrap();
        // Native-installer layout: ~/.local/bin/claude → ~/.local/share/claude/versions/x
        let share = tmp.path().join(".local/share/claude/versions");
        std::fs::create_dir_all(&share).unwrap();
        let real = share.join("2.1.246");
        std::fs::write(&real, b"bin").unwrap();
        #[cfg(unix)]
        {
            let bin_dir = tmp.path().join(".local/bin");
            std::fs::create_dir_all(&bin_dir).unwrap();
            let link = bin_dir.join("claude");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(claude_channel_for_path_hit(&link), "native-installer");
        }
        assert_eq!(claude_channel_for_path_hit(&real), "native-installer");

        let npm = tmp.path().join("lib/node_modules/@anthropic-ai/claude-code/cli.js");
        std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
        std::fs::write(&npm, b"js").unwrap();
        assert_eq!(claude_channel_for_path_hit(&npm), "npm-global");

        let plain = tmp.path().join("claude");
        std::fs::write(&plain, b"bin").unwrap();
        assert_eq!(claude_channel_for_path_hit(&plain), "path");
    }

    #[test]
    fn dsh_channel_resolves_symlink_into_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let target_dir = tmp.path().join("lib").join("node_modules").join("dshpkg");
        std::fs::create_dir_all(&target_dir).unwrap();
        let target = target_dir.join("bin.js");
        std::fs::write(&target, b"#!/usr/bin/env node\n").unwrap();

        #[cfg(unix)]
        {
            let link = tmp.path().join("dsh");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(dsh_channel_for_path(&link), "npm-global");
        }
        // A direct file inside node_modules classifies without a symlink too.
        assert_eq!(dsh_channel_for_path(&target), "npm-global");

        let plain = tmp.path().join("plain-dsh");
        std::fs::write(&plain, b"bin").unwrap();
        assert_eq!(dsh_channel_for_path(&plain), "path");
    }

    /// Manual smoke probe against the real machine — run with
    /// `cargo test -p claw-fleet-core --lib probe_all_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_all_smoke_prints_real_statuses() {
        for s in probe_all() {
            println!("{s:?}");
        }
    }

    #[test]
    fn probe_all_returns_stable_source_order() {
        // Pure shape check: whatever the host machine has installed, the
        // probe must answer for all three sources in stable order.
        let all = probe_all();
        let ids: Vec<&str> = all.iter().map(|s| s.source.as_str()).collect();
        assert_eq!(ids, vec!["claude-code", "codex", "dsh"]);
    }
}
