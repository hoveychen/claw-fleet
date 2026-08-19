//! foxy-switcher local API integration.
//!
//! foxy-switcher (https://github.com/hoveychen/foxy-switcher) is an account
//! switcher that runs a local HTTP daemon on `127.0.0.1`, polling Anthropic's
//! usage endpoint once a minute for each pooled account and re-serving the
//! result. When that daemon is alive we read usage from it instead of calling
//! Anthropic ourselves — it already has the numbers, so a second poll is
//! redundant and would count against the same rate limits.
//!
//! Detection is best-effort: a missing port file or any failed/slow request
//! means "foxy not available", and the caller falls back to the direct
//! Anthropic path. See `account::fetch_account_info`.

use crate::account::{ScopedUsage, UsageStats};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// The in-use account's usage, sourced from a running foxy-switcher daemon.
/// foxy only exposes `email` + `plan` per account (no full_name / org), so the
/// caller fills those gaps itself.
pub struct FoxyAccount {
    pub email: String,
    pub plan: String,
    pub five_hour: Option<UsageStats>,
    pub seven_day: Option<UsageStats>,
    /// Per-model weekly-scoped windows. Best-effort: foxy re-serves Anthropic's
    /// usage numbers in its own transformed shape, and whether it passes through
    /// the `limits[]` array (where scoped models like Fable now live) is not
    /// verified here — foxy is a separate product. When foxy exposes a
    /// `limits[]` array on the account we parse it with the same
    /// `account::parse_scoped_limits`; otherwise this is empty and foxy users
    /// simply see no scoped bar (five_hour / seven_day are unaffected). The
    /// direct-Anthropic path is where scoped usage is authoritative.
    pub seven_day_scoped: Vec<ScopedUsage>,
}

/// foxy's data directory: `$FOXY_DATA_DIR` if set and non-empty, else
/// `~/.foxy-switcher`. The `port` file and SQLite db live here.
fn data_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("FOXY_DATA_DIR") {
        if !d.trim().is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    crate::session::real_home_dir().map(|h| h.join(".foxy-switcher"))
}

/// Read the daemon's current port from `<data_dir>/port`. The port is random
/// per launch, so this is read on every poll rather than cached. `None` means
/// the file is absent (daemon not running) or unparseable.
fn read_port() -> Option<u16> {
    let path = data_dir()?.join("port");
    std::fs::read_to_string(&path).ok()?.trim().parse::<u16>().ok()
}

/// Parse one `{ utilization, resets_at }` window. foxy reports `utilization`
/// on a 0–100 scale; we store the 0–1 fraction to match the direct-API path
/// (`account::parse_usage`).
fn parse_window(v: &Value) -> Option<UsageStats> {
    let utilization = v.get("utilization")?.as_f64()? / 100.0;
    let resets_at = v
        .get("resets_at")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(UsageStats { utilization, resets_at, prev_utilization: None })
}

/// From the `/api/accounts` body, pick the account whose `id` equals
/// `managed_id` (the in-use account from `/api/cred/status`) and map its three
/// usage windows. `None` if that account isn't present.
fn map_in_use(accounts_body: &Value, managed_id: i64) -> Option<FoxyAccount> {
    let acct = accounts_body
        .get("accounts")?
        .as_array()?
        .iter()
        .find(|a| a.get("id").and_then(|x| x.as_i64()) == Some(managed_id))?;
    Some(FoxyAccount {
        email: acct.get("email").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        plan: acct.get("plan").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        five_hour: acct.get("five_hour").and_then(parse_window),
        seven_day: acct.get("seven_day").and_then(parse_window),
        // Best-effort: parse scoped models if foxy passes through Anthropic's
        // `limits[]`; empty otherwise (see `FoxyAccount::seven_day_scoped`).
        seven_day_scoped: crate::account::parse_scoped_limits(acct),
    })
}

/// The in-use **Codex** account, sourced from a running foxy-switcher daemon.
///
/// foxy pools Codex accounts alongside Claude ones and tracks them under a
/// *separate* in-use pointer (`codex_managed_account_id` on
/// `/api/cred/status`), so this is a distinct lookup from
/// [`fetch_in_use_account`] rather than a variant of it.
pub struct FoxyCodexAccount {
    pub email: String,
    pub plan: String,
    pub full_name: String,
    pub primary: Option<crate::codex_source::CodexRateLimitWindow>,
    pub secondary: Option<crate::codex_source::CodexRateLimitWindow>,
}

/// Map one foxy account row onto Codex's window pair.
///
/// **The field names lie, deliberately.** foxy stores every provider's usage in
/// the same three columns it grew for Anthropic, so its `pollCodex`
/// (`server/refresh/usage.go`) writes Codex's `primary` window into the
/// `five_hour` column and `secondary` into `seven_day`:
///
/// ```go
/// p.st.SetUsage(ctx, a.ID, primaryUtil, primaryReset, secondaryUtil, secondaryReset, 0, "", "")
/// //                       └─ fhU      └─ fhR        └─ sdU         └─ sdR
/// ```
///
/// A Codex `primary` window is **weekly** (verified: `windowDurationMins`
/// 10080 in Fleet's own snapshots, `x-codex-primary-window-minutes: 10080` on
/// live responses), so reading foxy's `five_hour` as a five-hour window would
/// render a weekly bar as a 5h one. We therefore map by foxy's *column
/// contract*, never by field name.
///
/// Two other shape notes, both verified against the live daemon:
/// - `utilization` is already a 0–100 percent for Codex, matching
///   [`crate::codex_source::CodexRateLimitWindow::used_percent`]. No `/100`
///   here, unlike [`parse_window`] which feeds Claude's 0–1 fractions.
/// - `resets_at` is RFC3339 text; Codex's field is Unix epoch **seconds**.
fn map_in_use_codex(accounts_body: &Value, codex_managed_id: i64) -> Option<FoxyCodexAccount> {
    let acct = accounts_body
        .get("accounts")?
        .as_array()?
        .iter()
        .find(|a| a.get("id").and_then(|x| x.as_i64()) == Some(codex_managed_id))?;
    let text = |key: &str| {
        acct.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    Some(FoxyCodexAccount {
        email: text("email"),
        plan: text("plan"),
        full_name: text("full_name"),
        primary: acct.get("five_hour").and_then(parse_codex_window),
        secondary: acct.get("seven_day").and_then(parse_codex_window),
    })
}

/// Convert one of foxy's `{ utilization, resets_at }` rows into a Codex window.
/// `None` when the row is absent or null — foxy omits a window entirely when the
/// plan has none (a Codex Team account reports no `secondary`).
fn parse_codex_window(v: &Value) -> Option<crate::codex_source::CodexRateLimitWindow> {
    if v.is_null() {
        return None;
    }
    let used_percent = v.get("utilization")?.as_f64()? as i32;
    let resets_at = v
        .get("resets_at")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp());
    Some(crate::codex_source::CodexRateLimitWindow {
        used_percent,
        // foxy stores no window length for any provider, so this stays unknown
        // rather than being inferred from the column it arrived in.
        window_duration_mins: None,
        resets_at,
    })
}

/// Fetch the in-use account's usage from a running foxy daemon, or `None` if
/// foxy isn't reachable. Two requests: `/api/cred/status` (cheap, instant) for
/// the in-use account id, then `/api/accounts` (can take a few seconds) for the
/// usage. `/api/accounts` gets a generous timeout per foxy's own guidance.
pub async fn fetch_in_use_account() -> Option<FoxyAccount> {
    let port = read_port()?;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let status: Value = client
        .get(format!("{base}/api/cred/status"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let managed_id = status.get("managed_account_id")?.as_i64()?;

    let accounts: Value = client
        .get(format!("{base}/api/accounts"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    map_in_use(&accounts, managed_id)
}

/// Fetch the in-use **Codex** account from a running foxy daemon, or `None` if
/// foxy isn't reachable / holds no Codex account.
///
/// Same two-request shape as [`fetch_in_use_account`], but keyed off
/// `codex_managed_account_id`. That field is absent whenever foxy has no Codex
/// remote configured, which is why a missing key is a plain `None` rather than
/// an error: "foxy manages no Codex account here" is an ordinary state, and the
/// caller falls back to asking Codex itself.
pub async fn fetch_in_use_codex_account() -> Option<FoxyCodexAccount> {
    let port = read_port()?;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let status: Value = client
        .get(format!("{base}/api/cred/status"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    // Deliberately NOT `managed_account_id` — that one names the in-use Claude
    // account. The two pointers are independent.
    let codex_managed_id = status.get("codex_managed_account_id")?.as_i64()?;
    // foxy reports 0 when a Codex remote exists but holds no lease.
    if codex_managed_id == 0 {
        return None;
    }

    let accounts: Value = client
        .get(format!("{base}/api/accounts"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    map_in_use_codex(&accounts, codex_managed_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_accounts() -> Value {
        json!({
            "accounts": [
                {
                    "id": 1,
                    "email": "you@example.com",
                    "plan": "Claude Max 20x",
                    "five_hour":        { "utilization": 42, "resets_at": "2026-05-27T08:20:00.868404+00:00" },
                    "seven_day":        { "utilization": 5,  "resets_at": "2026-06-03T02:00:00.868427+00:00" },
                    "limits": [
                        { "kind": "weekly_scoped", "percent": 7, "resets_at": "2026-06-03T02:00:00.868436+00:00",
                          "scope": { "model": { "display_name": "Fable" } } }
                    ]
                },
                {
                    "id": 2,
                    "email": "other@example.com",
                    "plan": "Claude Max 5x",
                    "five_hour":        { "utilization": 90, "resets_at": "2026-05-27T09:00:00+00:00" },
                    "seven_day":        { "utilization": 80, "resets_at": "2026-06-04T02:00:00+00:00" },
                    "seven_day_sonnet": { "utilization": 70, "resets_at": "2026-06-04T02:00:00+00:00" }
                }
            ]
        })
    }

    #[test]
    fn picks_in_use_account_by_managed_id() {
        let a = map_in_use(&sample_accounts(), 2).expect("account 2 present");
        assert_eq!(a.email, "other@example.com");
        assert_eq!(a.plan, "Claude Max 5x");
    }

    #[test]
    fn converts_utilization_from_percent_to_fraction() {
        // 42 on foxy's 0–100 scale must become 0.42, matching the direct-API path.
        let a = map_in_use(&sample_accounts(), 1).unwrap();
        assert!((a.five_hour.unwrap().utilization - 0.42).abs() < 1e-9);
        assert!((a.seven_day.unwrap().utilization - 0.05).abs() < 1e-9);
    }

    #[test]
    fn parses_scoped_models_from_limits_when_present() {
        // When foxy passes through Anthropic's `limits[]`, the scoped model is
        // parsed just like the direct path (7 percent → 0.07 fraction).
        let a = map_in_use(&sample_accounts(), 1).unwrap();
        assert_eq!(a.seven_day_scoped.len(), 1);
        assert_eq!(a.seven_day_scoped[0].model_label, "Fable");
        assert!((a.seven_day_scoped[0].utilization - 0.07).abs() < 1e-9);
    }

    #[test]
    fn preserves_resets_at() {
        let a = map_in_use(&sample_accounts(), 1).unwrap();
        assert_eq!(
            a.five_hour.unwrap().resets_at,
            "2026-05-27T08:20:00.868404+00:00"
        );
    }

    #[test]
    fn missing_managed_account_yields_none() {
        assert!(map_in_use(&sample_accounts(), 999).is_none());
    }

    // ── Codex accounts ───────────────────────────────────────────────────────
    //
    // Shapes copied from the live daemon (`GET /api/accounts`) on 2026-08-19:
    // account 48 is the in-use Claude account, 66 the in-use Codex one. Note
    // the Codex row carries `five_hour` but no `seven_day` — a Codex Team plan
    // has only a weekly (primary) window, and foxy omits empty ones.
    fn sample_mixed_accounts() -> Value {
        json!({
            "accounts": [
                {
                    "id": 48,
                    "provider": "claude",
                    "email": "pool@example.com",
                    "plan": "Claude Team Premium",
                    "five_hour": { "utilization": 3, "resets_at": "2026-08-19T23:20:00+00:00" },
                    "seven_day": { "utilization": 48, "resets_at": "2026-08-21T18:00:00+00:00" }
                },
                {
                    "id": 66,
                    "provider": "codex",
                    "email": "you@example.com",
                    "full_name": "Harry C",
                    "plan": "Codex Team",
                    // foxy's `five_hour` column, but the value is Codex's
                    // *weekly* primary window: this resets_at is the same
                    // instant codex's own `x-codex-primary-reset-at` header
                    // reported (1787622735, one second's rounding apart).
                    "five_hour": { "utilization": 1, "resets_at": "2026-08-25T01:52:16+00:00" }
                }
            ]
        })
    }

    #[test]
    fn codex_five_hour_column_maps_onto_the_primary_window() {
        let a = map_in_use_codex(&sample_mixed_accounts(), 66).expect("codex account 66 present");
        let primary = a.primary.expect("primary window mapped from the five_hour column");
        assert_eq!(primary.used_percent, 1);
        assert_eq!(primary.resets_at, Some(1_787_622_736));
    }

    #[test]
    fn codex_utilization_stays_a_percent() {
        // Claude's windows are stored as 0–1 fractions (`parse_window` divides
        // by 100); Codex's `used_percent` is already 0–100. Dividing here would
        // report a 1% weekly window as 0%.
        let a = map_in_use_codex(&sample_mixed_accounts(), 66).unwrap();
        assert_eq!(a.primary.unwrap().used_percent, 1);
    }

    #[test]
    fn codex_window_duration_is_left_unknown() {
        // foxy stores no window length, and guessing "five_hour column ⇒ 300
        // minutes" is exactly the mislabel this mapping exists to avoid.
        let a = map_in_use_codex(&sample_mixed_accounts(), 66).unwrap();
        assert_eq!(a.primary.unwrap().window_duration_mins, None);
    }

    #[test]
    fn codex_absent_seven_day_column_yields_no_secondary() {
        let a = map_in_use_codex(&sample_mixed_accounts(), 66).unwrap();
        assert!(a.secondary.is_none());
    }

    #[test]
    fn codex_seven_day_column_maps_onto_the_secondary_window() {
        let body = json!({
            "accounts": [{
                "id": 7, "provider": "codex", "email": "x", "plan": "Codex Plus",
                "five_hour": { "utilization": 10, "resets_at": "2026-08-25T01:52:16+00:00" },
                "seven_day": { "utilization": 25, "resets_at": "2026-08-26T10:00:00+00:00" }
            }]
        });
        let a = map_in_use_codex(&body, 7).unwrap();
        let secondary = a.secondary.expect("secondary mapped from the seven_day column");
        assert_eq!(secondary.used_percent, 25);
        assert_eq!(secondary.resets_at, Some(1_787_738_400));
    }

    #[test]
    fn codex_identity_fields_come_through() {
        let a = map_in_use_codex(&sample_mixed_accounts(), 66).unwrap();
        assert_eq!(a.email, "you@example.com");
        assert_eq!(a.plan, "Codex Team");
        assert_eq!(a.full_name, "Harry C");
    }

    #[test]
    fn codex_and_claude_pointers_select_different_accounts() {
        // The two in-use pointers are independent: `managed_account_id` names
        // the Claude account, `codex_managed_account_id` the Codex one. Reading
        // the Claude pointer for Codex is the bug this guards.
        let body = sample_mixed_accounts();
        assert_eq!(map_in_use(&body, 48).unwrap().plan, "Claude Team Premium");
        assert_eq!(map_in_use_codex(&body, 66).unwrap().plan, "Codex Team");
    }

    #[test]
    fn missing_codex_account_yields_none() {
        assert!(map_in_use_codex(&sample_mixed_accounts(), 999).is_none());
    }

    #[test]
    fn missing_window_yields_none_for_that_window() {
        let body = json!({
            "accounts": [{ "id": 1, "email": "x", "plan": "p",
                           "five_hour": { "utilization": 10, "resets_at": "" } }]
        });
        let a = map_in_use(&body, 1).unwrap();
        assert!(a.five_hour.is_some());
        assert!(a.seven_day.is_none());
        assert!(a.seven_day_scoped.is_empty());
    }
}
