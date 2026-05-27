use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Usage / Account types ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct UsageStats {
    pub utilization: f64,
    pub resets_at: String,
    pub prev_utilization: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AccountInfo {
    pub email: String,
    pub full_name: String,
    pub organization_name: String,
    pub plan: String,
    pub auth_method: String,
    pub five_hour: Option<UsageStats>,
    pub seven_day: Option<UsageStats>,
    pub seven_day_sonnet: Option<UsageStats>,
}

// ── Usage snapshot history ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
struct MetricSnap {
    utilization: f64,
    resets_at: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct SnapshotEntry {
    ts: i64,
    five_hour: Option<MetricSnap>,
    seven_day: Option<MetricSnap>,
    seven_day_sonnet: Option<MetricSnap>,
}

/// Snapshots older than this are dropped on each write. Kept at 8 days so the
/// 7-day "vs previous period" comparison (`find_prev_utilization`) still has a
/// full window of data plus a day of slack — the 24h occupancy chart only ever
/// reads the recent tail, but the 7d comparison genuinely needs the long history.
const HISTORY_RETENTION_MS: i64 = 8 * 24 * 3600 * 1000;

/// One point of the usage-occupancy time series consumed by the "占用率变化"
/// chart. `utilization` values are the 0–1 fraction (the UI multiplies by 100).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct UsageHistoryPoint {
    pub ts: i64,
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    pub seven_day_sonnet: Option<f64>,
}

/// Drop snapshots older than `HISTORY_RETENTION_MS` relative to `now_ms`.
fn prune_old_snapshots(history: &mut Vec<SnapshotEntry>, now_ms: i64) {
    let cutoff = now_ms - HISTORY_RETENTION_MS;
    history.retain(|e| e.ts >= cutoff);
}

fn snapshot_path() -> Option<std::path::PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("claw-fleet-usage-history.json"))
}

fn normalize_snap(snap: MetricSnap) -> MetricSnap {
    if snap.utilization > 1.0 {
        MetricSnap { utilization: snap.utilization / 100.0, resets_at: snap.resets_at }
    } else {
        snap
    }
}

fn load_snapshots() -> Vec<SnapshotEntry> {
    let path = match snapshot_path() {
        Some(p) => p,
        None => return vec![],
    };
    let entries: Vec<SnapshotEntry> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    entries
        .into_iter()
        .map(|e| SnapshotEntry {
            ts: e.ts,
            five_hour: e.five_hour.map(normalize_snap),
            seven_day: e.seven_day.map(normalize_snap),
            seven_day_sonnet: e.seven_day_sonnet.map(normalize_snap),
        })
        .collect()
}

fn save_snapshots(entries: &[SnapshotEntry]) {
    if let Some(path) = snapshot_path() {
        if let Ok(json) = serde_json::to_string(entries) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn period_ms(metric: &str) -> i64 {
    match metric {
        "five_hour" => 5 * 3600 * 1000,
        _ => 7 * 24 * 3600 * 1000,
    }
}

fn parse_ts_ms(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn get_metric_snap<'a>(entry: &'a SnapshotEntry, metric: &str) -> Option<&'a MetricSnap> {
    match metric {
        "five_hour" => entry.five_hour.as_ref(),
        "seven_day" => entry.seven_day.as_ref(),
        "seven_day_sonnet" => entry.seven_day_sonnet.as_ref(),
        _ => None,
    }
}

fn find_prev_utilization(
    history: &[SnapshotEntry],
    metric: &str,
    current_resets_at: &str,
    now_ms: i64,
) -> Option<f64> {
    let current_reset_ms = parse_ts_ms(current_resets_at)?;
    let pms = period_ms(metric);
    let current_start_ms = current_reset_ms - pms;
    let current_frac =
        ((now_ms - current_start_ms) as f64 / pms as f64).clamp(0.0, 1.0);

    let mut prev_resets: Vec<String> = history
        .iter()
        .filter_map(|e| get_metric_snap(e, metric))
        .filter(|m| m.resets_at != current_resets_at)
        .filter(|m| {
            parse_ts_ms(&m.resets_at)
                .map(|t| t < current_reset_ms)
                .unwrap_or(false)
        })
        .map(|m| m.resets_at.clone())
        .collect();
    prev_resets.sort();
    prev_resets.dedup();

    let prev_resets_at = prev_resets.last()?;
    let prev_reset_ms = parse_ts_ms(prev_resets_at)?;
    let prev_start_ms = prev_reset_ms - pms;

    history
        .iter()
        .filter_map(|e| {
            let snap = get_metric_snap(e, metric)?;
            if &snap.resets_at != prev_resets_at {
                return None;
            }
            let frac = ((e.ts - prev_start_ms) as f64 / pms as f64).clamp(0.0, 1.0);
            Some((frac, snap.utilization))
        })
        .min_by(|(f1, _), (f2, _)| {
            (f1 - current_frac)
                .abs()
                .partial_cmp(&(f2 - current_frac).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, u)| u)
}

// ── Credential loading ────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn read_keychain_credentials() -> Result<(String, String), String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .map_err(|e| format!("security command failed: {e}"))?;

    let raw = if out.status.success() {
        String::from_utf8(out.stdout).map_err(|e| e.to_string())?
    } else {
        let cred_path = crate::session::real_home_dir()
            .ok_or("No home dir")?
            .join(".claude")
            .join(".credentials.json");
        std::fs::read_to_string(&cred_path)
            .map_err(|_| "Credentials not found in keychain or file".to_string())?
    };

    let json: Value = serde_json::from_str(raw.trim()).map_err(|e| e.to_string())?;
    let oauth = json.get("claudeAiOauth").ok_or("No claudeAiOauth key")?;
    let token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or("No accessToken")?
        .to_string();
    let sub = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    Ok((token, sub))
}

#[cfg(not(target_os = "macos"))]
pub fn read_keychain_credentials() -> Result<(String, String), String> {
    let cred_path = crate::session::real_home_dir()
        .ok_or("No home dir")?
        .join(".claude")
        .join(".credentials.json");
    let raw = std::fs::read_to_string(&cred_path)
        .map_err(|e| format!("{e} (tried: {})", cred_path.display()))?;
    let json: Value = serde_json::from_str(raw.trim()).map_err(|e| e.to_string())?;
    let oauth = json.get("claudeAiOauth").ok_or("No claudeAiOauth key")?;
    let token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or("No accessToken")?
        .to_string();
    let sub = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    Ok((token, sub))
}

fn parse_usage(v: &Value) -> Option<UsageStats> {
    let utilization = v.get("utilization")?.as_f64()? / 100.0;
    let resets_at = v.get("resets_at")?.as_str().unwrap_or("").to_string();
    Some(UsageStats { utilization, resets_at, prev_utilization: None })
}

// ── Account info fetch ────────────────────────────────────────────────────────

pub async fn fetch_account_info() -> Result<AccountInfo, String> {
    let (token, subscription_type) = read_keychain_credentials()?;

    let client = reqwest::Client::new();
    let auth_header = format!("Bearer {}", token);
    let beta = "oauth-2025-04-20";

    // Fire both requests concurrently
    let profile_fut = client
        .get("https://api.anthropic.com/api/oauth/profile")
        .header("Authorization", &auth_header)
        .header("anthropic-beta", beta)
        .timeout(std::time::Duration::from_secs(5))
        .send();

    let usage_fut = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &auth_header)
        .header("anthropic-beta", beta)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(5))
        .send();

    let (profile_res, usage_res) = futures::future::join(profile_fut, usage_fut).await;

    let profile_raw = profile_res.map_err(|e| {
        let mut msg = format!("Profile request failed: {e}");
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            msg.push_str(&format!("\n  caused by: {cause}"));
            source = std::error::Error::source(cause);
        }
        msg
    })?;
    let profile_status = profile_raw.status();
    let profile_body = profile_raw
        .json::<Value>()
        .await
        .map_err(|e| format!("Profile parse failed: {e}"))?;
    if !profile_status.is_success() {
        return Err(format!("Profile API error {profile_status}: {profile_body}"));
    }

    let usage_raw = usage_res.map_err(|e| format!("Usage request failed: {e}"))?;
    let usage_status = usage_raw.status();
    let usage_body = usage_raw
        .json::<Value>()
        .await
        .map_err(|e| format!("Usage parse failed: {e}"))?;
    if !usage_status.is_success() {
        return Err(format!("Usage API error {usage_status}: {usage_body}"));
    }

    let email = profile_body
        .pointer("/account/email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let full_name = profile_body
        .pointer("/account/full_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let org_name = profile_body
        .pointer("/organization/name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let has_max = profile_body
        .pointer("/account/has_claude_max")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_pro = profile_body
        .pointer("/account/has_claude_pro")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let plan = if has_max {
        "Claude Max".to_string()
    } else if has_pro || subscription_type == "pro" {
        "Claude Pro".to_string()
    } else {
        "API / Free".to_string()
    };

    let mut five_hour = usage_body.get("five_hour").and_then(|v| parse_usage(v));
    let mut seven_day = usage_body.get("seven_day").and_then(|v| parse_usage(v));
    let mut seven_day_sonnet = usage_body.get("seven_day_sonnet").and_then(|v| parse_usage(v));

    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut history = load_snapshots();
    history.push(SnapshotEntry {
        ts: now_ms,
        five_hour: five_hour.as_ref().map(|s| MetricSnap {
            utilization: s.utilization,
            resets_at: s.resets_at.clone(),
        }),
        seven_day: seven_day.as_ref().map(|s| MetricSnap {
            utilization: s.utilization,
            resets_at: s.resets_at.clone(),
        }),
        seven_day_sonnet: seven_day_sonnet.as_ref().map(|s| MetricSnap {
            utilization: s.utilization,
            resets_at: s.resets_at.clone(),
        }),
    });
    prune_old_snapshots(&mut history, now_ms);
    save_snapshots(&history);

    if let Some(ref mut s) = five_hour {
        let ra = s.resets_at.clone();
        s.prev_utilization = find_prev_utilization(&history, "five_hour", &ra, now_ms);
    }
    if let Some(ref mut s) = seven_day {
        let ra = s.resets_at.clone();
        s.prev_utilization = find_prev_utilization(&history, "seven_day", &ra, now_ms);
    }
    if let Some(ref mut s) = seven_day_sonnet {
        let ra = s.resets_at.clone();
        s.prev_utilization = find_prev_utilization(&history, "seven_day_sonnet", &ra, now_ms);
    }

    Ok(AccountInfo {
        email,
        full_name,
        organization_name: org_name,
        plan,
        auth_method: "claudeai".to_string(),
        five_hour,
        seven_day,
        seven_day_sonnet,
    })
}

/// Blocking wrapper for use in the fleet CLI and background threads.
/// Handles being called both from within a tokio runtime (via `block_in_place`)
/// and from plain threads (via a new runtime).
pub fn fetch_account_info_blocking() -> Result<AccountInfo, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(fetch_account_info()))
    } else {
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to create tokio runtime: {e}"))?
            .block_on(fetch_account_info())
    }
}

/// Load persisted usage snapshots whose timestamp falls within
/// `[from_ms, to_ms]` (inclusive), projected to just the per-metric
/// utilization fractions. Sorted ascending by timestamp. Used by the
/// 24h occupancy chart via the `usage_history` Backend method.
pub fn load_usage_history(from_ms: i64, to_ms: i64) -> Vec<UsageHistoryPoint> {
    let mut points: Vec<UsageHistoryPoint> = load_snapshots()
        .into_iter()
        .filter(|e| e.ts >= from_ms && e.ts <= to_ms)
        .map(|e| UsageHistoryPoint {
            ts: e.ts,
            five_hour: e.five_hour.map(|m| m.utilization),
            seven_day: e.seven_day.map(|m| m.utilization),
            seven_day_sonnet: e.seven_day_sonnet.map(|m| m.utilization),
        })
        .collect();
    points.sort_by_key(|p| p.ts);
    points
}

use std::sync::atomic::{AtomicBool, Ordering};

/// Guards against spawning more than one sampler thread per process.
static SAMPLER_STARTED: AtomicBool = AtomicBool::new(false);

/// Spawn (once per process) a background thread that samples the Claude usage
/// API every `interval` and appends a snapshot as a side effect of
/// `fetch_account_info_blocking`. This gives the 24h occupancy chart continuous
/// coverage even when the desktop UI's usage tab isn't actively polling.
///
/// Idempotent: the second and later calls in the same process are no-ops, so it
/// is safe to call from both `fleet serve` startup and the desktop app startup.
/// Errors (offline, not logged in) are swallowed — the loop simply retries on
/// the next tick. The thread runs until process exit; no handle is kept.
pub fn start_background_sampler(interval: std::time::Duration) {
    if SAMPLER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || loop {
        let _ = fetch_account_info_blocking();
        std::thread::sleep(interval);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_at(ts: i64) -> SnapshotEntry {
        SnapshotEntry {
            ts,
            five_hour: Some(MetricSnap { utilization: 0.5, resets_at: String::new() }),
            seven_day: None,
            seven_day_sonnet: None,
        }
    }

    #[test]
    fn prune_drops_only_entries_older_than_retention() {
        let now = 100 * 24 * 3600 * 1000; // day 100, in ms
        let day = 24 * 3600 * 1000;
        let mut history = vec![
            snap_at(now - 9 * day), // older than 8d → dropped
            snap_at(now - 8 * day - 1), // just over 8d → dropped
            snap_at(now - 8 * day + 1), // just under 8d → kept
            snap_at(now - 1 * day), // recent → kept
            snap_at(now),           // now → kept
        ];
        prune_old_snapshots(&mut history, now);
        let kept: Vec<i64> = history.iter().map(|e| e.ts).collect();
        assert_eq!(
            kept,
            vec![now - 8 * day + 1, now - 1 * day, now],
            "only snapshots within 8 days of now should survive"
        );
    }

    #[test]
    fn prune_keeps_a_full_seven_day_window() {
        // The 7d "vs previous period" comparison needs ≥7 days of history.
        let now = 100 * 24 * 3600 * 1000;
        let day = 24 * 3600 * 1000;
        let mut history = vec![snap_at(now - 7 * day), snap_at(now)];
        prune_old_snapshots(&mut history, now);
        assert_eq!(history.len(), 2, "a 7-day-old snapshot must be retained");
    }
}
