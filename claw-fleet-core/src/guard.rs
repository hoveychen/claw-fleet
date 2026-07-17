//! Guard — real-time interception of critical Bash commands via Claude Code
//! synchronous `PreToolUse` hooks.
//!
//! When a Critical-risk command is detected, the `fleet guard` CLI subprocess
//! writes a request to `~/.fleet/guard/<uuid>.json` and polls for a response
//! file at `~/.fleet/guard/<uuid>.response.json`.  The Fleet desktop app
//! watches this directory, shows a dialog to the user, and writes the response.

use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    // Only a live pending request may be answered. A missing request file means
    // an unknown/already-consumed id; report it as such so the HTTP layer can
    // return 404 instead of a raw 500 (the old `fs::write` ENOENT when the
    // channel dir never existed). A real request's dir always exists, so this
    // never rejects a valid response.
    if !request_path(&resp.id).map(|p| p.exists()).unwrap_or(false) {
        return Err(format!("no pending request for id {}", resp.id));
    }
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
            transcript_path: None,
            tool_name: Some("Bash".into()),
            tool_input: Some(serde_json::json!({ "command": command })),
        }
    }

    #[test]
    fn classify_short_circuits_when_allow_rule_matches() {
        // An allow rule short-circuits even a Critical command. `eval ` (trailing
        // space) appears as a substring inside `patchwright-cli eval "..."`, so
        // the eval-exec builtin tags it Critical — exactly the false-positive
        // case the whitelist exists to wave through for a trusted command. There
        // is no signature gate: the rule is live as soon as it's in the list
        // (clicking "always allow" IS the approval). Non-silence is guaranteed by
        // the independent `audit::extract_audit_events` transcript trail, which
        // still records this command as an AuditEvent regardless of the rule.
        let mut rules = audit::UserAuditRules::default();
        audit::upsert_guard_allow_rule_in(
            &mut rules,
            "patchwright-cli eval".into(),
            Some("eval-exec".into()),
        );

        let input = bash_hook(r#"patchwright-cli eval "() => document.title""#);
        match classify_hook_input_with_rules(&input, &rules) {
            GuardClassification::Allow => {}
            GuardClassification::NeedsConfirmation { .. } => {
                panic!("expected an allow-list rule to short-circuit a Critical command")
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

    #[test]
    fn truncate_command_does_not_split_multibyte_chars() {
        // Each '中' is 3 bytes; with max=10 the byte index 10 lands *inside*
        // the 4th character (bytes 9..12), so a naive `&cmd[..10]` slice panics
        // with "byte index 10 is not a char boundary". This reproduces the
        // crash seen in get_guard_context (gui.rs) on long CJK assistant text.
        let cmd = "中".repeat(20); // 60 bytes
        let out = truncate_command(&cmd, 10);
        // Round down to the previous char boundary (byte 9 = 3 whole chars),
        // then append the ellipsis — never panic.
        assert_eq!(out, "中中中…");
        // ASCII / short inputs are unaffected.
        assert_eq!(truncate_command("ls -la", 100), "ls -la");
    }

    // ── Missing-description reminder ─────────────────────────────────────────

    fn bash_hook_full(command: &str, description: Option<&str>) -> HookInput {
        let mut input = serde_json::json!({ "command": command });
        if let Some(d) = description {
            input["description"] = serde_json::Value::String(d.to_string());
        }
        HookInput {
            session_id: Some("sess-1".into()),
            transcript_path: None,
            tool_name: Some("Bash".into()),
            tool_input: Some(input),
        }
    }

    #[test]
    fn bash_missing_description_detects_absent_and_blank() {
        // Absent key.
        assert!(bash_missing_description(&bash_hook_full("ls", None)));
        // Present but blank / whitespace-only.
        assert!(bash_missing_description(&bash_hook_full("ls", Some(""))));
        assert!(bash_missing_description(&bash_hook_full("ls", Some("   "))));
        // Present and meaningful → not missing.
        assert!(!bash_missing_description(&bash_hook_full("ls", Some("List files"))));
    }

    #[test]
    fn bash_missing_description_ignores_non_bash() {
        let input = HookInput {
            session_id: Some("s".into()),
            transcript_path: None,
            tool_name: Some("Read".into()),
            tool_input: Some(serde_json::json!({ "file_path": "/x" })),
        };
        assert!(!bash_missing_description(&input));
    }

    #[test]
    fn claim_first_reminder_fires_once_per_session() {
        let dir = fresh_tmp_dir("desc-once");
        // First call for a session wins; the second (same id) is suppressed.
        assert!(claim_first_reminder_in(&dir, "session-A"));
        assert!(!claim_first_reminder_in(&dir, "session-A"));
        // A different session id is independent.
        assert!(claim_first_reminder_in(&dir, "session-B"));
        // Empty session id is always suppressed (can't throttle reliably).
        assert!(!claim_first_reminder_in(&dir, ""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reminder_output_shape_is_allow_plus_context() {
        let out = build_reminder_output();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
        assert!(v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("description"));
    }

    // ── Missing Codex exec-note reminder ───────────────────────────────────

    fn write_rollout(path: &std::path::Path, exec_script: Option<&str>) {
        let mut lines = vec![serde_json::json!({
            "type": "session_meta",
            "payload": { "id": "sess-codex", "originator": "fleet" }
        })];
        if let Some(script) = exec_script {
            lines.push(serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "name": "exec",
                    "input": script
                }
            }));
        }
        let body = lines
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).unwrap();
    }

    #[test]
    fn latest_codex_exec_detects_missing_note_only() {
        let dir = fresh_tmp_dir("exec-note-detect");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("rollout.jsonl");

        write_rollout(&transcript, Some("const r = await tools.exec_command({});"));
        assert!(latest_codex_exec_missing_note(&transcript));

        write_rollout(
            &transcript,
            Some("// 检查工作区状态。\nconst r = await tools.exec_command({});"),
        );
        assert!(!latest_codex_exec_missing_note(&transcript));

        write_rollout(&transcript, None);
        assert!(!latest_codex_exec_missing_note(&transcript));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn exec_note_reminder_fires_once_per_session() {
        let dir = fresh_tmp_dir("exec-note-once");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("rollout.jsonl");
        let markers = dir.join("markers");
        write_rollout(&transcript, Some("const r = await tools.exec_command({});"));

        let first = missing_exec_note_reminder_output_in(
            &transcript,
            "session-A",
            &markers,
        )
        .expect("first missing note should inject context");
        let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
        let context = parsed["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("FIRST line"));
        assert!(context.contains("// "));

        assert!(
            missing_exec_note_reminder_output_in(&transcript, "session-A", &markers).is_none(),
            "same session must be reminded only once"
        );
        assert!(
            missing_exec_note_reminder_output_in(&transcript, "session-B", &markers).is_some(),
            "different sessions are independent"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

// ── Guard logic (used by `fleet guard` CLI) ─────────────────────────────────

/// Parsed hook input from Claude Code PreToolUse.
#[derive(Deserialize, Debug)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
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

// ── Missing-description reminder ─────────────────────────────────────────────
//
// Fleet's transcript UI renders a Bash call's `description` as the command's
// intent (`formatInput` in ToolUseBlock.tsx); without it the card can only show
// the raw command. Empirically a session either fills `description` on *every*
// Bash call or omits it on *all* of them (measured: 22/22, 59/59 — never mixed),
// so a single nudge on the first offender is enough to break the pattern for the
// rest of the session. This rides the existing `fleet guard` PreToolUse hook, so
// no new hook wiring is needed.

/// One-time context injected when a Bash call omits its `description`.
pub const MISSING_DESCRIPTION_REMINDER: &str = "Fleet: that Bash call omitted the `description` parameter. Please add a short one-line `description` to every Bash tool call (e.g. \"Find files referencing X\", \"Run unit tests\") — Fleet's transcript renders it as the command's intent, and without it the card shows only the raw command. This note fires once per session.";

/// True when `input` is a Bash call whose `description` is missing or blank.
/// Pure — no side effects.
pub fn bash_missing_description(input: &HookInput) -> bool {
    if input.tool_name.as_deref() != Some("Bash") {
        return false;
    }
    let desc = input
        .tool_input
        .as_ref()
        .and_then(|v| v.get("description"))
        .and_then(|d| d.as_str())
        .unwrap_or("");
    desc.trim().is_empty()
}

/// Per-session marker directory: `~/.fleet/bash-desc-reminded/`.
fn desc_reminder_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("bash-desc-reminded"))
}

/// Record that `session_id` has been reminded, returning `true` only the first
/// time so the caller emits the reminder exactly once per session.
///
/// Uses an atomic `create_new` so concurrent Bash hooks in the same session race
/// safely — exactly one wins. Returns `false` (suppress) on an empty session id
/// or any filesystem error: better silent than spammed.
fn claim_first_reminder_in(dir: &std::path::Path, session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    // Session ids are uuids, but sanitize defensively so the id can never escape
    // the marker directory.
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(safe))
        .is_ok()
}

/// The exact stdout `fleet guard` prints to inject the reminder — the documented
/// PreToolUse `allow` + `additionalContext` shape (both fields are independent;
/// `allow` matches the non-critical classification this call already got).
fn build_context_reminder_output(context: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "additionalContext": context,
        }
    })
    .to_string()
}

fn build_reminder_output() -> String {
    build_context_reminder_output(MISSING_DESCRIPTION_REMINDER)
}

/// If this Bash call omitted its `description` and the session has not been
/// reminded yet, return the JSON `fleet guard` should print to inject a one-time
/// reminder. Returns `None` otherwise, so the caller keeps its silent-allow path.
pub fn missing_description_reminder_output(input: &HookInput) -> Option<String> {
    if !bash_missing_description(input) {
        return None;
    }
    let dir = desc_reminder_dir()?;
    let session_id = input.session_id.as_deref().unwrap_or("");
    if !claim_first_reminder_in(&dir, session_id) {
        return None;
    }
    Some(build_reminder_output())
}

// ── Missing Codex exec-note reminder ───────────────────────────────────────

/// Context returned after the first observed Codex `exec` script that omitted
/// Rule 7's leading summary comment. `PreToolUse` cannot block the already
/// started outer `exec`, but the context is added to the model before its next
/// tool decision, which corrects the rest of the session without spawning a
/// surprise follow-up turn.
pub const MISSING_EXEC_NOTE_REMINDER: &str = "Fleet Rule 7 violation: your latest `functions.exec` script omitted its required summary comment. On EVERY `functions.exec` call, make the exact FIRST line `// <one short summary in Boss's language>`. Do this before any pragma or JavaScript. This reminder fires once per session.";

/// Bound transcript work in the synchronous hook. The current outer exec call
/// is adjacent to the nested tool hook, so 256 KiB leaves ample room while
/// avoiding a full read of multi-megabyte rollouts on every command.
const EXEC_NOTE_TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

fn exec_script_has_note(script: &str) -> bool {
    let Some(first) = script.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return false;
    };
    first
        .strip_prefix("//")
        .map(str::trim)
        .is_some_and(|note| !note.is_empty())
}

/// Read the bounded tail of a Codex rollout and inspect its newest `exec`
/// custom-tool call. Any non-Codex transcript or malformed/incomplete tail is
/// a silent `false`: a cosmetic reminder must never interfere with Guard.
fn latest_codex_exec_missing_note(transcript_path: &std::path::Path) -> bool {
    let Ok(mut file) = fs::File::open(transcript_path) else {
        return false;
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return false;
    };
    let start = len.saturating_sub(EXEC_NOTE_TRANSCRIPT_TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut tail = String::new();
    if file.read_to_string(&mut tail).is_err() {
        return false;
    }
    if start > 0 {
        let Some(after_partial) = tail.split_once('\n').map(|(_, rest)| rest) else {
            return false;
        };
        tail = after_partial.to_string();
    }

    for line in tail.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(|v| v.as_str()) != Some("custom_tool_call")
            || payload.get("name").and_then(|v| v.as_str()) != Some("exec")
        {
            continue;
        }
        return payload
            .get("input")
            .and_then(|v| v.as_str())
            .is_some_and(|script| !exec_script_has_note(script));
    }
    false
}

fn exec_note_reminder_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|dir| dir.join("codex-exec-note-reminded"))
}

fn missing_exec_note_reminder_output_in(
    transcript_path: &std::path::Path,
    session_id: &str,
    marker_dir: &std::path::Path,
) -> Option<String> {
    if !latest_codex_exec_missing_note(transcript_path)
        || !claim_first_reminder_in(marker_dir, session_id)
    {
        return None;
    }
    Some(build_context_reminder_output(MISSING_EXEC_NOTE_REMINDER))
}

/// Return the one-time Rule 7 correction for the latest missing-note Codex
/// outer `exec`, if the hook supplied a readable rollout path.
pub fn missing_exec_note_reminder_output(input: &HookInput) -> Option<String> {
    let transcript = input.transcript_path.as_deref()?.trim();
    if transcript.is_empty() {
        return None;
    }
    let marker_dir = exec_note_reminder_dir()?;
    missing_exec_note_reminder_output_in(
        std::path::Path::new(transcript),
        input.session_id.as_deref().unwrap_or(""),
        &marker_dir,
    )
}

/// Generate a new unique guard request ID.
pub fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}

/// Truncate a string to at most `max` bytes for display, appending an ellipsis.
///
/// `max` is a *byte* budget. To avoid slicing through a multi-byte UTF-8
/// sequence (CJK is 3 bytes per char), the cut point is rounded down to the
/// previous char boundary — otherwise `&cmd[..max]` panics when `max` lands
/// mid-character, which crashed the whole desktop app via get_guard_context.
pub fn truncate_command(cmd: &str, max: usize) -> String {
    if cmd.len() <= max {
        return cmd.to_string();
    }
    let mut end = max;
    while end > 0 && !cmd.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &cmd[..end])
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
