//! Plan snooze — "leave this plan alone until T", set by the agent (or by
//! Fleet itself) so the orphan reviver in [`crate::plan_revive`] does not keep
//! waking a session for a plan that is legitimately blocked.
//!
//! A plan can sit with pending P-tasks and no live session for good reasons: it
//! is waiting on the boss, on a login, on a deploy window, on a match that is
//! three days out. The reviver cannot tell those apart from "the session
//! crashed", so the agent that knows says so here, with a reason the boss can
//! read in the plan view.
//!
//! Same side-channel pattern as [`crate::task_progress`]: a Fleet-maintained
//! file outside TASKS.md, because the snooze is Fleet runtime state (it carries
//! a wall-clock deadline the agent cannot compute) and must not ride into git.
//!
//! Layout: `~/.fleet/plan-snooze/<plan_id>--<hash8>.json`. Plan ids are only
//! unique within one workspace, so the file name carries a short hash of the
//! canonical workspace root; the record repeats both fields for display.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Upper bound on one snooze. Anything longer is almost certainly "I gave up
/// on this plan", which should be said to the boss, not hidden behind a timer.
/// Fleet's own "don't ask again" snooze is the one exception and uses
/// `until_ms: None`.
pub const MAX_SNOOZE_MS: u64 = 14 * 24 * 3600 * 1000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PlanSnooze {
    pub workspace_path: String,
    pub plan_id: String,
    /// Epoch ms the snooze ends. `None` = until someone lifts it (only Fleet's
    /// own "stop asking about this plan" sets that).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub until_ms: Option<u64>,
    pub reason: String,
    /// Session id that set it, or `"fleet"` for the reviver's own snoozes.
    pub set_by: String,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub created_ms: u64,
}

impl PlanSnooze {
    pub fn is_active(&self, now_ms: u64) -> bool {
        self.until_ms.is_none_or(|u| now_ms < u)
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn snooze_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("plan-snooze"))
}

/// Stable key for (workspace, plan). Shared with [`crate::plan_revive`]'s
/// state store so both files for one plan line up.
pub(crate) fn plan_key(workspace_path: &str, plan_id: &str) -> String {
    let ws = workspace_path.trim_end_matches('/');
    let digest = Sha256::digest(ws.as_bytes());
    let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let safe: String = plan_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("{safe}--{hash}")
}

fn path_in(dir: &Path, workspace_path: &str, plan_id: &str) -> PathBuf {
    dir.join(format!("{}.json", plan_key(workspace_path, plan_id)))
}

/// Parse `8h`, `30m`, `2d`, `90s` (single unit) into milliseconds.
pub fn parse_duration_ms(spec: &str) -> Result<u64, String> {
    let s = spec.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("duration '{spec}' needs a unit (s/m/h/d), e.g. 8h"))?;
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("duration '{spec}' must start with a number, e.g. 8h"))?;
    let mult = match unit.trim() {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        other => return Err(format!("unknown duration unit '{other}' (use s/m/h/d)")),
    };
    if n == 0 {
        return Err("duration must be greater than zero".into());
    }
    Ok(n.saturating_mul(mult))
}

/// Snooze a plan for `duration_ms` (capped at [`MAX_SNOOZE_MS`]). Replaces any
/// existing snooze on the same plan.
pub fn snooze(
    workspace_path: &str,
    plan_id: &str,
    duration_ms: u64,
    reason: &str,
    set_by: &str,
) -> Result<PlanSnooze, String> {
    let dir = snooze_dir().ok_or("cannot determine home dir")?;
    snooze_in(&dir, workspace_path, plan_id, Some(duration_ms), reason, set_by, now_ms())
}

/// Snooze with no end — Fleet's "stop asking about this plan".
pub fn snooze_indefinitely(
    workspace_path: &str,
    plan_id: &str,
    reason: &str,
    set_by: &str,
) -> Result<PlanSnooze, String> {
    let dir = snooze_dir().ok_or("cannot determine home dir")?;
    snooze_in(&dir, workspace_path, plan_id, None, reason, set_by, now_ms())
}

pub(crate) fn snooze_in(
    dir: &Path,
    workspace_path: &str,
    plan_id: &str,
    duration_ms: Option<u64>,
    reason: &str,
    set_by: &str,
    now: u64,
) -> Result<PlanSnooze, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("a snooze needs a reason — say what the plan is blocked on".into());
    }
    let rec = PlanSnooze {
        workspace_path: workspace_path.to_string(),
        plan_id: plan_id.to_string(),
        until_ms: duration_ms.map(|d| now + d.min(MAX_SNOOZE_MS)),
        reason: reason.to_string(),
        set_by: set_by.to_string(),
        created_ms: now,
    };
    let bytes = serde_json::to_vec_pretty(&rec).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path_in(dir, workspace_path, plan_id), &bytes)
        .map_err(|e| format!("write plan snooze: {e}"))?;
    Ok(rec)
}

/// Lift a snooze. Returns whether one existed.
pub fn unsnooze(workspace_path: &str, plan_id: &str) -> bool {
    snooze_dir().is_some_and(|d| unsnooze_in(&d, workspace_path, plan_id))
}

pub(crate) fn unsnooze_in(dir: &Path, workspace_path: &str, plan_id: &str) -> bool {
    fs::remove_file(path_in(dir, workspace_path, plan_id)).is_ok()
}

/// The plan's snooze if one is on file and still active. Expired records are
/// left in place (harmless, and they tell the plan view "was snoozed until").
pub fn active(workspace_path: &str, plan_id: &str) -> Option<PlanSnooze> {
    active_in(&snooze_dir()?, workspace_path, plan_id, now_ms())
}

pub(crate) fn active_in(
    dir: &Path,
    workspace_path: &str,
    plan_id: &str,
    now: u64,
) -> Option<PlanSnooze> {
    let s = fs::read_to_string(path_in(dir, workspace_path, plan_id)).ok()?;
    let rec: PlanSnooze = serde_json::from_str(&s).ok()?;
    rec.is_active(now).then_some(rec)
}

/// Every active snooze in one workspace, keyed by plan id — for the plan view.
pub fn active_for_workspace(workspace_path: &str) -> std::collections::HashMap<String, PlanSnooze> {
    snooze_dir()
        .map(|d| active_for_workspace_in(&d, workspace_path, now_ms()))
        .unwrap_or_default()
}

pub(crate) fn active_for_workspace_in(
    dir: &Path,
    workspace_path: &str,
    now: u64,
) -> std::collections::HashMap<String, PlanSnooze> {
    let want = workspace_path.trim_end_matches('/');
    let mut out = std::collections::HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(s) = fs::read_to_string(e.path()) else { continue };
        let Ok(rec) = serde_json::from_str::<PlanSnooze>(&s) else { continue };
        if rec.workspace_path.trim_end_matches('/') == want && rec.is_active(now) {
            out.insert(rec.plan_id.clone(), rec);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_single_units() {
        assert_eq!(parse_duration_ms("90s").unwrap(), 90_000);
        assert_eq!(parse_duration_ms("30m").unwrap(), 1_800_000);
        assert_eq!(parse_duration_ms("8h").unwrap(), 28_800_000);
        assert_eq!(parse_duration_ms("2d").unwrap(), 172_800_000);
        assert!(parse_duration_ms("8").is_err());
        assert!(parse_duration_ms("h").is_err());
        assert!(parse_duration_ms("0h").is_err());
        assert!(parse_duration_ms("3w").is_err());
    }

    #[test]
    fn snooze_is_active_until_deadline_then_lapses() {
        let dir = tempfile::tempdir().unwrap();
        let rec = snooze_in(dir.path(), "/w", "p1", Some(1_000), "blocked on login", "s1", 10)
            .unwrap();
        assert_eq!(rec.until_ms, Some(1_010));
        assert!(active_in(dir.path(), "/w", "p1", 1_009).is_some());
        assert!(active_in(dir.path(), "/w", "p1", 1_010).is_none());
    }

    #[test]
    fn snooze_is_capped_and_requires_reason() {
        let dir = tempfile::tempdir().unwrap();
        let rec = snooze_in(dir.path(), "/w", "p1", Some(u64::MAX / 2), "x", "s1", 0).unwrap();
        assert_eq!(rec.until_ms, Some(MAX_SNOOZE_MS));
        assert!(snooze_in(dir.path(), "/w", "p1", Some(1), "  ", "s1", 0).is_err());
    }

    #[test]
    fn indefinite_snooze_never_lapses_until_lifted() {
        let dir = tempfile::tempdir().unwrap();
        snooze_in(dir.path(), "/w", "p1", None, "boss said stop asking", "fleet", 0).unwrap();
        assert!(active_in(dir.path(), "/w", "p1", u64::MAX).is_some());
        assert!(unsnooze_in(dir.path(), "/w", "p1"));
        assert!(active_in(dir.path(), "/w", "p1", 0).is_none());
        assert!(!unsnooze_in(dir.path(), "/w", "p1"));
    }

    #[test]
    fn same_plan_id_in_two_workspaces_is_two_snoozes() {
        let dir = tempfile::tempdir().unwrap();
        snooze_in(dir.path(), "/a", "p1", Some(1_000), "x", "s", 0).unwrap();
        assert!(active_in(dir.path(), "/b", "p1", 0).is_none());
        let by_ws = active_for_workspace_in(dir.path(), "/a/", 0);
        assert_eq!(by_ws.len(), 1);
        assert!(by_ws.contains_key("p1"));
    }
}
