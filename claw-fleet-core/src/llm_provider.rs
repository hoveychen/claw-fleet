//! LLM provider abstraction — trait + CLI implementations for Claude, Codex, and Cursor.
//!
//! Each provider wraps its respective CLI tool for non-interactive text completion.
//! The trait is used by `claude_analyze` and `daily_report` modules so that any
//! supported CLI can power session analysis, report summaries, and lesson extraction.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::log_debug;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LlmModel {
    pub id: String,
    pub display_name: String,
}

/// Snapshot of a provider's identity + available models (for UI display).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderInfo {
    pub name: String,
    pub display_name: String,
    pub available: bool,
    pub models: Vec<LlmModel>,
    pub default_fast_model: String,
    pub default_standard_model: String,
}

/// Persisted user config: which provider + models to use for analysis tasks.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub provider: String,
    pub fast_model: String,
    pub standard_model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "claude".into(),
            fast_model: "haiku".into(),
            standard_model: "sonnet".into(),
        }
    }
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Real token + cost numbers as reported by the underlying provider.
/// Present only when the CLI emits structured usage (Claude `--output-format json`);
/// `None` for providers that return text only.
#[derive(Clone, Debug, Default)]
pub struct CompletionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost_usd: f64,
}

/// Result of a `complete` call — always has text, may have real usage.
#[derive(Clone, Debug)]
pub struct Completion {
    pub text: String,
    pub usage: Option<CompletionUsage>,
}

pub trait LlmProvider: Send + Sync {
    /// Short identifier: "claude", "codex", "cursor".
    fn name(&self) -> &str;
    /// Human-readable display name.
    fn display_name(&self) -> &str;
    /// Whether the CLI binary is found on this machine.
    fn is_available(&self) -> bool;
    /// List models this provider supports.
    fn list_models(&self) -> Vec<LlmModel>;
    /// Recommended model for quick / cheap tasks (e.g. outcome classification).
    fn default_fast_model(&self) -> &str;
    /// Recommended model for complex tasks (e.g. report summaries, lessons).
    fn default_standard_model(&self) -> &str;
    /// Send a prompt and return the completion. Blocks up to `timeout`.
    /// When available, `Completion.usage` carries provider-reported token/cost
    /// numbers; otherwise callers fall back to character-based estimation.
    fn complete(&self, prompt: &str, model: &str, timeout: Duration) -> Option<Completion>;
}

// ── Shared helpers ───────────────────────────────────────────────────────────

/// Kill a process by PID in a cross-platform way.
pub fn kill_process(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Resolve a binary name to its full path, checking `which`/`where` and common
/// install locations.
fn resolve_binary(name: &str, extra_paths: &[&str]) -> Option<String> {
    #[cfg(unix)]
    let which = "which";
    #[cfg(not(unix))]
    let which = "where";

    if let Ok(output) = Command::new(which).arg(name).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    for tpl in extra_paths {
        let expanded = if tpl.starts_with("~/") {
            if let Some(home) = crate::session::real_home_dir() {
                home.join(&tpl[2..]).to_string_lossy().to_string()
            } else {
                continue;
            }
        } else {
            tpl.to_string()
        };
        if std::path::Path::new(&expanded).exists() {
            return Some(expanded);
        }
    }

    None
}

/// Spawn a CLI process with piped stdout, suppressed stderr/stdin, and a
/// timeout.  Returns the stdout content on success, or None on failure/timeout.
fn run_cli(
    bin: &str,
    args: &[&str],
    timeout: Duration,
    tag: &str,
) -> Option<String> {
    // Only log the binary and flag names, not prompt content (which can be huge).
    let safe_args: Vec<&str> = args.iter().map(|a| {
        if a.len() > 80 { "<prompt…>" } else { a }
    }).collect();
    log_debug(&format!("[{tag}] spawning: {bin} {}", safe_args.join(" ")));
    let child = match Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_debug(&format!("[{tag}] failed to spawn: {e}"));
            return None;
        }
    };

    let (tx, rx) = mpsc::channel();
    let child_id = child.id();
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    let output = match rx.recv_timeout(timeout) {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            log_debug(&format!("[{tag}] exited with status {}", output.status));
            return None;
        }
        Ok(Err(e)) => {
            log_debug(&format!("[{tag}] wait error: {e}"));
            return None;
        }
        Err(_) => {
            log_debug(&format!(
                "[{tag}] timed out after {}s, killing pid={child_id}",
                timeout.as_secs()
            ));
            kill_process(child_id);
            return None;
        }
    };

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        log_debug(&format!("[{tag}] empty response"));
        return None;
    }

    Some(raw)
}

// ── Claude CLI provider ──────────────────────────────────────────────────────

pub struct ClaudeCliProvider {
    bin_path: Option<String>,
}

impl ClaudeCliProvider {
    pub fn new() -> Self {
        // Use the unified discoverer so the LLM-completion path benefits from
        // the same IDE-extension scan and user override that auto-resume uses.
        let config = crate::claude_binary::ClaudeBinaryConfig::load();
        let bin_path = crate::claude_binary::resolve(config.override_path.as_deref())
            .map(|b| b.path);
        Self { bin_path }
    }
}

impl LlmProvider for ClaudeCliProvider {
    fn name(&self) -> &str { "claude" }
    fn display_name(&self) -> &str { "Claude Code" }

    fn is_available(&self) -> bool {
        self.bin_path.is_some()
    }

    fn list_models(&self) -> Vec<LlmModel> {
        vec![
            LlmModel { id: "opus".into(), display_name: "Opus".into() },
            LlmModel { id: "sonnet".into(), display_name: "Sonnet".into() },
            LlmModel { id: "haiku".into(), display_name: "Haiku".into() },
        ]
    }

    fn default_fast_model(&self) -> &str { "haiku" }
    fn default_standard_model(&self) -> &str { "sonnet" }

    fn complete(&self, prompt: &str, model: &str, timeout: Duration) -> Option<Completion> {
        let bin = self.bin_path.as_deref()?;
        // `--output-format json` makes Claude Code emit a single JSON object with
        // `result` (the text) and a `usage` block carrying real token counts
        // (including the ~36k cache_creation tokens CLI injects from its
        // bundled system prompt / CLAUDE.md / tool defs). Without this flag the
        // CLI only prints the assistant text, which forced us to estimate —
        // and that estimate was off by orders of magnitude because we couldn't
        // see the cache-creation head.
        let raw = run_cli(
            bin,
            &[
                "-p", prompt,
                "--model", model,
                "--no-session-persistence",
                "--output-format", "json",
            ],
            timeout,
            "llm:claude",
        )?;
        parse_claude_json_response(&raw)
    }
}

/// Parse Claude Code's `--output-format json` response into a `Completion`.
/// Returns `None` on malformed JSON or when the run reported an error.
fn parse_claude_json_response(raw: &str) -> Option<Completion> {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            log_debug(&format!("[llm:claude] json parse failed: {e}"));
            return None;
        }
    };
    if v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) {
        log_debug("[llm:claude] response marked is_error=true");
        return None;
    }
    let text = v.get("result").and_then(|r| r.as_str())?.to_string();
    let usage = v.get("usage").map(|u| CompletionUsage {
        input_tokens: u.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        output_tokens: u.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        cache_creation_tokens: u
            .get("cache_creation_input_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        cache_read_tokens: u
            .get("cache_read_input_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        total_cost_usd: v.get("total_cost_usd").and_then(|n| n.as_f64()).unwrap_or(0.0),
    });
    Some(Completion { text, usage })
}

// ── Codex CLI provider ───────────────────────────────────────────────────────

pub struct CodexCliProvider {
    bin_path: Option<String>,
}

impl CodexCliProvider {
    pub fn new() -> Self {
        let bin_path = resolve_binary("codex", &[
            "~/.local/bin/codex",
            "/usr/local/bin/codex",
            "/opt/homebrew/bin/codex",
        ]);
        Self { bin_path }
    }
}

impl LlmProvider for CodexCliProvider {
    fn name(&self) -> &str { "codex" }
    fn display_name(&self) -> &str { "Codex" }

    fn is_available(&self) -> bool {
        self.bin_path.is_some()
    }

    fn list_models(&self) -> Vec<LlmModel> {
        // Read from ~/.codex/models_cache.json (written by the Codex CLI on login).
        // Falls back to a small hardcoded set if the cache is missing.
        if let Some(models) = parse_codex_models_cache() {
            if !models.is_empty() {
                return models;
            }
        }
        // Fallback
        vec![
            LlmModel { id: "gpt-5.3-codex".into(), display_name: "gpt-5.3-codex".into() },
            LlmModel { id: "gpt-5.1-codex-mini".into(), display_name: "gpt-5.1-codex-mini".into() },
        ]
    }

    fn default_fast_model(&self) -> &str { "gpt-5.1-codex-mini" }
    fn default_standard_model(&self) -> &str { "gpt-5.3-codex" }

    fn complete(&self, prompt: &str, model: &str, timeout: Duration) -> Option<Completion> {
        let bin = self.bin_path.as_deref()?;
        // exec: non-interactive mode (stdout = final message only)
        // --ephemeral: don't persist session
        // --full-auto: auto-approve (no interactive prompts)
        // --skip-git-repo-check: we're not in a repo context
        // --sandbox read-only: prevent file writes (pure text generation)
        let text = run_cli(
            bin,
            &[
                "exec",
                prompt,
                "-m", model,
                "--ephemeral",
                "--full-auto",
                "--skip-git-repo-check",
                "--sandbox", "read-only",
            ],
            timeout,
            "llm:codex",
        )?;
        Some(Completion { text, usage: None })
    }
}

// ── Cursor Agent CLI provider ────────────────────────────────────────────────

pub struct CursorCliProvider {
    bin_path: Option<String>,
}

impl CursorCliProvider {
    pub fn new() -> Self {
        let bin_path = resolve_binary("agent", &[
            "~/.local/bin/agent",
            "/usr/local/bin/agent",
        ]);
        Self { bin_path }
    }
}

impl LlmProvider for CursorCliProvider {
    fn name(&self) -> &str { "cursor" }
    fn display_name(&self) -> &str { "Cursor Agent" }

    fn is_available(&self) -> bool {
        self.bin_path.is_some()
    }

    fn list_models(&self) -> Vec<LlmModel> {
        let bin = match self.bin_path.as_deref() {
            Some(b) => b,
            None => return Vec::new(),
        };

        // `agent models` outputs lines like:
        //   model-id - Display Name
        //   model-id - Display Name  (default)
        // with ANSI escape codes mixed in.
        let output = match Command::new(bin).arg("models").output() {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };

        let raw = String::from_utf8_lossy(&output.stdout);
        parse_cursor_models(&raw)
    }

    fn default_fast_model(&self) -> &str { "composer-2-fast" }
    fn default_standard_model(&self) -> &str { "composer-2" }

    fn complete(&self, prompt: &str, model: &str, timeout: Duration) -> Option<Completion> {
        let bin = self.bin_path.as_deref()?;
        // -p: print mode (non-interactive, output to stdout)
        // -f: trust workspace without interactive prompt
        // --mode ask: read-only Q&A (no file writes)
        let text = run_cli(
            bin,
            &["-p", "-f", "--model", model, "--mode", "ask", prompt],
            timeout,
            "llm:cursor",
        )?;
        Some(Completion { text, usage: None })
    }
}

/// Strip ANSI escape codes from a string.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC [ ... (final byte in 0x40–0x7E range)
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc.is_ascii() && (0x40..=0x7E).contains(&(nc as u8)) {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Parse `agent models` output into `LlmModel` entries.
fn parse_cursor_models(raw: &str) -> Vec<LlmModel> {
    let clean = strip_ansi(raw);
    let mut models = Vec::new();

    for line in clean.lines() {
        let trimmed = line.trim();
        // Skip headers, empty lines, and the "Tip:" line
        if trimmed.is_empty()
            || trimmed.starts_with("Available")
            || trimmed.starts_with("Loading")
            || trimmed.starts_with("Tip:")
        {
            continue;
        }
        // Expected format: "model-id - Display Name" or "model-id - Display Name  (default)"
        if let Some((id, rest)) = trimmed.split_once(" - ") {
            let display = rest.trim_end_matches("(default)").trim().to_string();
            models.push(LlmModel {
                id: id.trim().to_string(),
                display_name: display,
            });
        }
    }

    models
}

/// Read `~/.codex/models_cache.json` and return non-hidden models.
fn parse_codex_models_cache() -> Option<Vec<LlmModel>> {
    let path = crate::session::real_home_dir()?.join(".codex").join("models_cache.json");
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;

    // The cache is either `{ "models": [...] }` or a bare array.
    let arr = val.get("models").and_then(|v| v.as_array())
        .or_else(|| val.as_array())?;

    let models: Vec<LlmModel> = arr.iter()
        .filter(|m| !m.get("is_hidden").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|m| {
            let slug = m.get("slug").and_then(|v| v.as_str())?;
            let display = m.get("display_name").and_then(|v| v.as_str()).unwrap_or(slug);
            Some(LlmModel {
                id: slug.to_string(),
                display_name: display.to_string(),
            })
        })
        .collect();

    Some(models)
}

// ── Provider registry ────────────────────────────────────────────────────────

/// Create a provider by name.
pub fn resolve_provider(name: &str) -> Option<Box<dyn LlmProvider>> {
    match name {
        "claude" => Some(Box::new(ClaudeCliProvider::new())),
        "codex" => Some(Box::new(CodexCliProvider::new())),
        "cursor" => Some(Box::new(CursorCliProvider::new())),
        _ => None,
    }
}

/// Return info snapshots for all known providers, including a "none" (disabled)
/// option at the end.
pub fn all_provider_infos() -> Vec<LlmProviderInfo> {
    let providers: Vec<Box<dyn LlmProvider>> = vec![
        Box::new(ClaudeCliProvider::new()),
        Box::new(CodexCliProvider::new()),
        Box::new(CursorCliProvider::new()),
    ];

    let mut infos: Vec<LlmProviderInfo> = providers
        .into_iter()
        .map(|p| LlmProviderInfo {
            name: p.name().into(),
            display_name: p.display_name().into(),
            available: p.is_available(),
            models: p.list_models(),
            default_fast_model: p.default_fast_model().into(),
            default_standard_model: p.default_standard_model().into(),
        })
        .collect();

    // "none" — disable LLM analysis entirely.
    infos.push(LlmProviderInfo {
        name: "none".into(),
        display_name: "Disabled".into(),
        available: true,
        models: vec![],
        default_fast_model: String::new(),
        default_standard_model: String::new(),
    });

    infos
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_basic() {
        assert_eq!(strip_ansi("\x1b[2Khello"), "hello");
        assert_eq!(strip_ansi("\x1b[1A\x1b[2K\x1b[Gtext"), "text");
        assert_eq!(strip_ansi("no escapes"), "no escapes");
    }

    #[test]
    fn parse_cursor_models_real_output() {
        let raw = "\x1b[2K\x1b[GLoading models…\n\
                    \x1b[2K\x1b[1A\x1b[2K\x1b[GAvailable models\n\
                    \n\
                    auto - Auto\n\
                    composer-2-fast - Composer 2 Fast  (default)\n\
                    composer-2 - Composer 2\n\
                    grok-4-20 - Grok 4.20\n\
                    \n\
                    Tip: use --model <id> to switch.";
        let models = parse_cursor_models(raw);
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].id, "auto");
        assert_eq!(models[0].display_name, "Auto");
        assert_eq!(models[1].id, "composer-2-fast");
        assert_eq!(models[1].display_name, "Composer 2 Fast");
        assert_eq!(models[3].id, "grok-4-20");
    }

    #[test]
    fn resolve_unknown_provider_returns_none() {
        assert!(resolve_provider("unknown").is_none());
    }

    #[test]
    fn claude_provider_lists_models() {
        let p = ClaudeCliProvider::new();
        let models = p.list_models();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "opus");
    }

    #[test]
    fn codex_provider_lists_models() {
        let p = CodexCliProvider::new();
        let models = p.list_models();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id.contains("codex")));
    }

    #[test]
    fn parse_claude_json_success() {
        // Real-shape response from `claude -p "say hi" --output-format json`.
        let raw = r#"{
            "type":"result","subtype":"success","is_error":false,
            "result":"Hey! hi there.",
            "total_cost_usd":0.0477,
            "usage":{
                "input_tokens":10,
                "cache_creation_input_tokens":36382,
                "cache_read_input_tokens":0,
                "output_tokens":450
            }
        }"#;
        let c = parse_claude_json_response(raw).expect("parse ok");
        assert_eq!(c.text, "Hey! hi there.");
        let u = c.usage.expect("usage present");
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 450);
        assert_eq!(u.cache_creation_tokens, 36382);
        assert_eq!(u.cache_read_tokens, 0);
        assert!((u.total_cost_usd - 0.0477).abs() < 1e-9);
    }

    #[test]
    fn parse_claude_json_is_error_rejected() {
        let raw = r#"{"is_error":true,"result":"boom"}"#;
        assert!(parse_claude_json_response(raw).is_none());
    }

    #[test]
    fn parse_claude_json_missing_result_rejected() {
        let raw = r#"{"is_error":false,"usage":{"input_tokens":1}}"#;
        assert!(parse_claude_json_response(raw).is_none());
    }

    #[test]
    fn parse_claude_json_no_usage_block() {
        // Some response shapes may omit usage entirely; accept text and mark
        // usage as None so callers fall back to estimation.
        let raw = r#"{"is_error":false,"result":"ok"}"#;
        let c = parse_claude_json_response(raw).expect("parse ok");
        assert_eq!(c.text, "ok");
        assert!(c.usage.is_none());
    }

    #[test]
    fn default_config_is_claude() {
        let cfg = LlmConfig::default();
        assert_eq!(cfg.provider, "claude");
        assert_eq!(cfg.fast_model, "haiku");
        assert_eq!(cfg.standard_model, "sonnet");
    }
}
