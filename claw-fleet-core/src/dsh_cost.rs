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
//!
//! # A fresh generation is not queryable right away (measured)
//!
//! `/generation` answers `404 Generation … not found` for an id that was just
//! produced, and starts answering with the real record some time later.
//! Measured on one pair of ids: still 404 at roughly one minute and again at
//! two and a half minutes after the turn, priced normally when re-queried later
//! in the same working session; ids from two days earlier resolved immediately.
//! The exact window is **not** established — only that it is longer than the
//! ~2.5 minutes observed.
//!
//! (An earlier reading of this blamed interrupted turns. That was wrong: the
//! "control" session was five minutes older and queried at a different moment,
//! so it never controlled for anything. The interrupted session priced fine once
//! enough time had passed.)
//!
//! Consequence for the panel: opening a session's cost immediately after a turn
//! can legitimately show fewer priced calls than it has model messages. Those
//! land in `unpriced_calls` with a note and are never folded into the total as a
//! zero — "we could not price this yet" and "this was free" are different facts.
//! Because the on-disk cache only ever stores successful lookups, a later visit
//! re-asks and fills the gap in.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The provider whose cost API this module speaks.
const OPENROUTER: &str = "openrouter";

const GENERATION_ENDPOINT: &str = "https://openrouter.ai/api/v1/generation";

/// The env var holding the OpenRouter secret when `settings.yaml` does not name
/// one — dsh's own conventional choice, and the key `.credentials.yaml` uses.
const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";

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
/// `settings.yaml` declares the provider under `llm-pi-ai.providers.openrouter`,
/// either as an inline `apiKey` or — the shape on a real install — as
/// `apiKeyEnv: OPENROUTER_API_KEY`, *naming the variable that holds the secret*.
/// That name is the lookup key everywhere else:
///
/// 1. the process environment, and
/// 2. `$DSH_HOME/.credentials.yaml`, which per `@deepseek-ai/dsh-credentials-local`
///    is "a YAML mapping of credential reference to value, and nothing else" —
///    **keyed by env-var name, not by provider name**.
///
/// Measured against a real `dsh web`: with `OPENROUTER_API_KEY: sk-…` in that
/// file, `credentials.describe` answers
/// `{configured: true, source: "file", writable: true}` with no env var set.
/// An `openrouter: sk-…` entry is *accepted* by the file (a valid POSIX
/// identifier, so the server still boots) but no provider ever looks that name
/// up — the key silently does nothing. This function used to read exactly that
/// wrong name, which is why it is now derived from `apiKeyEnv`.
///
/// The env layer wins over the file, matching dsh's own documented precedence
/// (a per-run `VAR=… dsh` is operator intent for that run).
///
/// Returns `None` when no key is configured, which is a normal state (the user
/// may not use OpenRouter at all), not an error.
pub fn openrouter_api_key() -> Option<String> {
    let dsh_home = crate::session::get_dsh_dir()?;

    let settings = read_yaml(&dsh_home.join("settings.yaml"));
    let provider = settings
        .as_ref()
        .and_then(|s| s.pointer("/llm-pi-ai/providers/openrouter").cloned())
        .unwrap_or(Value::Null);

    // The variable that holds the secret. `OPENROUTER_API_KEY` is the
    // conventional name and the one a bare install uses, but honour whatever
    // the user pointed `apiKeyEnv` at.
    let var = provider
        .get("apiKeyEnv")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_KEY_ENV);

    if let Some(key) = non_empty(std::env::var(var).ok()) {
        return Some(key);
    }
    if let Some(key) = non_empty(
        provider
            .get("apiKey")
            .and_then(Value::as_str)
            .map(str::to_string),
    ) {
        return Some(key);
    }

    // The managed credential store, keyed by that same env-var name. An empty
    // `{}` (the shipped default) yields nothing.
    let creds = read_yaml(&dsh_home.join(".credentials.yaml"))?;
    non_empty(
        creds
            .get(var)
            .and_then(Value::as_str)
            .map(str::to_string),
    )
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
            // Names the exact line to write. A desktop-launched dsh inherits the
            // app's environment, which has no shell profile in it, so the file is
            // the option that actually works there — and it is keyed by the
            // env-var name, not by "openrouter".
            note: "no OpenRouter API key configured — add a line \
                   `OPENROUTER_API_KEY: <key>` to ~/.dsh/.credentials.yaml \
                   (or export that variable before launching)"
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

    /// One `assistant/message` source copied verbatim off the wire from a
    /// current dsh session (`session-edde1334-…`, read through
    /// `POST /api/session.history`).
    ///
    /// dsh wrapped the adapter's response record in a replay *envelope*: what
    /// used to sit directly on `replayState` now sits on `replayState.response`,
    /// alongside a `blocks` array. The installed package types it as
    /// `interface ReplayEnvelope { response: unknown; blocks?: readonly unknown[] }`
    /// (`@deepseek-ai/dsh-llm/lib/types/types.d.ts`), and the payload inside
    /// carries `version: 2` where the old flat one carried `version: 1`.
    fn v2_envelope_event() -> Value {
        json!({"type": "assistant/message", "data": {
            "turn": 1,
            "step": 1,
            "usage": {"inputTokens": 3, "outputTokens": 14, "cacheWriteTokens": 15069},
            "message": {
                "role": "assistant",
                "id": "msg-v2",
                "source": {
                    "kind": "model",
                    "provider": "openrouter",
                    "model": "anthropic/claude-haiku-4.5",
                    "replayState": {
                        "response": {
                            "kind": "pi-ai",
                            "version": 2,
                            "api": "openai-completions",
                            "provider": "openrouter",
                            "model": "anthropic/claude-haiku-4.5",
                            "responseId": "gen-1787026253-Te7maM1es7yqmeDdhUS6",
                            "stopReason": "stop"
                        },
                        "blocks": [{"type": "text"}]
                    }
                }
            }
        }})
    }

    /// The regression: every session dsh writes now uses the v2 envelope, so
    /// reading only the flat v1 path priced nothing and the panel said "no model
    /// calls" for all of them.
    ///
    /// Measured on a real install: of 64 sessions, 35 were v1 (priced fine — one
    /// summed to $0.0048613 through the live OpenRouter API) and 14 were v2 with
    /// a real `gen-…` id that Fleet could not see. The v2 ones were the newest,
    /// which is why every session the user opened looked unpriced.
    #[test]
    fn extracts_the_id_from_the_v2_replay_envelope() {
        let refs = generation_refs(&[v2_envelope_event()]);
        assert_eq!(
            refs,
            vec![GenerationRef {
                id: "gen-1787026253-Te7maM1es7yqmeDdhUS6".into(),
                provider: "openrouter".into(),
                model: "anthropic/claude-haiku-4.5".into(),
            }],
            "a current dsh session's generation id lives at \
             replayState.response.responseId"
        );
    }

    /// Both shapes coexist on one machine — a fix that only reads v2 would stop
    /// pricing the sessions that used to work.
    #[test]
    fn extracts_ids_from_both_replay_shapes_in_one_log() {
        let mut events = recorded_events();
        events.push(v2_envelope_event());
        let ids: Vec<String> = generation_refs(&events)
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(
            ids,
            vec!["gen-1", "gen-2", "gen-1787026253-Te7maM1es7yqmeDdhUS6"],
            "old flat sessions must keep pricing alongside the new envelope"
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

    /// The credential store is keyed by **environment-variable name**, not by
    /// provider name. Measured against a real `dsh web`: with
    /// `OPENROUTER_API_KEY: sk-…` in `$DSH_HOME/.credentials.yaml`,
    /// `credentials.describe` reports `{configured: true, source: "file"}`.
    /// A `openrouter: sk-…` entry is accepted by the file (it is a valid POSIX
    /// identifier) but nothing ever looks it up — measured too: the server still
    /// boots, the key is simply never used.
    #[test]
    fn key_comes_from_the_credentials_file_under_its_env_var_name() {
        with_temp_dsh_home(|base| {
            std::fs::write(
                base.join("settings.yaml"),
                "llm-pi-ai:\n  providers:\n    openrouter:\n      apiKeyEnv: FLEET_TEST_CRED_VAR\n",
            )
            .unwrap();
            std::env::remove_var("FLEET_TEST_CRED_VAR");

            // The provider-named form dsh never reads must NOT be picked up.
            std::fs::write(base.join(".credentials.yaml"), "openrouter: wrong-form\n").unwrap();
            assert_eq!(
                openrouter_api_key(),
                None,
                "a provider-named entry is not a credential dsh would use"
            );

            // The real form: the env-var name settings.yaml points at.
            std::fs::write(
                base.join(".credentials.yaml"),
                "FLEET_TEST_CRED_VAR: sk-from-file\n",
            )
            .unwrap();
            assert_eq!(openrouter_api_key().as_deref(), Some("sk-from-file"));
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

            // No provider block at all: the lookup falls back to the
            // conventional env-var name, which is also the credential-file key.
            std::fs::write(base.join("settings.yaml"), "llm-pi-ai: {}\n").unwrap();
            std::fs::write(
                base.join(".credentials.yaml"),
                "OPENROUTER_API_KEY: cred-key\n",
            )
            .unwrap();
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
