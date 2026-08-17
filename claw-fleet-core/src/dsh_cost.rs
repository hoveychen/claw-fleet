//! Real per-session spend for dsh sessions, read from the provider rather than
//! inferred from token counts.
//!
//! # Why not price the tokens
//!
//! Fleet's own price table ([`crate::model_cost`]) knows Claude and GPT tiers.
//! dsh routes through an open model space — OpenRouter alone fronts dozens of
//! providers — and **the same model costs different amounts through different
//! providers**, so multiplying dsh's token counts by any table Fleet holds would
//! produce a confident wrong number. A wrong number is worse than none.
//!
//! So this asks the provider what it actually charged.
//!
//! # How a session's calls are found (measured)
//!
//! Every successful model call leaves a durable `assistant/message` event whose
//! `data.message.source` is
//! `{kind: "model", provider, model, replayState: {responseId, …}}`. Measured
//! against a real 120-event session: three model calls, three distinct
//! `responseId`s of the form `gen-1786757116-pDmWgCIKXi2AWZCPaQTI`, each tagged
//! `provider: "openrouter"`. The same id also rides the preceding
//! `assistant/chunk` `finish` frame; this module reads it off the **message**
//! because that is the durable record and there is exactly one per call.
//!
//! A session's real spend is therefore the sum over its `responseId`s.
//!
//! # OpenRouter
//!
//! `GET https://openrouter.ai/api/v1/generation?id=<gen-id>` with a bearer key
//! returns `data.total_cost` in **USD** (not credits). A generation record is
//! immutable once written, which is what makes the on-disk cache sound: an id
//! priced once never needs asking again.
//!
//! Calls made through any other provider are counted but not priced — dsh's
//! other adapters expose no cost endpoint. The panel says how many were left
//! out rather than quietly under-reporting the total.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The provider whose cost API this module speaks.
const OPENROUTER: &str = "openrouter";

const GENERATION_ENDPOINT: &str = "https://openrouter.ai/api/v1/generation";

/// Cache file under `~/.fleet`. Generation records are immutable, so an entry
/// never expires.
const CACHE_FILE: &str = "dsh-generation-cost.json";

/// A single billable model call found in a session's durable log.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationRef {
    /// The provider's own id for this call (`gen-…` on OpenRouter).
    pub id: String,
    /// `openrouter`, `deepseek-official`, …
    pub provider: String,
    pub model: String,
}

/// What a dsh session actually cost, plus an honest account of what could not
/// be priced.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshSessionCost {
    /// Summed `total_cost` over every priced call, in USD. `None` when nothing
    /// could be priced at all — rendered as "unavailable", never as `$0.00`.
    pub total_usd: Option<f64>,
    /// Model calls whose real cost was obtained.
    pub priced_calls: u32,
    /// OpenRouter calls the API would not price (unknown id, request failed).
    pub unpriced_calls: u32,
    /// Calls through a provider with no cost API — counted, never guessed at.
    pub unpriceable_calls: u32,
    /// Why the total is absent or partial, for the panel to show verbatim.
    /// Empty when every call was priced.
    pub note: String,
}

// ── Extraction ───────────────────────────────────────────────────────────────

/// Pull one [`GenerationRef`] per successful model call out of a session's
/// durable events (the array [`crate::dsh_source`] gets from `session.history`).
///
/// Pure and order-preserving so it can be tested against a recorded log. Ids
/// repeat across a retried step, so duplicates collapse to the first sighting —
/// paying twice for one generation would inflate the total.
pub fn generation_refs(events: &[Value]) -> Vec<GenerationRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some("assistant/message") {
            continue;
        }
        let Some(source) = event.pointer("/data/message/source") else {
            continue;
        };
        if source.get("kind").and_then(Value::as_str) != Some("model") {
            continue;
        }
        let Some(id) = source
            .pointer("/replayState/responseId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        out.push(GenerationRef {
            id: id.to_string(),
            provider: source
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: source
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    out
}

// ── Credentials ──────────────────────────────────────────────────────────────

/// Resolve the OpenRouter key **the way dsh itself resolves it**, so a user who
/// already configured dsh does not configure Fleet a second time.
///
/// dsh's `settings.yaml` declares the provider under
/// `llm-pi-ai.providers.openrouter`, either as an inline `apiKey` or — the shape
/// on a real install — as `apiKeyEnv: OPENROUTER_API_KEY`, naming the variable
/// to read. `.credentials.yaml` is the other store dsh writes. All three are
/// tried; the env var named by `apiKeyEnv` wins because that is what dsh uses
/// when both are present.
///
/// Returns `None` when no key is configured, which is a normal state (the user
/// may not use OpenRouter at all), not an error.
pub fn openrouter_api_key() -> Option<String> {
    let dsh_home = crate::session::get_dsh_dir()?;

    let settings = read_yaml(&dsh_home.join("settings.yaml"));
    if let Some(settings) = settings.as_ref() {
        let provider = settings
            .pointer("/llm-pi-ai/providers/openrouter")
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(var) = provider.get("apiKeyEnv").and_then(Value::as_str) {
            if let Some(key) = non_empty(std::env::var(var).ok()) {
                return Some(key);
            }
        }
        if let Some(key) = provider.get("apiKey").and_then(Value::as_str) {
            if let Some(key) = non_empty(Some(key.to_string())) {
                return Some(key);
            }
        }
    }

    // `.credentials.yaml` is a flat map of provider → credential. An empty `{}`
    // (the shipped default) yields nothing.
    let creds = read_yaml(&dsh_home.join(".credentials.yaml"))?;
    let entry = creds.get(OPENROUTER)?;
    let key = match entry {
        Value::String(s) => Some(s.clone()),
        other => other
            .get("apiKey")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    non_empty(key)
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Read a YAML file into a `serde_json::Value`, or `None` when absent/unparseable.
fn read_yaml(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str::<Value>(&text).ok()
}

// ── Cost cache ───────────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(CACHE_FILE))
}

#[derive(Serialize, Deserialize, Default)]
struct CostCache {
    /// generation id → cost in USD.
    #[serde(default)]
    costs: BTreeMap<String, f64>,
}

fn load_cache() -> CostCache {
    let Some(path) = cache_path() else {
        return CostCache::default();
    };
    match crate::atomic_json::load_preserving(&path) {
        crate::atomic_json::JsonLoad::Loaded(c) => c,
        _ => CostCache::default(),
    }
}

/// Merge `fresh` into the on-disk cache under the cross-process file lock, so a
/// second Fleet process pricing another session cannot drop these entries.
fn store_cache(fresh: &BTreeMap<String, f64>) {
    if fresh.is_empty() {
        return;
    }
    let Some(path) = cache_path() else { return };
    crate::atomic_json::with_file_lock(&path, || {
        let mut cache: CostCache = match crate::atomic_json::load_preserving(&path) {
            crate::atomic_json::JsonLoad::Loaded(c) => c,
            crate::atomic_json::JsonLoad::Unreadable => return,
            _ => CostCache::default(),
        };
        cache.costs.extend(fresh.iter().map(|(k, v)| (k.clone(), *v)));
        if let Ok(bytes) = serde_json::to_vec_pretty(&cache) {
            if let Err(e) = crate::atomic_json::write_atomic(&path, &bytes) {
                crate::log_debug(&format!("dsh cost cache: write: {e}"));
            }
        }
    });
}

// ── OpenRouter ───────────────────────────────────────────────────────────────

/// Parse `data.total_cost` (USD) out of a `/generation` answer.
///
/// Split from the HTTP call so the contract is testable without a key: the
/// endpoint's own shape is the thing most likely to drift.
fn parse_total_cost(body: &str) -> Result<f64, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    v.pointer("/data/total_cost")
        .and_then(Value::as_f64)
        .ok_or_else(|| "no data.total_cost in response".to_string())
}

fn fetch_generation_cost(key: &str, id: &str) -> Result<f64, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(GENERATION_ENDPOINT)
        .query(&[("id", id)])
        .bearer_auth(key)
        .send()
        .map_err(|e| format!("{id}: {e}"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| format!("{id} body: {e}"))?;
    if !status.is_success() {
        return Err(format!("{id}: HTTP {status}"));
    }
    parse_total_cost(&body)
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Price a session's model calls, given the refs already extracted from its log.
///
/// Split from [`dsh_session_cost`] so the accounting — which calls are priced,
/// cached, skipped, or failed — is testable without any network. `price` stands
/// in for the OpenRouter call.
fn tally(
    refs: &[GenerationRef],
    cached: &BTreeMap<String, f64>,
    mut price: impl FnMut(&str) -> Result<f64, String>,
) -> (DshSessionCost, BTreeMap<String, f64>) {
    let mut fresh = BTreeMap::new();
    let mut total = 0.0f64;
    let mut priced = 0u32;
    let mut unpriced = 0u32;
    let mut unpriceable = 0u32;
    let mut first_error: Option<String> = None;

    for r in refs {
        if r.provider != OPENROUTER {
            unpriceable += 1;
            continue;
        }
        if let Some(cost) = cached.get(&r.id) {
            total += cost;
            priced += 1;
            continue;
        }
        match price(&r.id) {
            Ok(cost) => {
                fresh.insert(r.id.clone(), cost);
                total += cost;
                priced += 1;
            }
            Err(e) => {
                unpriced += 1;
                first_error.get_or_insert(e);
            }
        }
    }

    let mut notes = Vec::new();
    if unpriceable > 0 {
        notes.push(format!(
            "{unpriceable} call(s) went through a provider with no cost API and are not included"
        ));
    }
    if unpriced > 0 {
        let detail = first_error.unwrap_or_default();
        notes.push(format!("{unpriced} call(s) could not be priced: {detail}"));
    }

    (
        DshSessionCost {
            total_usd: (priced > 0).then_some(total),
            priced_calls: priced,
            unpriced_calls: unpriced,
            unpriceable_calls: unpriceable,
            note: notes.join("; "),
        },
        fresh,
    )
}

/// Real spend for a `dsh://` session URI.
///
/// Unlike [`crate::dsh_source::dsh_token_breakdown`] this needs the full
/// `session.history` — the generation ids live in the events, not in the
/// `session.list` projections — and it may go to the network, so the panel
/// fetches it separately from the (cheap, local) token counts rather than
/// making the token view wait on it.
pub fn dsh_session_cost(uri: &str) -> Result<DshSessionCost, String> {
    let events = crate::dsh_source::session_events(uri)?;
    let refs = generation_refs(&events);
    if refs.is_empty() {
        return Ok(DshSessionCost {
            note: "no model calls in this session yet".to_string(),
            ..Default::default()
        });
    }

    let needs_openrouter = refs.iter().any(|r| r.provider == OPENROUTER);
    let key = openrouter_api_key();
    if needs_openrouter && key.is_none() {
        return Ok(DshSessionCost {
            unpriced_calls: refs.iter().filter(|r| r.provider == OPENROUTER).count() as u32,
            unpriceable_calls: refs.iter().filter(|r| r.provider != OPENROUTER).count() as u32,
            note: "no OpenRouter API key configured — set `OPENROUTER_API_KEY` \
                   (the variable dsh's settings.yaml names) or put the key in \
                   ~/.dsh/.credentials.yaml"
                .to_string(),
            ..Default::default()
        });
    }

    let cached = load_cache().costs;
    let (cost, fresh) = tally(&refs, &cached, |id| match key.as_deref() {
        Some(key) => fetch_generation_cost(key, id),
        None => Err("no OpenRouter API key configured".to_string()),
    });
    store_cache(&fresh);
    Ok(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Two model calls plus noise, shaped like the real log: the `assistant/chunk`
    /// finish frame carries the same id as its `assistant/message`, and a step can
    /// repeat an id.
    fn recorded_events() -> Vec<Value> {
        vec![
            json!({"type": "user/message", "data": {"message": {"role": "user"}}}),
            json!({"type": "assistant/chunk", "data": {"chunk": {
                "type": "finish",
                "replayState": {"provider": "openrouter", "responseId": "gen-1"}
            }}}),
            json!({"type": "assistant/message", "data": {"message": {
                "role": "assistant",
                "source": {
                    "kind": "model",
                    "provider": "openrouter",
                    "model": "anthropic/claude-haiku-4.5",
                    "replayState": {"responseId": "gen-1"}
                }
            }}}),
            // A duplicate of the same generation — must not be counted twice.
            json!({"type": "assistant/message", "data": {"message": {
                "source": {
                    "kind": "model",
                    "provider": "openrouter",
                    "model": "anthropic/claude-haiku-4.5",
                    "replayState": {"responseId": "gen-1"}
                }
            }}}),
            json!({"type": "assistant/message", "data": {"message": {
                "source": {
                    "kind": "model",
                    "provider": "deepseek-official",
                    "model": "deepseek-chat",
                    "replayState": {"responseId": "gen-2"}
                }
            }}}),
            // A Fleet-injected instruction message: durable, but not a model call.
            json!({"type": "user/message", "data": {"message": {
                "source": {"kind": "agent-instructions"}
            }}}),
        ]
    }

    #[test]
    fn extracts_one_ref_per_model_call_and_dedupes() {
        let refs = generation_refs(&recorded_events());
        assert_eq!(
            refs,
            vec![
                GenerationRef {
                    id: "gen-1".into(),
                    provider: "openrouter".into(),
                    model: "anthropic/claude-haiku-4.5".into(),
                },
                GenerationRef {
                    id: "gen-2".into(),
                    provider: "deepseek-official".into(),
                    model: "deepseek-chat".into(),
                },
            ],
            "one ref per distinct responseId, chunk frames and non-model sources ignored"
        );
    }

    #[test]
    fn extraction_tolerates_a_log_with_nothing_to_price() {
        assert!(generation_refs(&[]).is_empty());
        assert!(generation_refs(&[json!({"type": "assistant/message", "data": {}})]).is_empty());
        assert!(
            generation_refs(&[json!({"type": "assistant/message", "data": {"message": {
                "source": {"kind": "model", "provider": "openrouter", "replayState": {}}
            }}})])
            .is_empty(),
            "a model message with no responseId is not billable to anything"
        );
    }

    #[test]
    fn total_cost_is_read_from_the_documented_field_in_usd() {
        let body = r#"{"data":{"id":"gen-1","total_cost":0.00123,"tokens_prompt":10}}"#;
        assert_eq!(parse_total_cost(body).unwrap(), 0.00123);
    }

    #[test]
    fn a_response_without_total_cost_is_an_error_not_a_zero() {
        // Pricing a call at $0 because the field moved would silently under-report
        // the session — the whole failure this module exists to avoid.
        assert!(parse_total_cost(r#"{"data":{"id":"gen-1"}}"#).is_err());
        assert!(parse_total_cost("not json").is_err());
    }

    #[test]
    fn tally_sums_priced_calls_and_reports_the_rest() {
        let refs = generation_refs(&recorded_events());
        let (cost, fresh) = tally(&refs, &BTreeMap::new(), |id| {
            assert_eq!(id, "gen-1", "only the openrouter call is priced");
            Ok(0.25)
        });
        assert_eq!(cost.total_usd, Some(0.25));
        assert_eq!(cost.priced_calls, 1);
        assert_eq!(cost.unpriceable_calls, 1, "the deepseek call is counted, not guessed");
        assert_eq!(cost.unpriced_calls, 0);
        assert!(
            cost.note.contains("no cost API"),
            "the excluded call must be visible in the note: {}",
            cost.note
        );
        assert_eq!(fresh.get("gen-1"), Some(&0.25), "the new price is cached");
    }

    #[test]
    fn tally_reuses_the_cache_and_never_refetches() {
        let refs = generation_refs(&recorded_events());
        let cached = BTreeMap::from([("gen-1".to_string(), 0.5)]);
        let (cost, fresh) = tally(&refs, &cached, |id| {
            panic!("must not fetch a cached id: {id}");
        });
        assert_eq!(cost.total_usd, Some(0.5));
        assert!(fresh.is_empty(), "nothing new to write back");
    }

    #[test]
    fn tally_surfaces_a_failed_lookup_instead_of_dropping_it() {
        let refs = vec![GenerationRef {
            id: "gen-x".into(),
            provider: "openrouter".into(),
            model: "m".into(),
        }];
        let (cost, fresh) = tally(&refs, &BTreeMap::new(), |_| Err("HTTP 404".into()));
        assert_eq!(cost.total_usd, None, "nothing priced → no total, not $0");
        assert_eq!(cost.unpriced_calls, 1);
        assert!(cost.note.contains("HTTP 404"), "note: {}", cost.note);
        assert!(fresh.is_empty());
    }

    // --- key resolution, against a temp DSH_HOME ---

    fn with_temp_dsh_home<T>(f: impl FnOnce(&PathBuf) -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let base = std::env::temp_dir().join(format!("fleet-dsh-cost-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let prev = std::env::var_os("DSH_HOME");
        std::env::set_var("DSH_HOME", &base);
        let out = f(&base);
        match prev {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
        let _ = std::fs::remove_dir_all(&base);
        out
    }

    #[test]
    fn key_comes_from_the_env_var_settings_yaml_names() {
        with_temp_dsh_home(|base| {
            // The shape on a real install.
            std::fs::write(
                base.join("settings.yaml"),
                "llm-pi-ai:\n  providers:\n    openrouter:\n      apiKeyEnv: FLEET_TEST_OR_KEY\n",
            )
            .unwrap();
            std::env::remove_var("FLEET_TEST_OR_KEY");
            assert_eq!(openrouter_api_key(), None, "named var unset → no key");

            std::env::set_var("FLEET_TEST_OR_KEY", "sk-or-test");
            assert_eq!(openrouter_api_key().as_deref(), Some("sk-or-test"));
            std::env::remove_var("FLEET_TEST_OR_KEY");
        });
    }

    #[test]
    fn key_falls_back_to_inline_settings_then_credentials() {
        with_temp_dsh_home(|base| {
            std::fs::write(
                base.join("settings.yaml"),
                "llm-pi-ai:\n  providers:\n    openrouter:\n      apiKey: inline-key\n",
            )
            .unwrap();
            assert_eq!(openrouter_api_key().as_deref(), Some("inline-key"));

            std::fs::write(base.join("settings.yaml"), "llm-pi-ai: {}\n").unwrap();
            std::fs::write(base.join(".credentials.yaml"), "openrouter: cred-key\n").unwrap();
            assert_eq!(openrouter_api_key().as_deref(), Some("cred-key"));
        });
    }

    #[test]
    fn an_unconfigured_install_yields_no_key_rather_than_an_error() {
        with_temp_dsh_home(|base| {
            // Exactly what ships: an empty credentials map, no provider settings.
            std::fs::write(base.join(".credentials.yaml"), "{}\n").unwrap();
            assert_eq!(openrouter_api_key(), None);
        });
    }
}
