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

use crate::account::UsageStats;
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
    pub seven_day_sonnet: Option<UsageStats>,
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
        seven_day_sonnet: acct.get("seven_day_sonnet").and_then(parse_window),
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
                    "seven_day_sonnet": { "utilization": 0,  "resets_at": "2026-06-03T02:00:00.868436+00:00" }
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
        assert_eq!(a.seven_day_sonnet.unwrap().utilization, 0.0);
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

    #[test]
    fn missing_window_yields_none_for_that_window() {
        let body = json!({
            "accounts": [{ "id": 1, "email": "x", "plan": "p",
                           "five_hour": { "utilization": 10, "resets_at": "" } }]
        });
        let a = map_in_use(&body, 1).unwrap();
        assert!(a.five_hour.is_some());
        assert!(a.seven_day.is_none());
        assert!(a.seven_day_sonnet.is_none());
    }
}
