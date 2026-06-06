//! Guard — real-time interception of critical Bash commands via Claude Code
//! synchronous `PreToolUse` hooks.
//!
//! When a Critical-risk command is detected, the `fleet guard` CLI subprocess
//! writes a request to `~/.fleet/guard/<uuid>.json` and polls for a response
//! file at `~/.fleet/guard/<uuid>.response.json`.  The Fleet desktop app
//! watches this directory, shows a dialog to the user, and writes the response.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditRiskLevel};

// ── Types ────────────────────────────────────────────────────────────────────

/// Decision the user makes in the guard dialog.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GuardDecision {
    Allow,
    Block,
}

/// Written by `fleet guard` → read by Fleet desktop app.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GuardRequest {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    /// AI-generated session title (distinct from workspace_name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    pub tool_name: String,
    pub command: String,
    pub command_summary: String,
    pub risk_tags: Vec<String>,
    pub timestamp: String,
    /// Structured view of `command` (shell AST flattened into a list of
    /// leaves + connectors) for UI rendering.  Optional: an older CLI may
    /// not populate it, in which case the front-end falls back to showing
    /// the raw `command` string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_command: Option<crate::cmd_ast::CommandView>,
}

/// Written by Fleet desktop app → read by `fleet guard`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GuardResponse {
    pub id: String,
    pub decision: GuardDecision,
    /// Optional human-typed reason captured in the Block input field; the
    /// `fleet guard` CLI forwards it back to Claude Code so the AI knows why
    /// the user refused. Older responses without this field deserialize as
    /// `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Side-channel payload attached to a guard response when the user clicks
/// "Always allow" — Fleet desktop / `fleet serve` writes a persisted allow
/// rule (see [`crate::audit::add_guard_allow_rule`]) and then writes the
/// usual `GuardResponse` so the CLI is unaware of the always-allow concept.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GuardAlwaysAllow {
    pub prefix: String,
    /// The audit tag (e.g. `eval-exec`) that triggered the guard; stored on
    /// the rule for UI / debugging only.
    #[serde(default)]
    pub source_tag: Option<String>,
}

/// HTTP / Tauri IPC envelope for `respond_to_guard` calls.  The legacy shape
/// `{ id, decision }` continues to deserialize because `always_allow` defaults
/// to `None`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GuardRespondPayload {
    pub id: String,
    pub decision: GuardDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_allow: Option<GuardAlwaysAllow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── Paths ────────────────────────────────────────────────────────────────────

pub fn guard_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("guard"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    guard_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    guard_dir().map(|d| d.join(format!("{id}.response.json")))
}

// ── File-based IPC ───────────────────────────────────────────────────────────

/// Write a guard request.  Called by `fleet guard` CLI.
pub fn write_request(req: &GuardRequest) -> Result<(), String> {
    let dir = guard_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create guard dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write request: {e}"))
}

/// Poll for a guard response.  Called by `fleet guard` CLI.
/// Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<GuardResponse> {
    let path = response_path(id)?;
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);

    loop {
        if start.elapsed() > timeout {
            return None;
        }
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(resp) = serde_json::from_str::<GuardResponse>(&content) {
                    return Some(resp);
                }
            }
        }
        std::thread::sleep(poll_interval);
    }
}

/// Non-blocking read of a guard response, if one exists yet.
pub fn try_read_response(id: &str) -> Option<GuardResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<GuardResponse>(&content).ok()
}

/// Write a guard response.  Called by the desktop app.
pub fn write_response(resp: &GuardResponse) -> Result<(), String> {
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write response: {e}"))
}

/// Clean up request + response files.
pub fn cleanup(id: &str) {
    if let Some(p) = request_path(id) {
        let _ = fs::remove_file(p);
    }
    if let Some(p) = response_path(id) {
        let _ = fs::remove_file(p);
    }
}

/// Read a pending request file.
pub fn read_request(id: &str) -> Option<GuardRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// List all pending request IDs in the guard directory. Soft form —
/// returns an empty vec on any failure. See [`list_pending_requests_checked`]
/// for the variant that distinguishes "no requests" from "couldn't read".
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict form — `Ok(vec![])` for "directory missing / no requests" but
/// `Err` for actual I/O errors. The directory watcher uses this so a
/// transient `read_dir` error doesn't get treated as "all requests vanished"
/// and dismiss every active panel.
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = guard_dir() else {
        return Ok(Vec::new());
    };
    list_pending_in_dir(&dir)
}

fn list_pending_in_dir(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    // Two-pass: drop any request whose partner `<id>.response.json` already
    // exists.  Orphan request files left behind by SIGKILL'd `fleet guard`
    // CLIs would otherwise re-surface as fresh panels on every desktop-app
    // launch — see the same logic in `elicitation::list_pending_in_dir`.
    let mut request_ids: Vec<String> = Vec::new();
    let mut response_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".response.json") {
            response_ids.insert(id.to_string());
        } else if let Some(id) = name.strip_suffix(".json") {
            request_ids.push(id.to_string());
        }
    }
    Ok(request_ids
        .into_iter()
        .filter(|id| !response_ids.contains(id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-guard-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn list_pending_in_dir_excludes_already_answered_request() {
        let dir = fresh_tmp_dir("answered");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("answered.json"), "{}").unwrap();
        fs::write(dir.join("answered.response.json"), "{}").unwrap();
        fs::write(dir.join("still-pending.json"), "{}").unwrap();

        let mut ids = list_pending_in_dir(&dir).unwrap();
        ids.sort();
        assert_eq!(ids, vec!["still-pending".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    // ── Guard allow-list short-circuit tests ────────────────────────────────

    fn bash_hook(command: &str) -> HookInput {
        HookInput {
            session_id: None,
            tool_name: Some("Bash".into()),
            tool_input: Some(serde_json::json!({ "command": command })),
        }
    }

    #[test]
    fn classify_short_circuits_when_allow_rule_matches() {
        // [REQ-035] DEC-017: a SIGNED allow rule may short-circuit even a
        // Critical command. `eval ` (trailing space) appears as a substring
        // inside `patchwright-cli eval "..."`, so the eval-exec builtin tags it
        // Critical — exactly the false-positive case the whitelist exists to
        // wave through for a trusted command. Non-silence is guaranteed by the
        // independent `audit::extract_audit_events` transcript trail, which
        // still records this command as an AuditEvent regardless of the rule.
        let mut rules = audit::UserAuditRules::default();
        let r = audit::upsert_guard_allow_rule_in(
            &mut rules,
            "patchwright-cli eval".into(),
            Some("eval-exec".into()),
        );
        // DEC-017: the rule only bites once a human signs it.
        audit::sign_guard_allow_rule_in(&mut rules, &r.id, "boss").unwrap();

        let input = bash_hook(r#"patchwright-cli eval "() => document.title""#);
        match classify_hook_input_with_rules(&input, &rules) {
            GuardClassification::Allow => {}
            GuardClassification::NeedsConfirmation { .. } => {
                panic!("expected signed allow-list rule to short-circuit Critical command")
            }
        }
    }

    #[test]
    fn classify_does_not_short_circuit_critical_for_unsigned_rule() {
        // [REQ-035] DEC-017: the signature is the require-approval teeth. An
        // UNSIGNED rule (just clicking "always allow", never approved) must NOT
        // short-circuit a Critical command — it still needs confirmation. This
        // is the symmetric counterpart to the test above.
        let mut rules = audit::UserAuditRules::default();
        audit::upsert_guard_allow_rule_in(
            &mut rules,
            "patchwright-cli eval".into(),
            Some("eval-exec".into()),
        );
        // Deliberately NOT signed.

        let input = bash_hook(r#"patchwright-cli eval "() => document.title""#);
        match classify_hook_input_with_rules(&input, &rules) {
            GuardClassification::NeedsConfirmation { risk_tags, .. } => {
                assert!(
                    risk_tags.contains(&"eval-exec".to_string()),
                    "unsigned rule must not bypass; eval-exec confirmation still required, got {risk_tags:?}"
                );
            }
            GuardClassification::Allow => {
                panic!("unsigned allow-list rule must NOT short-circuit a Critical command")
            }
        }
    }

    #[test]
    fn classify_still_prompts_for_non_matching_prefix() {
        // Same allow rule, different command — must NOT short-circuit (and
        // would normally be Allow because it's not Critical, but we verify
        // the rule didn't fire either).
        let mut rules = audit::UserAuditRules::default();
        audit::upsert_guard_allow_rule_in(&mut rules, "patchwright-cli eval".into(), None);

        // `sudo rm -rf /` is Critical and the prefix doesn't match — must
        // still require confirmation.
        let input = bash_hook("sudo rm -rf /tmp/foo");
        match classify_hook_input_with_rules(&input, &rules) {
            GuardClassification::NeedsConfirmation { risk_tags, .. } => {
                assert!(risk_tags.contains(&"sudo".to_string()));
            }
            GuardClassification::Allow => panic!("expected NeedsConfirmation for unrelated sudo"),
        }
    }

    #[test]
    fn classify_no_rules_still_critical_needs_confirmation() {
        let rules = audit::UserAuditRules::default();
        let input = bash_hook("sudo rm -rf /tmp/foo");
        match classify_hook_input_with_rules(&input, &rules) {
            GuardClassification::NeedsConfirmation { .. } => {}
            GuardClassification::Allow => panic!("sudo must require confirmation by default"),
        }
    }

    #[test]
    fn guard_response_legacy_json_without_reason_still_parses() {
        // Older Fleet desktop versions wrote responses without the `reason`
        // field. The newly-added optional field must default to None so
        // running CLIs/servers don't break on rollback.
        let legacy = r#"{"id":"abc","decision":"block"}"#;
        let parsed: GuardResponse = serde_json::from_str(legacy).expect("legacy parse");
        assert_eq!(parsed.id, "abc");
        assert!(matches!(parsed.decision, GuardDecision::Block));
        assert_eq!(parsed.reason, None);
    }

    #[test]
    fn guard_response_round_trips_reason_in_camel_case() {
        let resp = GuardResponse {
            id: "xyz".into(),
            decision: GuardDecision::Block,
            reason: Some("dangerous on prod".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"reason\":\"dangerous on prod\""));
        let parsed: GuardResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.reason.as_deref(), Some("dangerous on prod"));
    }

    #[test]
    fn guard_respond_payload_legacy_json_without_reason_still_parses() {
        let legacy = r#"{"id":"abc","decision":"allow"}"#;
        let parsed: GuardRespondPayload = serde_json::from_str(legacy).expect("legacy parse");
        assert!(matches!(parsed.decision, GuardDecision::Allow));
        assert_eq!(parsed.reason, None);
        assert!(parsed.always_allow.is_none());
    }
}

// ── Guard logic (used by `fleet guard` CLI) ─────────────────────────────────

/// Parsed hook input from Claude Code PreToolUse.
#[derive(Deserialize, Debug)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
}

/// Result of classifying the hook input.
pub enum GuardClassification {
    /// Not a Bash command, or not Critical — allow silently.
    Allow,
    /// Critical risk — needs user confirmation.
    NeedsConfirmation {
        command: String,
        risk_tags: Vec<String>,
    },
}

/// Classify a hook input.  Returns whether this needs user confirmation.
///
/// Before requesting confirmation for a Critical command, this checks the
/// user's persisted "always allow" rule list (see
/// [`audit::match_guard_allow_rule`]) and short-circuits to `Allow` when a
/// rule's `prefix` matches the command.
pub fn classify_hook_input(input: &HookInput) -> GuardClassification {
    let rules = audit::load_user_rules();
    classify_hook_input_with_rules(input, &rules)
}

/// Test-friendly variant of [`classify_hook_input`] that takes the user-rules
/// snapshot as a parameter instead of reading it from disk.
pub fn classify_hook_input_with_rules(
    input: &HookInput,
    user_rules: &audit::UserAuditRules,
) -> GuardClassification {
    let tool_name = input.tool_name.as_deref().unwrap_or("");
    if tool_name != "Bash" {
        return GuardClassification::Allow;
    }

    let command = input
        .tool_input
        .as_ref()
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    if command.is_empty() {
        return GuardClassification::Allow;
    }

    match audit::classify_bash_command_pub(command) {
        Some((AuditRiskLevel::Critical, tags)) => {
            if let Some(rule) = audit::match_guard_allow_rule_in(user_rules, command) {
                eprintln!(
                    "fleet guard: allowed by user rule {} (prefix={:?})",
                    rule.id, rule.prefix
                );
                return GuardClassification::Allow;
            }
            GuardClassification::NeedsConfirmation {
                command: command.to_string(),
                risk_tags: tags,
            }
        }
        _ => GuardClassification::Allow,
    }
}

/// Generate a new unique guard request ID.
pub fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}

/// Truncate a command for display.
pub fn truncate_command(cmd: &str, max: usize) -> String {
    if cmd.len() <= max {
        cmd.to_string()
    } else {
        format!("{}…", &cmd[..max])
    }
}

// ── LLM analysis prompt ─────────────────────────────────────────────────────

/// Build a prompt for the LLM to analyze a guarded command.
pub fn build_analysis_prompt(
    command: &str,
    risk_tags: &[String],
    context_message: &str,
    lang: &str,
) -> String {
    let lang_instruction = match lang {
        "zh" => "请用中文回答。",
        _ => "Answer in English.",
    };

    format!(
        r#"You are a security analyst reviewing a command about to be executed by an AI coding agent.

Context (the agent's last message before this tool call):
{context_message}

Command to be executed:
```
{command}
```

Risk tags: {tags}

{lang_instruction}
In 2-3 concise sentences:
1. What this command is doing
2. What the specific security risk is
3. Whether this seems intentional given the context (false positive?)"#,
        context_message = if context_message.is_empty() {
            "(no context available)"
        } else {
            context_message
        },
        command = command,
        tags = risk_tags.join(", "),
        lang_instruction = lang_instruction,
    )
}
