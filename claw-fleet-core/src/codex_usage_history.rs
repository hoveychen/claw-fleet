//! Codex usage-occupancy history — the codex parallel of the Claude usage
//! snapshot store in [`crate::account`].
//!
//! Claude records a `SnapshotEntry` on every `fetch_account_info`; this module
//! does the equivalent for Codex, persisting each `account/rateLimits/read`
//! reading so the desktop "占用率历史" chart has continuous 24h coverage even
//! when the usage panel isn't actively polling.
//!
//! Two deliberate differences from the Claude store:
//! - Codex percentages are the **0–100 ints** the app-server hands back
//!   (`usedPercent`), not Claude's 0–1 fractions — no normalization needed.
//! - Codex has no per-window "vs previous period" trend (the live bars show no
//!   trend arrow), so there is no `resets_at` bookkeeping / prev-utilization
//!   lookup here — just the raw percentage time series.

use serde::{Deserialize, Serialize};

use crate::codex_source::CodexUsageItem;

/// Snapshots older than this are dropped on each write. Kept at 8 days to mirror
/// the Claude store ([`crate::account`]); the 24h occupancy chart only ever
/// reads the recent tail, so the extra retention is just cheap slack.
const HISTORY_RETENTION_MS: i64 = 8 * 24 * 3600 * 1000;

/// One point of the codex usage-occupancy time series consumed by the codex
/// "占用率历史" chart. `*_pct` are the 0–100 ints (the UI plots them directly).
/// The window lengths ride along so the chart can label each line the same way
/// the live bars do (`codexWindowLabel`) — a Team plan reports a single 7-day
/// window in the primary slot, so the label must be derived from the duration.
///
/// camelCase on the wire to match the rest of the codex-facing types
/// (`CodexUsageItem` etc.), unlike the snake_case Claude `UsageHistoryPoint`.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageHistoryPoint {
    pub ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_pct: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_pct: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_window_mins: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_window_mins: Option<i64>,
}

fn history_path() -> Option<std::path::PathBuf> {
    crate::session::real_home_dir()
        .map(|h| h.join(".fleet").join("claw-fleet-codex-usage-history.json"))
}

fn load_from(path: &std::path::Path) -> Vec<CodexUsageHistoryPoint> {
    // Read-only callers get an empty series on any absent/corrupt/unreadable
    // file; a corrupt file is preserved to `.corrupt-*` by `load_preserving`.
    match crate::atomic_json::load_preserving::<Vec<CodexUsageHistoryPoint>>(path) {
        crate::atomic_json::JsonLoad::Loaded(v) => v,
        _ => Vec::new(),
    }
}

fn load() -> Vec<CodexUsageHistoryPoint> {
    match history_path() {
        Some(p) => load_from(&p),
        None => vec![],
    }
}

fn save_to(path: &std::path::Path, points: &[CodexUsageHistoryPoint]) {
    if let Ok(json) = serde_json::to_vec(points) {
        let _ = crate::atomic_json::write_atomic(path, &json);
    }
}

/// Minimum gap between two persisted codex samples. `record_snapshot` fires on
/// every successful `fetch_codex_usage` (on-demand, ~every 10s), which without
/// this throttle stormed the file and multiplied torn-write collisions — the
/// same hazard as the Claude store's `MIN_APPEND_INTERVAL_MS`.
const MIN_APPEND_INTERVAL_MS: i64 = 60 * 1000;

/// Drop points older than `HISTORY_RETENTION_MS` relative to `now_ms`.
fn prune_old(history: &mut Vec<CodexUsageHistoryPoint>, now_ms: i64) {
    let cutoff = now_ms - HISTORY_RETENTION_MS;
    history.retain(|p| p.ts >= cutoff);
}

/// Project a live `CodexUsageItem` into a persistable history point at `now_ms`.
fn point_from_usage(usage: &CodexUsageItem, now_ms: i64) -> CodexUsageHistoryPoint {
    CodexUsageHistoryPoint {
        ts: now_ms,
        primary_pct: usage.primary.as_ref().map(|w| w.used_percent),
        secondary_pct: usage.secondary.as_ref().map(|w| w.used_percent),
        primary_window_mins: usage.primary.as_ref().and_then(|w| w.window_duration_mins),
        secondary_window_mins: usage.secondary.as_ref().and_then(|w| w.window_duration_mins),
    }
}

/// Append one codex usage reading to the on-disk history, prune stale points,
/// and persist. A no-op if the reading carries neither window (nothing to plot).
/// Called as a side effect of every successful `fetch_codex_usage`, exactly the
/// way `fetch_account_info` records the Claude snapshot.
pub fn record_snapshot(usage: &CodexUsageItem) {
    if usage.primary.is_none() && usage.secondary.is_none() {
        return;
    }
    let path = match history_path() {
        Some(p) => p,
        None => return,
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    record_snapshot_at(&path, usage, now_ms);
}

/// Testable core of [`record_snapshot`] against an explicit path + clock.
/// Non-destructive (an unreadable file is left untouched, a corrupt one backed
/// up) and throttled (samples closer than `MIN_APPEND_INTERVAL_MS` are dropped).
fn record_snapshot_at(path: &std::path::Path, usage: &CodexUsageItem, now_ms: i64) {
    let mut history = match crate::atomic_json::load_preserving::<Vec<CodexUsageHistoryPoint>>(path)
    {
        crate::atomic_json::JsonLoad::Loaded(v) => v,
        crate::atomic_json::JsonLoad::Missing | crate::atomic_json::JsonLoad::Corrupt => Vec::new(),
        // Couldn't read the file — skip persisting so we never overwrite bytes
        // we failed to read.
        crate::atomic_json::JsonLoad::Unreadable => return,
    };
    let last_ts = history.iter().map(|p| p.ts).max().unwrap_or(i64::MIN);
    if now_ms.saturating_sub(last_ts) < MIN_APPEND_INTERVAL_MS {
        return;
    }
    history.push(point_from_usage(usage, now_ms));
    prune_old(&mut history, now_ms);
    save_to(path, &history);
}

/// Load persisted codex usage points whose timestamp falls within
/// `[from_ms, to_ms]` (inclusive), sorted ascending by timestamp. The read side
/// consumed by the codex occupancy chart via the `codex_usage_history` Backend
/// method.
pub fn load_codex_usage_history(from_ms: i64, to_ms: i64) -> Vec<CodexUsageHistoryPoint> {
    let mut points: Vec<CodexUsageHistoryPoint> = load()
        .into_iter()
        .filter(|p| p.ts >= from_ms && p.ts <= to_ms)
        .collect();
    points.sort_by_key(|p| p.ts);
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_source::CodexRateLimitWindow;

    fn window(used: i32, mins: i64) -> CodexRateLimitWindow {
        CodexRateLimitWindow {
            used_percent: used,
            window_duration_mins: Some(mins),
            resets_at: Some(0),
        }
    }

    // Same regression as the Claude store: a corrupt/torn codex history file
    // must be preserved, not silently overwritten by the next reading.
    #[test]
    fn record_snapshot_at_backs_up_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex-history.json");
        std::fs::write(&path, "[{\"ts\":1 GARBAGE").unwrap();

        let usage = CodexUsageItem { primary: Some(window(50, 300)), ..Default::default() };
        record_snapshot_at(&path, &usage, 2_000_000);

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt"))
            .collect();
        assert_eq!(backups.len(), 1, "corrupt codex history must be backed up, not destroyed");
    }

    // `record_snapshot` fires on every fetch_codex_usage (~10s); throttle it.
    #[test]
    fn record_snapshot_at_throttled_within_min_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex-history.json");
        let base = 1_000_000_000_i64;
        let seed = vec![CodexUsageHistoryPoint { ts: base, ..Default::default() }];
        std::fs::write(&path, serde_json::to_string(&seed).unwrap()).unwrap();

        let usage = CodexUsageItem { primary: Some(window(50, 300)), ..Default::default() };
        record_snapshot_at(&path, &usage, base + 10_000); // 10s later — throttled
        assert_eq!(
            load_from(&path).len(),
            1,
            "a codex sample within the min interval must not be appended"
        );

        record_snapshot_at(&path, &usage, base + 600_000); // 10min later — appended
        assert_eq!(load_from(&path).len(), 2);
    }

    #[test]
    fn point_from_usage_projects_both_windows() {
        let usage = CodexUsageItem {
            primary: Some(window(58, 300)),
            secondary: Some(window(11, 10080)),
            ..Default::default()
        };
        let p = point_from_usage(&usage, 1_000);
        assert_eq!(p.ts, 1_000);
        assert_eq!(p.primary_pct, Some(58));
        assert_eq!(p.secondary_pct, Some(11));
        assert_eq!(p.primary_window_mins, Some(300));
        assert_eq!(p.secondary_window_mins, Some(10080));
    }

    #[test]
    fn point_from_usage_tolerates_single_window() {
        // Team plan: a single 7-day window in the primary slot, no secondary.
        let usage = CodexUsageItem {
            primary: Some(window(7, 10080)),
            secondary: None,
            ..Default::default()
        };
        let p = point_from_usage(&usage, 500);
        assert_eq!(p.primary_pct, Some(7));
        assert_eq!(p.secondary_pct, None);
        assert_eq!(p.secondary_window_mins, None);
    }

    #[test]
    fn prune_drops_only_points_older_than_retention() {
        let now = 100 * 24 * 3600 * 1000; // some day-100 epoch-like value
        let day = 24 * 3600 * 1000;
        let mut history = vec![
            CodexUsageHistoryPoint { ts: now - 9 * day, ..Default::default() }, // stale
            CodexUsageHistoryPoint { ts: now - 7 * day, ..Default::default() }, // kept
            CodexUsageHistoryPoint { ts: now, ..Default::default() },           // kept
        ];
        prune_old(&mut history, now);
        let kept: Vec<i64> = history.iter().map(|p| p.ts).collect();
        assert_eq!(kept, vec![now - 7 * day, now]);
    }

    #[test]
    fn prune_keeps_exactly_the_retention_boundary() {
        let now = 100 * 24 * 3600 * 1000;
        let mut history = vec![
            CodexUsageHistoryPoint { ts: now - HISTORY_RETENTION_MS, ..Default::default() },
            CodexUsageHistoryPoint { ts: now - HISTORY_RETENTION_MS - 1, ..Default::default() },
        ];
        prune_old(&mut history, now);
        let kept: Vec<i64> = history.iter().map(|p| p.ts).collect();
        assert_eq!(kept, vec![now - HISTORY_RETENTION_MS], "boundary point retained, 1ms older dropped");
    }
}
