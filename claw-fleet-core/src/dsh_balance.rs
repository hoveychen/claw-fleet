//! What is left to spend behind a dsh install, asked of the providers directly.
//!
//! # Why this cannot come from dsh
//!
//! dsh publishes no account, quota or balance of its own. Its `/api` catalog is
//! `agentPresets/* · credentials/* · goal.* · host.* · llm.* · session.* ·
//! settings.* · skill.list · subagent.* · workspace.*` — there is no
//! `account.*`, `usage.*` or `quota.*` (see [`crate::dsh_source`], which
//! measured the catalog against a live server). The near misses answer a
//! different question: `credentials/describe` reports whether a ref is *set*,
//! never its value or its balance; `llm.providers` lists the catalog with an
//! `active` flag and no limit at all.
//!
//! That follows from what dsh is — a bring-your-own-key harness. The money
//! belongs to whichever provider the user configured, so the only truthful
//! number comes from that provider's own API, reached with the key dsh already
//! stores.
//!
//! # What each provider answers (measured 2026-09-06, real keys)
//!
//! * **DeepSeek** — `GET https://api.deepseek.com/user/balance` returns
//!   `balance_infos[]`, one entry **per currency**, each with `total_balance` /
//!   `granted_balance` / `topped_up_balance` as *decimal strings*. A live
//!   account answered `[{USD, "0.00"…}, {CNY, total "175.05", topped_up
//!   "175.05"}]` — so an account can legitimately hold money in one currency
//!   and nothing in another, and picking a single entry blind would print
//!   `$0.00` for an account with ¥175 in it. Hence one row per funded currency.
//! * **OpenRouter** — two endpoints, deliberately both:
//!   - `GET /api/v1/credits` → `{total_credits, total_usage}` (USD). The
//!     difference is what the *account* has left.
//!   - `GET /api/v1/key` → `{limit, usage, limit_remaining}`. A per-key spend
//!     ceiling, when the key carries one. Measured: an account with ~26.5 left
//!     held a key capped at 350 with 331.14 used — **18.86 left on the key**.
//!     Reporting only the account balance would have overstated what that key
//!     can actually spend, so the ceiling rides along and is what the panel
//!     draws its bar from (it is the only real denominator in this module).
//!
//! # No cache
//!
//! Unlike [`crate::dsh_cost`], nothing here is written down. A per-call price is
//! a historical fact that must be frozen; a balance is a *current* position
//! whose whole value is being fresh. The usage panel's own refresh cadence
//! governs how often these are asked.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// dsh's provider id for the built-in DeepSeek route.
pub const DEEPSEEK_PROVIDER: &str = "deepseek-official";
/// dsh's provider id for the OpenRouter route (served by `llm-pi-ai`).
pub const OPENROUTER_PROVIDER: &str = "openrouter";

const DEEPSEEK_BALANCE_ENDPOINT: &str = "https://api.deepseek.com/user/balance";
const OPENROUTER_CREDITS_ENDPOINT: &str = "https://openrouter.ai/api/v1/credits";
const OPENROUTER_KEY_ENDPOINT: &str = "https://openrouter.ai/api/v1/key";

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// One provider's money position.
///
/// `balance` is an amount with no denominator — that is what a prepaid balance
/// *is*, and inventing a ceiling to draw a bar from would be a made-up number.
/// `limit`/`used` are populated only where the provider enforces a real one.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DshProviderBalance {
    /// dsh's provider id, e.g. `deepseek-official` / `openrouter`.
    pub provider: String,
    /// Display name for the row.
    pub label: String,
    /// Currency the amounts are in (`"CNY"` / `"USD"`), carried per row because
    /// the two providers do not agree on one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Money left. `None` when the lookup failed — see `error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    /// Spend ceiling on this key, when the provider enforces one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    /// Spend already counted against `limit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    /// Why this provider has no numbers. Present *instead of* silence, so an
    /// expired key reads as a failure rather than as a zero balance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The payload `get_source_usage { source: "dsh" }` answers with.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DshUsageItem {
    /// One row per funded currency per configured provider. Empty when dsh has
    /// no provider key at all, which is a normal state rather than an error.
    pub balances: Vec<DshProviderBalance>,
}

// ── Parsing (split from the HTTP calls so the contracts are testable) ────────

/// Pull `(currency, total_balance)` pairs out of DeepSeek's answer.
///
/// The amounts arrive as strings (`"175.05"`), not numbers — parsing them as
/// `f64` up front is what keeps the rest of the module in one numeric type.
/// An entry whose amount will not parse is dropped rather than counted as zero.
pub fn parse_deepseek_balances(body: &str) -> Result<Vec<(String, f64)>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let infos = v
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or_else(|| "no balance_infos in response".to_string())?;
    Ok(infos
        .iter()
        .filter_map(|info| {
            let currency = info.get("currency").and_then(Value::as_str)?.to_string();
            let amount = info
                .get("total_balance")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<f64>().ok())?;
            Some((currency, amount))
        })
        .collect())
}

/// Keep the currencies worth showing.
///
/// A DeepSeek account carries a row for every currency it *could* hold, most of
/// them zero. Showing all of them buries the funded one; showing none when
/// everything is zero would hide a genuinely empty account, which is exactly
/// when the number matters most. So: the funded ones, or — if there are none —
/// the first row, printed as the zero it is.
pub fn fund_bearing<'a>(entries: &'a [(String, f64)]) -> Vec<&'a (String, f64)> {
    let funded: Vec<&(String, f64)> = entries.iter().filter(|(_, amount)| *amount != 0.0).collect();
    if !funded.is_empty() {
        return funded;
    }
    entries.first().into_iter().collect()
}

/// `{total_credits, total_usage}` → what the account has left.
pub fn parse_openrouter_credits(body: &str) -> Result<f64, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let credits = v
        .pointer("/data/total_credits")
        .and_then(Value::as_f64)
        .ok_or_else(|| "no data.total_credits in response".to_string())?;
    let usage = v
        .pointer("/data/total_usage")
        .and_then(Value::as_f64)
        .ok_or_else(|| "no data.total_usage in response".to_string())?;
    Ok(credits - usage)
}

/// `{limit, usage}` → the key's own ceiling, when it has one.
///
/// `limit: null` is the ordinary shape for an uncapped key, so it yields
/// `Ok(None)` rather than an error: no ceiling is not a failure to read one.
pub fn parse_openrouter_key_limit(body: &str) -> Result<Option<(f64, f64)>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let data = v.get("data").ok_or_else(|| "no data in response".to_string())?;
    let Some(limit) = data.get("limit").and_then(Value::as_f64) else {
        return Ok(None);
    };
    let usage = data.get("usage").and_then(Value::as_f64).unwrap_or(0.0);
    Ok(Some((limit, usage)))
}

// ── HTTP ────────────────────────────────────────────────────────────────────

/// GET with a bearer key, off the async runtime.
///
/// The hop matters for the same reason it does in [`crate::dsh_client`]: this
/// runs under a tauri `(async)` command on a tokio worker, where a
/// `reqwest::blocking` call panics inside `wait::enter` and the panic is
/// swallowed — the invoke promise then never settles and the panel spins
/// forever. See [`crate::off_runtime`].
fn get_json(url: &str, key: &str) -> Result<String, String> {
    crate::off_runtime::off_runtime(|| {
        let client = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let resp = client
            .get(url)
            .bearer_auth(key)
            .send()
            .map_err(|e| format!("{e}"))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| format!("body: {e}"))?;
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        Ok(body)
    })?
}

// ── Per-provider rows ───────────────────────────────────────────────────────

/// Build DeepSeek's rows from an already-fetched body.
fn deepseek_rows(body: &str) -> Vec<DshProviderBalance> {
    match parse_deepseek_balances(body) {
        Ok(entries) => fund_bearing(&entries)
            .into_iter()
            .map(|(currency, amount)| DshProviderBalance {
                provider: DEEPSEEK_PROVIDER.to_string(),
                label: "DeepSeek".to_string(),
                currency: Some(currency.clone()),
                balance: Some(*amount),
                ..Default::default()
            })
            .collect(),
        Err(e) => vec![DshProviderBalance {
            provider: DEEPSEEK_PROVIDER.to_string(),
            label: "DeepSeek".to_string(),
            error: Some(e),
            ..Default::default()
        }],
    }
}

/// Build OpenRouter's row from the two already-fetched bodies.
///
/// The key ceiling is optional in both senses — the request may fail, and a key
/// may have none — so a missing one silently leaves `limit`/`used` empty rather
/// than failing the whole row. The account balance is the load-bearing number.
fn openrouter_row(credits_body: &str, key_body: Option<&str>) -> DshProviderBalance {
    let mut row = DshProviderBalance {
        provider: OPENROUTER_PROVIDER.to_string(),
        label: "OpenRouter".to_string(),
        currency: Some("USD".to_string()),
        ..Default::default()
    };
    match parse_openrouter_credits(credits_body) {
        Ok(balance) => row.balance = Some(balance),
        Err(e) => row.error = Some(e),
    }
    if let Some((limit, used)) = key_body.and_then(|b| parse_openrouter_key_limit(b).ok()).flatten()
    {
        row.limit = Some(limit);
        row.used = Some(used);
    }
    row
}

// ── Public entry point ──────────────────────────────────────────────────────

/// Ask every provider dsh holds a key for what is left.
///
/// A provider with no key configured contributes nothing — that is a normal
/// state (most installs use one of the two), not an error worth a row. A
/// provider whose key *is* configured but whose call fails contributes a row
/// carrying the failure, because there the user is entitled to know the number
/// is missing rather than zero.
pub fn fetch_balances() -> DshUsageItem {
    let mut balances = Vec::new();

    if let Some(key) = crate::dsh_cost::deepseek_api_key() {
        balances.extend(match get_json(DEEPSEEK_BALANCE_ENDPOINT, &key) {
            Ok(body) => deepseek_rows(&body),
            Err(e) => vec![DshProviderBalance {
                provider: DEEPSEEK_PROVIDER.to_string(),
                label: "DeepSeek".to_string(),
                error: Some(e),
                ..Default::default()
            }],
        });
    }

    if let Some(key) = crate::dsh_cost::openrouter_api_key() {
        let key_body = get_json(OPENROUTER_KEY_ENDPOINT, &key).ok();
        balances.push(match get_json(OPENROUTER_CREDITS_ENDPOINT, &key) {
            Ok(body) => openrouter_row(&body, key_body.as_deref()),
            Err(e) => DshProviderBalance {
                provider: OPENROUTER_PROVIDER.to_string(),
                label: "OpenRouter".to_string(),
                currency: Some("USD".to_string()),
                error: Some(e),
                ..Default::default()
            },
        });
    }

    DshUsageItem { balances }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim body from a live account (2026-09-06), key redacted out of the
    /// request only — the response carries no secret.
    const DEEPSEEK_BODY: &str = r#"{"is_available":true,"balance_infos":[
        {"currency":"USD","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"},
        {"currency":"CNY","total_balance":"175.05","granted_balance":"0.00","topped_up_balance":"175.05"}]}"#;

    const OPENROUTER_CREDITS_BODY: &str =
        r#"{"data":{"total_credits":1535,"total_usage":1508.476966502}}"#;

    const OPENROUTER_KEY_BODY: &str = r#"{"data":{"label":"sk-or-v1-568...d2e","limit":350,
        "limit_remaining":18.856440098000007,"usage":331.143559902,"is_free_tier":false}}"#;

    #[test]
    fn deepseek_amounts_are_strings_and_parse_per_currency() {
        let entries = parse_deepseek_balances(DEEPSEEK_BODY).unwrap();
        assert_eq!(
            entries,
            vec![("USD".to_string(), 0.0), ("CNY".to_string(), 175.05)]
        );
    }

    /// The funded currency must survive; the zero one must not crowd it out.
    /// This is the case that made a "take the first entry" reading wrong: the
    /// first entry on this real account is the empty USD one.
    #[test]
    fn only_funded_currencies_are_shown() {
        let rows = deepseek_rows(DEEPSEEK_BODY);
        assert_eq!(rows.len(), 1, "one funded currency");
        assert_eq!(rows[0].currency.as_deref(), Some("CNY"));
        assert_eq!(rows[0].balance, Some(175.05));
        assert!(rows[0].error.is_none());
    }

    /// An account with nothing anywhere still gets a row — "¥0.00" is the
    /// answer, and printing nothing would look like a lookup that never ran.
    #[test]
    fn an_empty_account_still_reports_a_zero_rather_than_nothing() {
        let body = r#"{"balance_infos":[{"currency":"CNY","total_balance":"0.00"}]}"#;
        let rows = deepseek_rows(body);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].balance, Some(0.0));
    }

    #[test]
    fn a_malformed_body_becomes_an_error_row_not_a_zero_balance() {
        let rows = deepseek_rows("not json");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].balance.is_none(), "no number invented");
        assert!(rows[0].error.is_some());
    }

    #[test]
    fn openrouter_balance_is_credits_minus_usage() {
        let left = parse_openrouter_credits(OPENROUTER_CREDITS_BODY).unwrap();
        assert!((left - 26.523033498).abs() < 1e-6, "left = {left}");
    }

    #[test]
    fn openrouter_key_ceiling_rides_along_with_the_account_balance() {
        let row = openrouter_row(OPENROUTER_CREDITS_BODY, Some(OPENROUTER_KEY_BODY));
        assert_eq!(row.limit, Some(350.0));
        assert_eq!(row.used, Some(331.143559902));
        assert!(row.balance.unwrap() > 0.0);
        assert!(row.error.is_none());
    }

    /// An uncapped key is not a failure to read a cap.
    #[test]
    fn a_key_with_no_ceiling_yields_no_limit_and_no_error() {
        let body = r#"{"data":{"label":"k","limit":null,"usage":12.5}}"#;
        assert_eq!(parse_openrouter_key_limit(body).unwrap(), None);
        let row = openrouter_row(OPENROUTER_CREDITS_BODY, Some(body));
        assert!(row.limit.is_none() && row.used.is_none());
        assert!(row.error.is_none());
    }

    /// The key endpoint failing must not take the account balance down with it.
    #[test]
    fn a_failed_key_lookup_still_reports_the_account_balance() {
        let row = openrouter_row(OPENROUTER_CREDITS_BODY, None);
        assert!(row.balance.is_some());
        assert!(row.limit.is_none());
        assert!(row.error.is_none());
    }
}
