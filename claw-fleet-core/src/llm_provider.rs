//! LLM provider abstraction — trait + CLI implementations for Claude and Codex.
//!
//! Each provider wraps its respective CLI tool for non-interactive text completion.
//! The trait is used by `claude_analyze` and `daily_report` modules so that any
//! supported CLI can power session analysis, report summaries, and lesson extraction.

use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::log_debug;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LlmModel {
    pub id: String,
    pub display_name: String,
    /// Short display name of the cross-engine sibling at the same tier
    /// (e.g. Claude "Haiku" pairs with Codex "Luna"). Populated by
    /// `all_provider_infos`; the UI shows it as a `Haiku / Luna` suffix when
    /// both engines are active so the tier alignment is visible. `None` when
    /// there is no other engine to align against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aligned_display: Option<String>,
}

impl LlmModel {
    fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self { id: id.into(), display_name: display_name.into(), aligned_display: None }
    }
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
    /// Preferred provider for daily-report AI work. Reports automatically use
    /// the other monitored provider when this one is rate-limited. `None`
    /// preserves legacy configs by inheriting `provider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_report_preference: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "claude".into(),
            fast_model: "haiku".into(),
            standard_model: "sonnet".into(),
            daily_report_preference: Some("claude".into()),
        }
    }
}

impl LlmConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else { return Self::default() };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or_else(|| "cannot determine home dir".to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn effective_daily_report_preference(&self) -> &str {
        self.daily_report_preference
            .as_deref()
            .filter(|name| matches!(*name, "claude" | "codex"))
            .or_else(|| matches!(self.provider.as_str(), "claude" | "codex").then_some(self.provider.as_str()))
            .unwrap_or("claude")
    }

    pub fn standard_model_for(&self, provider: &dyn LlmProvider) -> String {
        if self.provider == provider.name() && !self.standard_model.is_empty() {
            self.standard_model.clone()
        } else {
            provider.default_standard_model().to_string()
        }
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    crate::session::real_home_dir().map(|home| home.join(".fleet").join("llm-config.json"))
}

/// Process-wide current config, for callers with no handle on the owning
/// state (the mobile relay's `guard_analyze`). The desktop backend and the
/// `fleet serve` `/llm/config` handler mirror their updates here; readers fall
/// back to `LlmConfig::default()` until the first update — the same effective
/// behavior both hosts already have at boot.
static SHARED_CONFIG: std::sync::Mutex<Option<LlmConfig>> = std::sync::Mutex::new(None);

pub fn set_shared_config(cfg: LlmConfig) {
    *SHARED_CONFIG.lock().unwrap() = Some(cfg);
}

pub fn shared_config() -> LlmConfig {
    SHARED_CONFIG.lock().unwrap().clone().unwrap_or_default()
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
    /// Short identifier: "claude", "codex".
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
        let _ = crate::process_util::command("taskkill")
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

    if let Ok(output) = crate::process_util::command(which).arg(name).output() {
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
    envs: &[(String, String)],
    timeout: Duration,
    tag: &str,
) -> Option<String> {
    // Only log the binary and flag names, not prompt content (which can be huge).
    let safe_args: Vec<&str> = args.iter().map(|a| {
        if a.len() > 80 { "<prompt…>" } else { a }
    }).collect();
    log_debug(&format!("[{tag}] spawning: {bin} {}", safe_args.join(" ")));
    let mut cmd = crate::process_util::command(bin);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let child = match cmd
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
            LlmModel::new("fable", "Fable"),
            LlmModel::new("opus", "Opus"),
            LlmModel::new("sonnet", "Sonnet"),
            LlmModel::new("haiku", "Haiku"),
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
            &[],
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

/// Build a minimal CODEX_HOME at `clean` that carries the credentials from
/// `source` but none of its guidance: `codex exec` unconditionally loads
/// `$CODEX_HOME/AGENTS.md`, which on Fleet machines holds the injected
/// interaction/PRD rules ("address the user as Boss", end turns with a
/// decision card). Those bled into pure text generation — the 2026-07-16
/// daily-report summary came out in decision-card voice. There is no CLI flag
/// to skip the file (`-c project_doc_max_bytes=0` does not affect it; verified
/// live), so isolation via a stripped-down home is the only lever.
///
/// Links `auth.json` (required) and `models_cache.json` (optional) and removes
/// any stray `AGENTS.md`/`config.toml`, so a stale copy can never resurrect
/// the guidance. Idempotent; call before every spawn.
fn prepare_clean_codex_home(source: &std::path::Path, clean: &std::path::Path) -> Result<(), String> {
    if !source.join("auth.json").exists() {
        return Err(format!("no auth.json under {}", source.display()));
    }
    std::fs::create_dir_all(clean).map_err(|e| format!("create {}: {e}", clean.display()))?;
    let remove_entry = |p: &std::path::Path| -> Result<(), String> {
        if p.symlink_metadata().is_ok() {
            std::fs::remove_file(p).map_err(|e| format!("remove {}: {e}", p.display()))?;
        }
        Ok(())
    };
    for stray in ["AGENTS.md", "config.toml"] {
        remove_entry(&clean.join(stray))?;
    }
    for name in ["auth.json", "models_cache.json"] {
        let src = source.join(name);
        if !src.exists() {
            continue;
        }
        let dst = clean.join(name);
        // Re-link fresh every call: if a codex run replaced the symlink with a
        // regular file (e.g. an atomic token-refresh rewrite), heal it here.
        remove_entry(&dst)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &dst).map_err(|e| format!("link {}: {e}", dst.display()))?;
        // Symlinks need elevated privileges on Windows; a per-call copy stays
        // fresh enough because we rebuild it before every spawn.
        #[cfg(not(unix))]
        std::fs::copy(&src, &dst).map(|_| ()).map_err(|e| format!("copy {}: {e}", dst.display()))?;
    }
    Ok(())
}

/// CODEX_HOME override pointing at the clean home, or empty (= use the real
/// home, guidance bleed and all) when it cannot be prepared — a degraded run
/// beats no run.
fn clean_codex_home_env() -> Vec<(String, String)> {
    let Some(home) = crate::session::real_home_dir() else { return Vec::new() };
    let source = home.join(".codex");
    let clean = home.join(".fleet").join("codex-clean-home");
    match prepare_clean_codex_home(&source, &clean) {
        Ok(()) => vec![("CODEX_HOME".into(), clean.to_string_lossy().into_owned())],
        Err(e) => {
            log_debug(&format!("[llm:codex] clean CODEX_HOME unavailable ({e}), using default home"));
            Vec::new()
        }
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
        // Fallback when the cache is missing. Keep these to models a current
        // ChatGPT-account login can actually run — the older `*-codex` /
        // `*-codex-mini` slugs 400 with "not supported when using Codex with a
        // ChatGPT account" (verified live).
        vec![
            LlmModel::new("gpt-5.6-sol", "GPT-5.6-Sol"),
            LlmModel::new("gpt-5.6-terra", "GPT-5.6-Terra"),
            LlmModel::new("gpt-5.6-luna", "GPT-5.6-Luna"),
        ]
    }

    // Luna is Codex's fast/cheap tier; Terra the balanced one. The previous
    // defaults (`gpt-5.1-codex-mini` / `gpt-5.3-codex`) 400 under a ChatGPT
    // account, so any scenario using Codex as its analysis provider failed.
    fn default_fast_model(&self) -> &str { "gpt-5.6-luna" }
    fn default_standard_model(&self) -> &str { "gpt-5.6-terra" }

    fn complete(&self, prompt: &str, model: &str, timeout: Duration) -> Option<Completion> {
        let bin = self.bin_path.as_deref()?;
        // exec: non-interactive mode (stdout = final message only)
        // --ephemeral: don't persist session
        // --full-auto: auto-approve (no interactive prompts)
        // --skip-git-repo-check: we're not in a repo context
        // --sandbox read-only: prevent file writes (pure text generation)
        // CODEX_HOME=<clean home>: keep the global AGENTS.md guidance out of
        // pure text generation (see prepare_clean_codex_home).
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
            &clean_codex_home_env(),
            timeout,
            "llm:codex",
        )?;
        Some(Completion { text, usage: None })
    }
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
            Some(LlmModel::new(slug, display))
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
        _ => None,
    }
}

/// Return info snapshots for all known providers, including a "none" (disabled)
/// option at the end.
pub fn all_provider_infos() -> Vec<LlmProviderInfo> {
    let providers: Vec<Box<dyn LlmProvider>> = vec![
        Box::new(ClaudeCliProvider::new()),
        Box::new(CodexCliProvider::new()),
    ];

    let mut infos: Vec<LlmProviderInfo> = providers
        .into_iter()
        .map(|p| {
            let name = p.name().to_string();
            let models = p
                .list_models()
                .into_iter()
                .map(|m| LlmModel { aligned_display: aligned_tier_label(&name, &m.id), ..m })
                .collect();
            LlmProviderInfo {
                display_name: p.display_name().into(),
                available: p.is_available(),
                models,
                default_fast_model: p.default_fast_model().into(),
                default_standard_model: p.default_standard_model().into(),
                name,
            }
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

// ── Quota-aware provider routing ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaState { Healthy, Limited, Unknown }

pub struct LlmRoute {
    pub provider: Box<dyn LlmProvider>,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSlot { Fast, Standard }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelTier { Fast, Standard, Premium }

fn model_tier(model: &str, fallback: ModelSlot) -> ModelTier {
    let model = model.to_ascii_lowercase();
    if model.contains("haiku") || model.contains("luna") {
        ModelTier::Fast
    } else if model.contains("opus") || model.contains("fable") || model.contains("sol") {
        ModelTier::Premium
    } else if model.contains("sonnet") || model.contains("terra") {
        ModelTier::Standard
    } else {
        match fallback { ModelSlot::Fast => ModelTier::Fast, ModelSlot::Standard => ModelTier::Standard }
    }
}

fn equivalent_model(target_provider: &str, selected_model: &str, slot: ModelSlot) -> String {
    match (target_provider, model_tier(selected_model, slot)) {
        ("claude", ModelTier::Fast) => "haiku",
        ("claude", ModelTier::Standard) => "sonnet",
        ("claude", ModelTier::Premium) => "opus",
        ("codex", ModelTier::Fast) => "gpt-5.6-luna",
        ("codex", ModelTier::Standard) => "gpt-5.6-terra",
        ("codex", ModelTier::Premium) => "gpt-5.6-sol",
        _ => selected_model,
    }.to_string()
}

/// Compact tier label for a model id, for the cross-engine `Haiku / Luna`
/// pairing shown in settings. Codex slugs (`gpt-5.6-luna`) collapse to their
/// tier word (`Luna`); Claude ids (`haiku`) title-case to their display name
/// (`Haiku`). Deliberately not the full Codex `displayName` ("GPT-5.6-Luna"),
/// which would be too verbose as a suffix.
fn tier_label(id: &str) -> String {
    let base = id.strip_prefix("gpt-5.6-").unwrap_or(id);
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Label of the same-tier model in the *other* engine, or `None` when the
/// provider has no counterpart to align against. Uses `equivalent_model` as the
/// single source of truth for which sibling maps to which tier.
fn aligned_tier_label(provider: &str, id: &str) -> Option<String> {
    let other = match provider {
        "claude" => "codex",
        "codex" => "claude",
        _ => return None,
    };
    Some(tier_label(&equivalent_model(other, id, ModelSlot::Standard)))
}

fn rank_providers(preference: &str, states: &[(String, QuotaState)]) -> Vec<String> {
    let mut usable: Vec<&(String, QuotaState)> = states.iter()
        .filter(|(_, state)| *state != QuotaState::Limited)
        .collect();
    usable.sort_by_key(|(name, state)| {
        (usize::from(name != preference), usize::from(*state == QuotaState::Unknown))
    });
    usable.into_iter().map(|(name, _)| name.clone()).collect()
}

fn claude_quota_state() -> QuotaState {
    const MAX_SNAPSHOT_AGE_MS: i64 = 20 * 60 * 1000;
    let Some(snapshot) = crate::account::latest_usage_snapshot() else { return QuotaState::Unknown };
    if chrono::Utc::now().timestamp_millis() - snapshot.ts > MAX_SNAPSHOT_AGE_MS {
        return QuotaState::Unknown;
    }
    let values = [snapshot.five_hour, snapshot.seven_day, snapshot.seven_day_sonnet];
    if values.iter().flatten().any(|value| *value >= 0.999) {
        QuotaState::Limited
    } else if values.iter().any(Option::is_some) {
        QuotaState::Healthy
    } else {
        QuotaState::Unknown
    }
}

fn codex_quota_state() -> QuotaState {
    match crate::codex_source::fetch_codex_usage_blocking() {
        Ok(usage) if usage.rate_limit_reached_type.is_some() => QuotaState::Limited,
        Ok(_) => QuotaState::Healthy,
        Err(e) => {
            log_debug(&format!("[daily_report] codex quota probe unavailable: {e}"));
            QuotaState::Unknown
        }
    }
}

static QUOTA_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, (Instant, QuotaState)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn provider_quota_state(name: &str) -> QuotaState {
    const TTL: Duration = Duration::from_secs(60);
    if let Some((checked_at, state)) = QUOTA_CACHE.lock().unwrap().get(name).copied() {
        if checked_at.elapsed() < TTL { return state }
    }
    let state = match name {
        "claude" => claude_quota_state(),
        "codex" => codex_quota_state(),
        _ => QuotaState::Unknown,
    };
    QUOTA_CACHE.lock().unwrap().insert(name.to_string(), (Instant::now(), state));
    state
}

/// Resolve monitored providers in preference order, omitting accounts known to
/// be rate-limited and translating the selected model onto the equivalent tier
/// of the fallback provider.
pub fn provider_routes(config: &LlmConfig, slot: ModelSlot, preference: &str) -> Vec<LlmRoute> {
    if config.provider == "none" { return Vec::new() }
    let sources = crate::agent_source::SourcesConfig::load();
    let mut providers = Vec::<(String, Box<dyn LlmProvider>)>::new();
    for name in ["claude", "codex"] {
        if !sources.is_source_enabled(name) { continue }
        let Some(provider) = resolve_provider(name) else { continue };
        if provider.is_available() { providers.push((name.to_string(), provider)); }
    }
    // Probe the preferred account first. If it is usable, the fallback's
    // quota does not affect this call and remains lazy (actual CLI failure
    // still falls through). Only probe the fallback eagerly when preference
    // is known-limited, avoiding a Codex app-server round trip on every first
    // Claude-preferred Guard/session-analysis call.
    let primary = providers.iter()
        .find(|(name, _)| name == preference)
        .or_else(|| providers.first())
        .map(|(name, _)| name.clone());
    let primary_state = primary.as_deref().map(provider_quota_state).unwrap_or(QuotaState::Unknown);
    let states: Vec<(String, QuotaState)> = providers.iter().map(|(name, _)| {
        let state = if Some(name.as_str()) == primary.as_deref() {
            primary_state
        } else if primary_state == QuotaState::Limited {
            provider_quota_state(name)
        } else {
            QuotaState::Unknown
        };
        (name.clone(), state)
    }).collect();
    let order = rank_providers(preference, &states);
    let selected_model = match slot {
        ModelSlot::Fast => &config.fast_model,
        ModelSlot::Standard => &config.standard_model,
    };
    order.into_iter().filter_map(|name| {
        let index = providers.iter().position(|(candidate, _)| candidate == &name)?;
        let (_, provider) = providers.swap_remove(index);
        let model = if name == config.provider {
            selected_model.to_string()
        } else {
            equivalent_model(&name, selected_model, slot)
        };
        Some(LlmRoute { provider, model })
    }).collect()
}

pub fn daily_report_routes(config: &LlmConfig) -> Vec<LlmRoute> {
    provider_routes(config, ModelSlot::Standard, config.effective_daily_report_preference())
}

pub fn complete_routed(
    config: &LlmConfig,
    slot: ModelSlot,
    prompt: &str,
    timeout: Duration,
    scenario: &str,
) -> Option<String> {
    for route in provider_routes(config, slot, &config.provider) {
        log_debug(&format!("[llm:route] scenario={scenario} provider={} model={}", route.provider.name(), route.model));
        if let Some(text) = crate::llm_usage::complete_accounted(
            route.provider.as_ref(), prompt, &route.model, timeout, scenario,
        ) {
            return Some(text);
        }
    }
    None
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_unknown_provider_returns_none() {
        assert!(resolve_provider("unknown").is_none());
    }

    #[test]
    fn daily_report_routing_prefers_choice_and_skips_limited() {
        let healthy = vec![("claude".into(), QuotaState::Healthy), ("codex".into(), QuotaState::Healthy)];
        assert_eq!(rank_providers("codex", &healthy), ["codex", "claude"]);
        let limited = vec![("claude".into(), QuotaState::Limited), ("codex".into(), QuotaState::Healthy)];
        assert_eq!(rank_providers("claude", &limited), ["codex"]);
    }

    #[test]
    fn equivalent_models_preserve_capability_tier() {
        assert_eq!(equivalent_model("codex", "haiku", ModelSlot::Fast), "gpt-5.6-luna");
        assert_eq!(equivalent_model("codex", "sonnet", ModelSlot::Standard), "gpt-5.6-terra");
        assert_eq!(equivalent_model("codex", "opus", ModelSlot::Standard), "gpt-5.6-sol");
        assert_eq!(equivalent_model("claude", "gpt-5.6-luna", ModelSlot::Fast), "haiku");
        assert_eq!(equivalent_model("claude", "gpt-5.6-terra", ModelSlot::Standard), "sonnet");
        assert_eq!(equivalent_model("claude", "gpt-5.6-sol", ModelSlot::Standard), "opus");
    }

    #[test]
    fn provider_infos_pair_cross_engine_tier_labels() {
        let infos = all_provider_infos();
        let claude = infos.iter().find(|p| p.name == "claude").expect("claude provider");
        let label = |id: &str| {
            claude.models.iter().find(|m| m.id == id)
                .unwrap_or_else(|| panic!("model {id}")).aligned_display.clone()
        };
        assert_eq!(label("haiku"), Some("Luna".into()));
        assert_eq!(label("sonnet"), Some("Terra".into()));
        assert_eq!(label("opus"), Some("Sol".into()));
        assert_eq!(label("fable"), Some("Sol".into()));

        let codex = infos.iter().find(|p| p.name == "codex").expect("codex provider");
        // Codex side always aligns back to one of Claude's short tier names.
        // (Robust to whether list_models came from the local cache or the
        // hardcoded fallback.)
        for m in &codex.models {
            let aligned = m.aligned_display.as_deref();
            assert!(
                matches!(aligned, Some("Haiku" | "Sonnet" | "Opus")),
                "codex model {} aligned to {:?}", m.id, aligned,
            );
        }

        // The "none" provider has no counterpart, so nothing to align.
        let none = infos.iter().find(|p| p.name == "none").expect("none provider");
        assert!(none.models.is_empty());
    }

    #[test]
    fn legacy_config_inherits_general_provider_for_report_preference() {
        let cfg = LlmConfig { provider: "codex".into(), fast_model: "fast".into(), standard_model: "standard".into(), daily_report_preference: None };
        assert_eq!(cfg.effective_daily_report_preference(), "codex");
    }

    #[test]
    fn claude_provider_lists_models() {
        let p = ClaudeCliProvider::new();
        let models = p.list_models();
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].id, "fable");
        assert!(models.iter().any(|m| m.id == "opus"));
    }

    #[test]
    fn codex_provider_lists_models() {
        let p = CodexCliProvider::new();
        let models = p.list_models();
        assert!(!models.is_empty());
        // Codex lists the GPT-5 family (sol/terra/luna, or whatever the CLI
        // cached at login). The older `*-codex` slugs were dropped because they
        // 400 under a ChatGPT account, so assert the current contract instead.
        assert!(models.iter().any(|m| m.id.starts_with("gpt-5")));
    }

    #[test]
    fn clean_codex_home_links_credentials_and_drops_guidance() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("codex");
        let clean = tmp.path().join("clean");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("auth.json"), "{}").unwrap();
        std::fs::write(source.join("models_cache.json"), "[]").unwrap();
        std::fs::write(source.join("AGENTS.md"), "# guidance").unwrap();
        std::fs::write(source.join("config.toml"), "x = 1").unwrap();

        prepare_clean_codex_home(&source, &clean).unwrap();

        // Credentials reach the clean home (as links to the live files)…
        assert_eq!(std::fs::read_to_string(clean.join("auth.json")).unwrap(), "{}");
        assert_eq!(std::fs::read_to_string(clean.join("models_cache.json")).unwrap(), "[]");
        // …but guidance and config must not.
        assert!(!clean.join("AGENTS.md").exists());
        assert!(!clean.join("config.toml").exists());

        // Idempotent: a second run over the existing clean home succeeds.
        prepare_clean_codex_home(&source, &clean).unwrap();
        assert_eq!(std::fs::read_to_string(clean.join("auth.json")).unwrap(), "{}");
    }

    /// Live probe through the real `complete()` path: with the clean
    /// CODEX_HOME in place, codex must no longer see the global AGENTS.md
    /// guidance. Needs a logged-in codex CLI; run explicitly with
    /// `cargo test -p claw-fleet-core codex_complete_isolated -- --ignored`.
    #[test]
    #[ignore]
    fn codex_complete_isolated_from_global_agents_md() {
        let p = CodexCliProvider::new();
        assert!(p.is_available(), "codex CLI not installed");
        let reply = p
            .complete(
                "In any AGENTS.md/project-doc content you were given, find the first \
                 line containing the word Fleet and quote it verbatim. If no such \
                 line exists reply exactly NONE.",
                p.default_fast_model(),
                Duration::from_secs(120),
            )
            .expect("codex completion failed");
        assert!(
            reply.text.contains("NONE"),
            "global AGENTS.md guidance leaked into the codex run: {}",
            reply.text
        );
    }

    #[test]
    fn clean_codex_home_requires_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("codex");
        std::fs::create_dir_all(&source).unwrap(); // no auth.json
        assert!(prepare_clean_codex_home(&source, &tmp.path().join("clean")).is_err());
    }

    #[test]
    fn clean_codex_home_heals_stale_regular_files() {
        // If codex ever rewrote auth.json through the link (replacing the
        // symlink with a regular file), the next run must re-link to source.
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("codex");
        let clean = tmp.path().join("clean");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&clean).unwrap();
        std::fs::write(source.join("auth.json"), "fresh").unwrap();
        std::fs::write(clean.join("auth.json"), "stale").unwrap();
        std::fs::write(clean.join("AGENTS.md"), "stale guidance").unwrap();

        prepare_clean_codex_home(&source, &clean).unwrap();

        assert_eq!(std::fs::read_to_string(clean.join("auth.json")).unwrap(), "fresh");
        assert!(!clean.join("AGENTS.md").exists());
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
