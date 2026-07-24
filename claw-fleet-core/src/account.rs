use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Usage / Account types ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct UsageStats {
    pub utilization: f64,
    pub resets_at: String,
    pub prev_utilization: Option<f64>,
}

/// One per-model weekly-scoped usage window, parsed from the usage API's
/// `limits[]` array (entries with `kind == "weekly_scoped"`). `model_label` is
/// the scope's `display_name` — e.g. "Fable". This replaces the old fixed
/// `seven_day_sonnet` slot: Anthropic now returns that top-level field null and
/// expresses every model-scoped weekly cap through `limits[]` instead, so the
/// scoped model can be Fable (or several) rather than always Sonnet.
/// `utilization` is the 0–1 fraction (the API hands back an integer `percent`;
/// we divide by 100 to match `UsageStats`).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ScopedUsage {
    pub model_label: String,
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
    /// Per-model weekly-scoped windows (from `limits[]`). Replaces the old
    /// `seven_day_sonnet` field. `#[serde(default)]` keeps older serialized
    /// payloads (which lacked this field) deserializable.
    #[serde(default)]
    pub seven_day_scoped: Vec<ScopedUsage>,
    /// Where the usage numbers came from: "foxy-switcher" when read from a
    /// running foxy daemon's local API, "anthropic" when fetched directly.
    /// `#[serde(default)]` keeps older serialized payloads deserializable.
    #[serde(default)]
    pub usage_source: String,
}

// ── Usage snapshot history ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
struct MetricSnap {
    utilization: f64,
    resets_at: String,
}

/// One scoped-model window inside a persisted snapshot, keyed by the model's
/// display name so `find_prev_utilization` can match periods per model.
#[derive(Serialize, Deserialize, Clone, Default)]
struct ScopedSnap {
    model_label: String,
    snap: MetricSnap,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct SnapshotEntry {
    ts: i64,
    five_hour: Option<MetricSnap>,
    seven_day: Option<MetricSnap>,
    /// Retained so the `UsageHistoryPoint` that auto-resume reads keeps its
    /// `seven_day_sonnet` field. Anthropic no longer populates a Sonnet-specific
    /// weekly window (scoped models moved to `limits[]` → `seven_day_scoped`),
    /// so new samples always write `None` here; the field stays for old on-disk
    /// snapshots and for auto-resume's `SonnetLimit` recovery path, which keys
    /// off rate-limit *error text* and is out of scope for the scoped-usage
    /// display work.
    #[serde(default)]
    seven_day_sonnet: Option<MetricSnap>,
    /// Per-model weekly-scoped windows sampled from `limits[]`.
    #[serde(default)]
    seven_day_scoped: Vec<ScopedSnap>,
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
            seven_day_scoped: e
                .seven_day_scoped
                .into_iter()
                .map(|s| ScopedSnap { model_label: s.model_label, snap: normalize_snap(s.snap) })
                .collect(),
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

/// The weekly-scoped snapshot for `model_label` within one entry, if present.
fn get_scoped_snap<'a>(entry: &'a SnapshotEntry, model_label: &str) -> Option<&'a MetricSnap> {
    entry
        .seven_day_scoped
        .iter()
        .find(|s| s.model_label == model_label)
        .map(|s| &s.snap)
}

/// Find the previous period's utilization at the same point in its cycle, for a
/// metric identified by a fixed string key (`five_hour` / `seven_day` / …).
fn find_prev_utilization(
    history: &[SnapshotEntry],
    metric: &str,
    current_resets_at: &str,
    now_ms: i64,
) -> Option<f64> {
    find_prev_utilization_by(
        history,
        |e| get_metric_snap(e, metric),
        period_ms(metric),
        current_resets_at,
        now_ms,
    )
}

/// Core of the "vs previous period" comparison, generic over how each snapshot
/// exposes the metric of interest. `accessor` pulls the relevant `MetricSnap`
/// out of a snapshot (fixed field for the built-in metrics, label lookup for
/// scoped models); `pms` is the window length in ms. Factored out so the scoped
/// per-model windows reuse the exact same period-alignment logic as the fixed
/// five-hour / seven-day metrics.
fn find_prev_utilization_by<'a>(
    history: &'a [SnapshotEntry],
    accessor: impl Fn(&'a SnapshotEntry) -> Option<&'a MetricSnap>,
    pms: i64,
    current_resets_at: &str,
    now_ms: i64,
) -> Option<f64> {
    let current_reset_ms = parse_ts_ms(current_resets_at)?;
    let current_start_ms = current_reset_ms - pms;
    let current_frac =
        ((now_ms - current_start_ms) as f64 / pms as f64).clamp(0.0, 1.0);

    let mut prev_resets: Vec<String> = history
        .iter()
        .filter_map(|e| accessor(e))
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
            let snap = accessor(e)?;
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
        let cred_path = crate::session::get_claude_dir()
            .ok_or("No home dir")?
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
    // Windows: Claude Desktop App keeps its OAuth token DPAPI-encrypted under
    // `%APPDATA%\Claude\config.json#oauth:tokenCache`, NOT in
    // `~/.claude/.credentials.json` (which only the standalone Claude Code
    // CLI uses). Try the Desktop App store first so users who never install
    // the CLI can still authenticate; fall back to the file otherwise.
    #[cfg(windows)]
    if let Ok(creds) = read_desktop_app_credentials() {
        return Ok(creds);
    }

    let cred_path = crate::session::get_claude_dir()
        .ok_or("No home dir")?
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

/// Read `(accessToken, subscriptionType)` from Claude Desktop App's
/// DPAPI-encrypted `config.json` (Windows only). The Desktop App stores the
/// token under `oauth:tokenCache` as a base64-encoded Electron `safeStorage`
/// blob; the decrypted plaintext is a JSON document whose schema Anthropic
/// has not publicly documented, so we probe a few plausible shapes:
///
/// * `{ "claudeAiOauth": { "accessToken": …, "subscriptionType": … } }`
///   (CLI-style — what `~/.claude/.credentials.json` uses)
/// * `{ "accessToken": …, "subscriptionType": … }` (flat camelCase)
/// * `{ "access_token": …, "subscription_type": … }` (flat snake_case)
///
/// On an unrecognised shape the error message lists the top-level keys so
/// future schema changes can be diagnosed without re-decrypting.
#[cfg(windows)]
fn read_desktop_app_credentials() -> Result<(String, String), String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA env var not set".to_string())?;
    let config_path = std::path::PathBuf::from(appdata)
        .join("Claude")
        .join("config.json");

    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("read {}: {e}", config_path.display()))?;
    let cfg: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("parse {}: {e}", config_path.display()))?;

    let encoded = cfg
        .get("oauth:tokenCache")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Desktop App config.json missing `oauth:tokenCache`".to_string())?;

    let plain_bytes = crate::dpapi::decrypt_safe_storage(encoded)?;
    let plain_str = std::str::from_utf8(&plain_bytes)
        .map_err(|e| format!("decrypted blob not UTF-8: {e}"))?;
    let blob: Value = serde_json::from_str(plain_str)
        .map_err(|e| format!("decrypted blob not JSON: {e}"))?;

    let inner = blob.get("claudeAiOauth").unwrap_or(&blob);

    let token = inner
        .get("accessToken")
        .or_else(|| inner.get("access_token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            let keys: Vec<&str> = blob
                .as_object()
                .map(|o| o.keys().map(|k| k.as_str()).collect())
                .unwrap_or_default();
            format!("decrypted Desktop App blob has no accessToken; top-level keys: {keys:?}")
        })?
        .to_string();
    let sub = inner
        .get("subscriptionType")
        .or_else(|| inner.get("subscription_type"))
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

/// Extract per-model weekly-scoped usage from the usage API's `limits[]` array.
/// Each `kind == "weekly_scoped"` entry carries the model it caps under
/// `scope.model.display_name` (e.g. "Fable") and an integer `percent`. This is
/// where Anthropic moved the model-scoped weekly caps that used to surface as
/// the top-level `seven_day_sonnet` field (now always null). Entries without a
/// model display name or percent are skipped. `is_active` is deliberately not
/// filtered on — the old fixed bars showed regardless of which window was the
/// currently-binding one, and the utilization is meaningful either way.
pub(crate) fn parse_scoped_limits(v: &Value) -> Vec<ScopedUsage> {
    let Some(limits) = v.get("limits").and_then(|l| l.as_array()) else {
        return Vec::new();
    };
    limits
        .iter()
        .filter(|e| e.get("kind").and_then(|k| k.as_str()) == Some("weekly_scoped"))
        .filter_map(|e| {
            let model_label = e
                .pointer("/scope/model/display_name")
                .and_then(|d| d.as_str())?
                .to_string();
            let percent = e.get("percent").and_then(|p| p.as_f64())?;
            let resets_at = e
                .get("resets_at")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string();
            Some(ScopedUsage {
                model_label,
                utilization: percent / 100.0,
                resets_at,
                prev_utilization: None,
            })
        })
        .collect()
}

// ── Account info fetch ────────────────────────────────────────────────────────

/// Raw account fields before snapshot/prev-utilization processing. Shared
/// return shape for both the foxy-switcher path and the direct-Anthropic path
/// so the snapshot tail in `fetch_account_info` runs identically for either.
type RawAccount = (
    String,             // email
    String,             // full_name
    String,             // organization_name
    String,             // plan
    Option<UsageStats>, // five_hour
    Option<UsageStats>, // seven_day
    Vec<ScopedUsage>,   // seven_day_scoped (per-model weekly caps from limits[])
);

/// Direct path: read keychain credentials and fetch profile + usage straight
/// from Anthropic. Used as the fallback when foxy-switcher isn't reachable.
/// Map the `/api/oauth/profile` response (plus the credential's raw
/// `subscription_type`) to a human-readable plan label.
///
/// Personal plans come from `account.has_claude_max` / `has_claude_pro`.
/// Org-level plans come from `organization.organization_type` ("claude_team");
/// within a Team org, `organization.rate_limit_tier` distinguishes Premium
/// (Max-parity quota, tier contains "claude_max") from the standard tier.
/// Keep in sync with foxy-switcher's `anthropic.DerivePlan`.
fn derive_plan(profile_body: &Value, subscription_type: &str) -> String {
    let has_max = profile_body
        .pointer("/account/has_claude_max")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_pro = profile_body
        .pointer("/account/has_claude_pro")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let org_type = profile_body
        .pointer("/organization/organization_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let rate_tier = profile_body
        .pointer("/organization/rate_limit_tier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if has_max {
        // rate_limit_tier is the only field that splits personal Max 5x from
        // Max 20x — has_claude_max is true for both.
        match rate_tier {
            "default_claude_max_20x" => "Claude Max 20x".to_string(),
            "default_claude_max_5x" => "Claude Max 5x".to_string(),
            _ => "Claude Max".to_string(),
        }
    } else if has_pro || subscription_type == "pro" {
        "Claude Pro".to_string()
    } else if org_type == "claude_team" {
        // Premium gets Max-parity rate limits (tier contains "claude_max");
        // the standard Team tier reports pro-level limits.
        if rate_tier.contains("claude_max") {
            "Claude Team Premium".to_string()
        } else {
            "Claude Team".to_string()
        }
    } else {
        "API / Free".to_string()
    }
}

async fn fetch_via_anthropic() -> Result<RawAccount, String> {
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

    let plan = derive_plan(&profile_body, &subscription_type);

    let five_hour = usage_body.get("five_hour").and_then(|v| parse_usage(v));
    let seven_day = usage_body.get("seven_day").and_then(|v| parse_usage(v));
    let seven_day_scoped = parse_scoped_limits(&usage_body);

    Ok((email, full_name, org_name, plan, five_hour, seven_day, seven_day_scoped))
}

pub async fn fetch_account_info() -> Result<AccountInfo, String> {
    // Prefer foxy-switcher when its daemon is alive: it already polls Anthropic's
    // usage endpoint once a minute for the in-use account, so reading from it
    // avoids a redundant Anthropic call (and the rate limits that come with it).
    // foxy only exposes email + plan, so full_name falls back to the email and
    // organization_name is left blank. Any failure falls back to the direct API.
    let (usage_source, (email, full_name, organization_name, plan, mut five_hour, mut seven_day, mut seven_day_scoped)) =
        if let Some(f) = crate::foxy::fetch_in_use_account().await {
            (
                "foxy-switcher".to_string(),
                (f.email.clone(), f.email, String::new(), f.plan, f.five_hour, f.seven_day, f.seven_day_scoped),
            )
        } else {
            ("anthropic".to_string(), fetch_via_anthropic().await?)
        };

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
        // Sonnet-specific weekly window no longer exists in the API; scoped
        // models are sampled into `seven_day_scoped` below.
        seven_day_sonnet: None,
        seven_day_scoped: seven_day_scoped
            .iter()
            .map(|s| ScopedSnap {
                model_label: s.model_label.clone(),
                snap: MetricSnap { utilization: s.utilization, resets_at: s.resets_at.clone() },
            })
            .collect(),
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
    for sc in seven_day_scoped.iter_mut() {
        let ra = sc.resets_at.clone();
        let label = sc.model_label.clone();
        sc.prev_utilization = find_prev_utilization_by(
            &history,
            |e| get_scoped_snap(e, &label),
            period_ms("seven_day"),
            &ra,
            now_ms,
        );
    }

    Ok(AccountInfo {
        email,
        full_name,
        organization_name,
        plan,
        auth_method: "claudeai".to_string(),
        five_hour,
        seven_day,
        seven_day_scoped,
        usage_source,
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

/// Pick the newest snapshot out of an unsorted history slice.
fn latest_of(history: &[SnapshotEntry]) -> Option<UsageHistoryPoint> {
    history.iter().max_by_key(|e| e.ts).map(|e| UsageHistoryPoint {
        ts: e.ts,
        five_hour: e.five_hour.as_ref().map(|m| m.utilization),
        seven_day: e.seven_day.as_ref().map(|m| m.utilization),
        seven_day_sonnet: e.seven_day_sonnet.as_ref().map(|m| m.utilization),
    })
}

/// The most recent usage snapshot the background sampler has persisted, read
/// straight off disk with **no network call**. `ts` is the sample time in epoch
/// ms so callers can reject a stale reading; the utilization fields are the
/// 0–1 fraction (same convention as `load_usage_history`, NOT the 0–100 the
/// Anthropic API hands back — `normalize_snap` has already divided).
///
/// This is the read side of the auto-resume "has the account's limit actually
/// come back?" check (`auto_resume::limit_recovered`). Deliberately cache-only:
/// the sampler already refreshes on its own interval, and the auto-resume
/// ticker runs every 30s — hitting the usage API from that hot loop would be a
/// request amplifier for no benefit.
pub fn latest_usage_snapshot() -> Option<UsageHistoryPoint> {
    latest_of(&load_snapshots())
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
            seven_day_scoped: Vec::new(),
        }
    }

    #[test]
    fn latest_of_picks_newest_regardless_of_order() {
        assert!(latest_of(&[]).is_none());

        let mut newer = snap_at(2_000);
        newer.five_hour = Some(MetricSnap { utilization: 0.91, resets_at: String::new() });
        newer.seven_day = Some(MetricSnap { utilization: 0.12, resets_at: String::new() });
        // Deliberately out of order: the newest entry is not last.
        let history = vec![newer, snap_at(1_000), snap_at(500)];

        let got = latest_of(&history).expect("non-empty history has a latest");
        assert_eq!(got.ts, 2_000);
        assert_eq!(got.five_hour, Some(0.91));
        assert_eq!(got.seven_day, Some(0.12));
        assert_eq!(got.seven_day_sonnet, None);
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

    /// The real `/api/oauth/usage` shape as of 2026-07: scoped models live in
    /// `limits[]` (kind `weekly_scoped`, model under `scope.model.display_name`),
    /// while the top-level `seven_day_sonnet` field is null.
    #[test]
    fn parse_scoped_limits_extracts_weekly_scoped_models() {
        let body = serde_json::json!({
            "five_hour": { "utilization": 42.0, "resets_at": "2026-07-23T06:50:00+00:00" },
            "seven_day": { "utilization": 16.0, "resets_at": "2026-07-27T10:59:59+00:00" },
            "seven_day_sonnet": null,
            "limits": [
                { "kind": "session", "percent": 42, "resets_at": "2026-07-23T06:50:00+00:00", "scope": null },
                { "kind": "weekly_all", "percent": 16, "resets_at": "2026-07-27T10:59:59+00:00", "scope": null },
                { "kind": "weekly_scoped", "percent": 4, "resets_at": "2026-07-27T10:59:59+00:00",
                  "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null } }
            ]
        });
        let scoped = parse_scoped_limits(&body);
        assert_eq!(scoped.len(), 1, "only the weekly_scoped entry counts");
        assert_eq!(scoped[0].model_label, "Fable");
        assert!((scoped[0].utilization - 0.04).abs() < 1e-9, "4 percent → 0.04 fraction");
        assert_eq!(scoped[0].resets_at, "2026-07-27T10:59:59+00:00");
    }

    /// The real `/api/oauth/profile` shape for a Claude Team org: the personal
    /// `has_claude_*` flags are false and the plan lives under `organization`
    /// (`organization_type` + `rate_limit_tier`). Premium has Max-parity limits.
    #[test]
    fn derive_plan_recognizes_team_premium_and_team() {
        let premium = serde_json::json!({
            "account": { "has_claude_max": false, "has_claude_pro": false },
            "organization": { "organization_type": "claude_team", "rate_limit_tier": "default_claude_max_5x" }
        });
        assert_eq!(derive_plan(&premium, ""), "Claude Team Premium");

        let standard = serde_json::json!({
            "account": { "has_claude_max": false, "has_claude_pro": false },
            "organization": { "organization_type": "claude_team", "rate_limit_tier": "default_claude_pro" }
        });
        assert_eq!(derive_plan(&standard, ""), "Claude Team");
    }

    /// Personal Max/Pro and the genuine API/Free fallback must be preserved,
    /// and Max 20x/5x are split by rate_limit_tier.
    #[test]
    fn derive_plan_preserves_personal_and_free() {
        let max20 = serde_json::json!({
            "account": { "has_claude_max": true, "has_claude_pro": false },
            "organization": { "rate_limit_tier": "default_claude_max_20x" }
        });
        assert_eq!(derive_plan(&max20, ""), "Claude Max 20x");

        let pro = serde_json::json!({ "account": { "has_claude_max": false, "has_claude_pro": true } });
        assert_eq!(derive_plan(&pro, ""), "Claude Pro");

        // Credential subscription_type == "pro" still routes to Pro.
        let cred_pro = serde_json::json!({ "account": {} });
        assert_eq!(derive_plan(&cred_pro, "pro"), "Claude Pro");

        // No signals at all → the honest API/Free fallback.
        let free = serde_json::json!({ "account": {}, "organization": {} });
        assert_eq!(derive_plan(&free, ""), "API / Free");
    }

    #[test]
    fn parse_scoped_limits_absent_or_empty_yields_empty() {
        assert!(parse_scoped_limits(&serde_json::json!({})).is_empty());
        assert!(parse_scoped_limits(&serde_json::json!({ "limits": [] })).is_empty());
        // A weekly_scoped entry missing its model display name is skipped.
        let no_model = serde_json::json!({
            "limits": [{ "kind": "weekly_scoped", "percent": 9, "scope": { "model": {} } }]
        });
        assert!(parse_scoped_limits(&no_model).is_empty());
    }

    /// A scoped model's prev-period utilization is matched by model label
    /// through the same period-alignment logic as the fixed metrics.
    #[test]
    fn scoped_prev_utilization_matches_by_label() {
        let day = 24 * 3600 * 1000;
        let now = 100 * day;
        let prev_reset = "2026-01-08T00:00:00+00:00";
        let cur_reset = "2026-01-15T00:00:00+00:00";
        let prev_reset_ms = parse_ts_ms(prev_reset).unwrap();
        let cur_reset_ms = parse_ts_ms(cur_reset).unwrap();
        let week = period_ms("seven_day");
        // Two samples in the previous Fable window, one in the current window.
        let mk = |ts: i64, resets: &str, util: f64| SnapshotEntry {
            ts,
            five_hour: None,
            seven_day: None,
            seven_day_sonnet: None,
            seven_day_scoped: vec![ScopedSnap {
                model_label: "Fable".into(),
                snap: MetricSnap { utilization: util, resets_at: resets.into() },
            }],
        };
        // Current sample sits ~2 days into its window.
        let now2 = cur_reset_ms - week + 2 * day;
        let history = vec![
            mk(prev_reset_ms - week + 2 * day, prev_reset, 0.30), // prev, same cycle phase
            mk(prev_reset_ms - week + 5 * day, prev_reset, 0.55),
            mk(now2, cur_reset, 0.10),
        ];
        let prev = find_prev_utilization_by(
            &history,
            |e| get_scoped_snap(e, "Fable"),
            week,
            cur_reset,
            now2,
        );
        assert_eq!(prev, Some(0.30), "closest prev-window phase match");
        let _ = now;
    }
}
