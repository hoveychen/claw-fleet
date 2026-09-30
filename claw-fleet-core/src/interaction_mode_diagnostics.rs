//! Diagnostic checks for the QA interaction mode pipeline.
//!
//! Users sometimes report "I turned interaction mode on but Agent's
//! AskUserQuestion never reaches my Decision Panel". The link from toggle to
//! Decision Card has three backend-observable checkpoints and one
//! frontend-only one, all about the sessions Fleet starts — the pipeline
//! reaches them through launch arguments ([`crate::claude_launch`]), not the
//! user's global config:
//!
//!   1. `interaction_mode`    — the interaction-mode guidance is switched on,
//!      so a launch appends it to the system prompt.
//!   2. `elicitation_hook`    — the `PreToolUse → AskUserQuestion` hook is
//!      switched on, so a launch's `--settings` carries it and the CLI can
//!      intercept the tool call and write a request file.
//!   3. `fleet_mcp_server`    — a fleet binary resolves, so a launch's
//!      `--mcp-config` carries the server exposing `fleet__ask`.
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

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::consumer_heartbeat::{self, ConsumerStatus};
use crate::control_plane_prefs::{is_enabled, Feature};

/// Stable check ids — used by the frontend for i18n lookup and the Tauri
/// fix-action dispatch. Kept as `&'static str` here so the source of truth
/// is one place; serde serializes them as the same string via the `id`
/// field below.
pub mod id {
    pub const INTERACTION_MODE: &str = "interaction_mode";
    pub const ELICITATION_HOOK: &str = "elicitation_hook";
    pub const WATCHER_HEARTBEAT: &str = "watcher_heartbeat";
    pub const FLEET_MCP_SERVER: &str = "fleet_mcp_server";
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
    /// Switch interaction mode back on (`apply_interaction_mode`).
    ReinstallInteractionMode,
    /// Switch the elicitation hook back on (`apply_elicitation_hook`).
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
        check_interaction_mode(is_enabled(Feature::InteractionMode)),
        check_elicitation_hook(is_enabled(Feature::ElicitationHook)),
        check_watcher_heartbeat(&consumer_heartbeat::consumer_status(HEARTBEAT_STALE_AFTER)),
        check_fleet_mcp_server(
            crate::hooks::resolve_fleet_binary(),
            crate::mcp_injector::fleet_server_registered(),
        ),
    ]
}

pub fn check_interaction_mode(enabled: bool) -> DiagnosticCheck {
    let label = "interaction mode".to_string();
    if enabled {
        DiagnosticCheck {
            id: id::INTERACTION_MODE.into(),
            label,
            status: CheckStatus::Pass,
            detail: "On — Fleet sessions start with the decision-card guidance".into(),
            fix_action: None,
        }
    } else {
        DiagnosticCheck {
            id: id::INTERACTION_MODE.into(),
            label,
            status: CheckStatus::Fail,
            detail: "Off — Fleet sessions start without the guidance that steers the Agent \
                 to ask through the Decision Panel"
                .into(),
            fix_action: Some(FixAction::ReinstallInteractionMode),
        }
    }
}

pub fn check_elicitation_hook(enabled: bool) -> DiagnosticCheck {
    let label = "elicitation hook".to_string();
    if enabled {
        DiagnosticCheck {
            id: id::ELICITATION_HOOK.into(),
            label,
            status: CheckStatus::Pass,
            detail: "On — Fleet sessions carry the PreToolUse → AskUserQuestion hook".into(),
            fix_action: None,
        }
    } else {
        DiagnosticCheck {
            id: id::ELICITATION_HOOK.into(),
            label,
            status: CheckStatus::Fail,
            detail: "Off — AskUserQuestion calls in Fleet sessions will not be intercepted".into(),
            fix_action: Some(FixAction::EnableElicitationHook),
        }
    }
}

/// `fleet_bin` is what a launch would name in its `--mcp-config`;
/// `globally_registered` is an older Fleet's `mcpServers.fleet` in
/// `~/.claude.json`, which a launch defers to while it is still there.
pub fn check_fleet_mcp_server(
    fleet_bin: Option<String>,
    globally_registered: bool,
) -> DiagnosticCheck {
    let label = "fleet MCP server".to_string();
    match fleet_bin {
        Some(bin) => DiagnosticCheck {
            id: id::FLEET_MCP_SERVER.into(),
            label,
            status: CheckStatus::Pass,
            detail: format!("Fleet sessions get the fleet MCP server → {bin}"),
            fix_action: None,
        },
        None if globally_registered => DiagnosticCheck {
            id: id::FLEET_MCP_SERVER.into(),
            label,
            status: CheckStatus::Pass,
            detail: "Fleet sessions get the fleet MCP server from ~/.claude.json".into(),
            fix_action: None,
        },
        None => DiagnosticCheck {
            id: id::FLEET_MCP_SERVER.into(),
            label,
            status: CheckStatus::Fail,
            detail: "No fleet binary found next to this Fleet — sessions start without the \
                 fleet MCP server, so fleet__ask is invisible to Claude Code"
                .into(),
            fix_action: None,
        },
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
        // `fleet serve` / `fleet webui` *is* running — it wrote this file. What
        // it no longer has is a head: no SSE client, no phone on the relay, no
        // decision panel polling. Saying "start fleet serve" here would send
        // the reader after the one thing that is already true.
        ConsumerStatus::StaleServerNoHead { .. } => DiagnosticCheck {
            id: id::WATCHER_HEARTBEAT.into(),
            label: "decision watcher heartbeat".into(),
            status: CheckStatus::Fail,
            detail: format!(
                "`fleet serve`/`fleet webui` is running but nothing is watching it — open the web UI (or pair a phone) so cards have somewhere to appear: {status}"
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
    fn interaction_mode_on_passes_no_fix() {
        let c = check_interaction_mode(true);
        assert_eq!(c.id, id::INTERACTION_MODE);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn interaction_mode_off_fails_with_reinstall_fix() {
        let c = check_interaction_mode(false);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.fix_action, Some(FixAction::ReinstallInteractionMode));
    }

    #[test]
    fn elicitation_hook_on_passes() {
        let c = check_elicitation_hook(true);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn elicitation_hook_off_fails_with_enable_fix() {
        let c = check_elicitation_hook(false);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.fix_action, Some(FixAction::EnableElicitationHook));
    }

    #[test]
    fn watcher_alive_fresh_passes() {
        let s = ConsumerStatus::Alive {
            fresh: true,
            pid: Some(42),
        };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.fix_action.is_none());
    }

    #[test]
    fn watcher_alive_stale_warns() {
        let s = ConsumerStatus::Alive {
            fresh: false,
            pid: Some(42),
        };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("stale"));
    }

    #[test]
    fn watcher_file_unreadable_fails() {
        let s = ConsumerStatus::FileUnreadable("No such file".into());
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(
            c.fix_action.is_none(),
            "no automated fix for watcher absence"
        );
    }

    #[test]
    fn watcher_stale_pid_dead_fails() {
        let s = ConsumerStatus::StalePidDead {
            age_ms: 120_000,
            pid: 9999,
        };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    /// Fails, and says the useful thing: the server is up, it is the head that
    /// is missing. The generic arm's advice ("start `fleet serve`") is exactly
    /// the thing already true in this state.
    #[test]
    fn watcher_stale_server_no_head_fails_and_does_not_blame_the_server() {
        let s = ConsumerStatus::StaleServerNoHead {
            age_ms: 120_000,
            pid: 1688,
        };
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(
            c.detail.contains("nothing is watching"),
            "detail was {:?}",
            c.detail
        );
    }

    #[test]
    fn watcher_home_dir_unknown_fails() {
        let s = ConsumerStatus::HomeDirUnknown;
        let c = check_watcher_heartbeat(&s);
        assert_eq!(c.status, CheckStatus::Fail);
    }

    #[test]
    fn fleet_mcp_server_passes_with_a_binary() {
        let c = check_fleet_mcp_server(Some("/opt/fleet/bin/fleet".into()), false);
        assert_eq!(c.id, id::FLEET_MCP_SERVER);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.detail.contains("/opt/fleet/bin/fleet"));
    }

    #[test]
    fn fleet_mcp_server_passes_on_an_older_global_registration() {
        let c = check_fleet_mcp_server(None, true);
        assert_eq!(c.status, CheckStatus::Pass);
    }

    #[test]
    fn fleet_mcp_server_fails_without_a_binary_and_has_no_fix() {
        let c = check_fleet_mcp_server(None, false);
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.fix_action.is_none());
    }
}
