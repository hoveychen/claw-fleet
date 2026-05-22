//! Diagnostic checks for the QA interaction mode pipeline.
//!
//! Users sometimes report "I turned interaction mode on but Agent's
//! AskUserQuestion never reaches my Decision Panel". The link from toggle to
//! Decision Card has four backend-observable checkpoints and one
//! frontend-only one:
//!
//!   1. `claude_md_sentinel`  — `~/.claude/CLAUDE.md` carries the
//!      `fleet:interaction-mode` sentinel block that imports the guidance.
//!   2. `guidance_file`       — `~/.claude/fleet-interaction-mode.md` exists
//!      and is non-empty.
//!   3. `elicitation_hook`    — `~/.claude/settings.json` has the Fleet
//!      `PreToolUse → AskUserQuestion` hook group so the CLI can intercept
//!      the tool call and write a request file.
//!   4. `watcher_heartbeat`   — The decision watcher (desktop app or
//!      `fleet serve` SSE consumer) is alive: it polls the elicitation dir
//!      and emits events. Tracked via `~/.fleet/consumer.heartbeat`.
//!   5. `frontend_listener`   — (frontend only) The Tauri event listener for
//!      `elicitation-request` is attached. Backend can't observe this; the
//!      desktop's diagnostics view flips this to Pass via the
//!      `test_decision_frontend_only` round-trip.
//!
//! This module owns the first four. The pure `check_*` helpers take the
//! already-collected raw inputs so they can be unit-tested without touching
//! the file system.

use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::consumer_heartbeat::{self, ConsumerStatus};
use crate::hooks;
use crate::interaction_mode;

/// Stable check ids — used by the frontend for i18n lookup and the Tauri
/// fix-action dispatch. Kept as `&'static str` here so the source of truth
/// is one place; serde serializes them as the same string via the `id`
/// field below.
pub mod id {
    pub const CLAUDE_MD_SENTINEL: &str = "claude_md_sentinel";
    pub const GUIDANCE_FILE: &str = "guidance_file";
    pub const ELICITATION_HOOK: &str = "elicitation_hook";
    pub const WATCHER_HEARTBEAT: &str = "watcher_heartbeat";
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixAction {
    /// Re-run `interaction_mode::apply_interaction_mode` to rewrite both the
    /// CLAUDE.md sentinel block and the guidance markdown file.
    ReinstallInteractionMode,
    /// Re-run `hooks::apply_elicitation_hook` to re-install the
    /// `PreToolUse → AskUserQuestion` hook group in settings.json.
    EnableElicitationHook,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub id: String,
    /// English fallback label; the frontend resolves the i18n title via `id`.
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
    /// When set, the frontend may show a one-click fix button that dispatches
    /// the corresponding Tauri command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_action: Option<FixAction>,
}

/// Matches the `stale_after` window the `fleet guard` / `fleet elicitation`
/// CLIs use when deciding whether to block on the consumer. Keeping the two
/// in lockstep means a Warn here is exactly the case where the hook would
/// also be uncertain.
const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(30);

/// Run all four backend-observable checks. The frontend appends its own
/// `frontend_listener` row.
pub fn run_checks() -> Vec<DiagnosticCheck> {
    vec![
        check_claude_md_sentinel(interaction_mode::is_interaction_mode_installed()),
        check_guidance_file(read_guidance_file()),
        check_elicitation_hook(hooks::plan_hook_setup().elicitation_installed),
        check_watcher_heartbeat(&consumer_heartbeat::consumer_status(HEARTBEAT_STALE_AFTER)),
    ]
}

fn read_guidance_file() -> Option<String> {
    let path = crate::session::real_home_dir()?
        .join(".claude")
        .join("fleet-interaction-mode.md");
    fs::read_to_string(&path).ok()
}

pub fn check_claude_md_sentinel(installed: bool) -> DiagnosticCheck {
    if installed {
        DiagnosticCheck {
            id: id::CLAUDE_MD_SENTINEL.into(),
            label: "CLAUDE.md sentinel block".into(),
            status: CheckStatus::Pass,
            detail: "Sentinel block found in ~/.claude/CLAUDE.md".into(),
            fix_action: None,
        }
    } else {
        DiagnosticCheck {
            id: id::CLAUDE_MD_SENTINEL.into(),
            label: "CLAUDE.md sentinel block".into(),
            status: CheckStatus::Fail,
            detail:
                "~/.claude/CLAUDE.md is missing the fleet:interaction-mode sentinel block — \
                 Agent will not load the AskUserQuestion guidance"
                    .into(),
            fix_action: Some(FixAction::ReinstallInteractionMode),
        }
    }
}

pub fn check_guidance_file(content: Option<String>) -> DiagnosticCheck {
    match content {
        Some(c) if !c.trim().is_empty() => DiagnosticCheck {
            id: id::GUIDANCE_FILE.into(),
            label: "fleet-interaction-mode.md".into(),
            status: CheckStatus::Pass,
            detail: format!(
                "~/.claude/fleet-interaction-mode.md present ({} bytes)",
                c.len()
            ),
            fix_action: None,
        },
        Some(_) => DiagnosticCheck {
            id: id::GUIDANCE_FILE.into(),
            label: "fleet-interaction-mode.md".into(),
            status: CheckStatus::Fail,
            detail: "~/.claude/fleet-interaction-mode.md exists but is empty".into(),
            fix_action: Some(FixAction::ReinstallInteractionMode),
        },
        None => DiagnosticCheck {
            id: id::GUIDANCE_FILE.into(),
            label: "fleet-interaction-mode.md".into(),
            status: CheckStatus::Fail,
            detail: "~/.claude/fleet-interaction-mode.md does not exist".into(),
            fix_action: Some(FixAction::ReinstallInteractionMode),
        },
    }
}

pub fn check_elicitation_hook(installed: bool) -> DiagnosticCheck {
    if installed {
        DiagnosticCheck {
            id: id::ELICITATION_HOOK.into(),
            label: "elicitation hook in settings.json".into(),
            status: CheckStatus::Pass,
            detail: "Fleet PreToolUse → AskUserQuestion hook installed".into(),
            fix_action: None,
        }
    } else {
        DiagnosticCheck {
            id: id::ELICITATION_HOOK.into(),
            label: "elicitation hook in settings.json".into(),
            status: CheckStatus::Fail,
            detail:
                "~/.claude/settings.json does not have the Fleet elicitation hook — \
                 AskUserQuestion calls will not be intercepted"
                    .into(),
            fix_action: Some(FixAction::EnableElicitationHook),
        }
    }
}

pub fn check_watcher_heartbeat(status: &ConsumerStatus) -> DiagnosticCheck {
    match status {
        ConsumerStatus::Alive { fresh: true, .. } => DiagnosticCheck {
            id: id::WATCHER_HEARTBEAT.into(),
            label: "decision watcher heartbeat".into(),
            status: CheckStatus::Pass,
            detail: format!("Watcher fresh: {status}"),
            fix_action: None,
        },
        ConsumerStatus::Alive { fresh: false, .. } => DiagnosticCheck {
            id: id::WATCHER_HEARTBEAT.into(),
            label: "decision watcher heartbeat".into(),
            status: CheckStatus::Warn,
            detail: format!(
                "Watcher process alive but heartbeat stale (system sleep / frozen process): {status}"
            ),
            fix_action: None,
        },
        _ => DiagnosticCheck {
            id: id::WATCHER_HEARTBEAT.into(),
            label: "decision watcher heartbeat".into(),
            status: CheckStatus::Fail,
            detail: format!(
                "No live watcher detected — desktop app or `fleet serve` SSE consumer must be running: {status}"
            ),
            fix_action: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_md_sentinel_installed_passes_no_fix() {
        let c = check_claude_md_sentinel(true);
        assert_eq!(c.id, id::CLAUDE_MD_SENTINEL);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn claude_md_sentinel_missing_fails_with_reinstall_fix() {
        let c = check_claude_md_sentinel(false);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.fix_action, Some(FixAction::ReinstallInteractionMode));
        assert!(c.detail.contains("CLAUDE.md"));
    }

    #[test]
    fn guidance_file_present_passes() {
        let c = check_guidance_file(Some("# Fleet Interaction Mode...".to_string()));
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.detail.contains("bytes"));
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn guidance_file_empty_fails_with_reinstall_fix() {
        let c = check_guidance_file(Some("   \n  ".to_string()));
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.fix_action, Some(FixAction::ReinstallInteractionMode));
        assert!(c.detail.contains("empty"));
    }

    #[test]
    fn guidance_file_missing_fails_with_reinstall_fix() {
        let c = check_guidance_file(None);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.fix_action, Some(FixAction::ReinstallInteractionMode));
        assert!(c.detail.contains("does not exist"));
    }

    #[test]
    fn elicitation_hook_installed_passes() {
        let c = check_elicitation_hook(true);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn elicitation_hook_missing_fails_with_enable_fix() {
        let c = check_elicitation_hook(false);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.fix_action, Some(FixAction::EnableElicitationHook));
        assert!(c.detail.contains("settings.json"));
    }

    #[test]
    fn watcher_alive_fresh_passes() {
        let s = ConsumerStatus::Alive { fresh: true, pid: Some(42) };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn watcher_alive_stale_warns() {
        let s = ConsumerStatus::Alive { fresh: false, pid: Some(42) };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("stale"));
    }

    #[test]
    fn watcher_file_unreadable_fails() {
        let s = ConsumerStatus::FileUnreadable("No such file".into());
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.fix_action.is_none(), "no automated fix for watcher absence");
    }

    #[test]
    fn watcher_stale_pid_dead_fails() {
        let s = ConsumerStatus::StalePidDead { age_ms: 120_000, pid: 9999 };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    #[test]
    fn watcher_home_dir_unknown_fails() {
        let s = ConsumerStatus::HomeDirUnknown;
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    #[test]
    fn serialized_check_uses_camel_case_for_fix_action() {
        let c = check_claude_md_sentinel(false);
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            json.contains("\"fixAction\":\"reinstall_interaction_mode\""),
            "frontend expects camelCase field name + snake_case enum tag: {json}"
        );
    }

    #[test]
    fn serialized_check_omits_fix_action_when_none() {
        let c = check_claude_md_sentinel(true);
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            !json.contains("fixAction"),
            "no fixAction key should appear for a Pass result: {json}"
        );
    }

    #[test]
    fn check_status_serializes_lowercase() {
        let c = check_claude_md_sentinel(true);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"status\":\"pass\""));
    }
}
