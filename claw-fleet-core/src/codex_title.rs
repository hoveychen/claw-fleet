//! Fleet-side semantic title generation for Codex **local** sessions.
//!
//! Codex's local `codex exec` threads store the *raw first user message* as
//! their `title` — see `codex-rs/state/src/extract.rs`, where `EventMsg::
//! UserMessage` fills an empty `title` with the truncated first message and no
//! model is ever consulted. The AI auto-naming people remember from ChatGPT is
//! a *cloud* feature (`has_generated_title` lives only on the backend
//! `TaskResponse`/`TaskListItem` models, i.e. hosted Codex cloud tasks); local
//! `codex exec` threads never get it.
//!
//! Claude sessions look better only because the Claude fork writes an
//! `ai-title` record into the transcript (`saveAiGeneratedTitle`). Codex has no
//! equivalent, so Fleet fills the gap here: generate a short semantic title
//! from the first user message using **Codex's own cheapest model** (so we
//! don't couple Codex sessions to Claude), cache it under
//! `~/.fleet/codex-titles.json`, and let [`crate::codex_source`] prefer the
//! cached title over the raw prompt.
//!
//! Generation is fire-and-forget: [`maybe_generate`] is called from the codex
//! scan for every eligible thread, returns instantly, and spawns at most one
//! background `codex exec` at a time. Results persist, so it is a one-time cost
//! per thread; failures are remembered for the process lifetime so a broken
//! Codex CLI is not hammered every 2 s scan tick.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::llm_provider::{CodexCliProvider, LlmProvider};
use crate::log_debug;

/// Raw first messages shorter than this are already a fine, concise title on
/// their own — summarizing them wastes Codex quota and usually produces a title
/// no better than the prompt itself. Only longer prompts (which the UI would
/// otherwise truncate) are worth a model call. CJK prompts are information-dense,
/// so this is deliberately below a typical English sentence length.
const MIN_CHARS_TO_SUMMARIZE: usize = 30;

/// Cap on how much of the first message we feed the model. A title only needs
/// the gist; the opening is where the ask lives.
const MAX_INPUT_CHARS: usize = 800;

/// Longest title we keep. Codex should return something short; this guards
/// against a runaway response.
const MAX_TITLE_CHARS: usize = 80;

/// How long a single generation may run before we give up.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(60);

/// At most this many background generations run concurrently. Codex `exec`
/// spins up a full CLI process, so we keep this small; threads that don't get a
/// slot simply retry on the next scan tick (they are not marked in-flight).
const MAX_CONCURRENT: usize = 1;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// In-memory mirror of `~/.fleet/codex-titles.json`, lazily loaded on first
/// read. `None` means "not loaded yet".
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Thread ids whose generation is currently running or has already failed this
/// process run. Both cases must not spawn again: a running one would duplicate
/// work, a failed one would hammer a broken CLI every scan.
static IN_FLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn cache_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("codex-titles.json"))
}

fn load_from_disk() -> HashMap<String, String> {
    let Some(path) = cache_path() else {
        return HashMap::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Return the Fleet-generated title for `thread_id`, if one has been cached.
///
/// Reads from an in-memory map (loaded once from disk), so it is cheap enough
/// to call for every thread on every scan tick.
pub fn cached_title(thread_id: &str) -> Option<String> {
    let mut guard = CACHE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(load_from_disk());
    }
    guard.as_ref().and_then(|m| m.get(thread_id).cloned())
}

fn persist(thread_id: &str, title: &str) {
    // Update the in-memory map first so `cached_title` reflects it immediately.
    {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(thread_id.to_string(), title.to_string());
    }
    // Then write the whole map back. Re-read from disk and merge so a
    // concurrent writer's entries aren't clobbered by our stale snapshot.
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut on_disk = load_from_disk();
    on_disk.insert(thread_id.to_string(), title.to_string());
    // Fold in anything the in-memory map has that disk might not (other entries
    // added this run) — in-memory is the union of disk + our writes.
    if let Some(mem) = CACHE.lock().unwrap().as_ref() {
        for (k, v) in mem {
            on_disk.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&on_disk) {
        let _ = std::fs::write(&path, json);
    }
}

/// True if `thread_id` is already generating or has failed this run.
fn is_in_flight(thread_id: &str) -> bool {
    let guard = IN_FLIGHT.lock().unwrap();
    guard.as_ref().is_some_and(|s| s.contains(thread_id))
}

fn mark_in_flight(thread_id: &str) {
    let mut guard = IN_FLIGHT.lock().unwrap();
    guard
        .get_or_insert_with(HashSet::new)
        .insert(thread_id.to_string());
}

fn clear_in_flight(thread_id: &str) {
    if let Some(s) = IN_FLIGHT.lock().unwrap().as_mut() {
        s.remove(thread_id);
    }
}

/// Whether `first_message` is worth summarizing. Empty/whitespace messages
/// (e.g. image-only or resumed sessions) and already-short ones are skipped.
pub fn is_eligible(first_message: &str) -> bool {
    first_message.trim().chars().count() >= MIN_CHARS_TO_SUMMARIZE
}

/// Fire-and-forget: ensure a semantic title exists for `thread_id`, generating
/// one in the background if needed. Returns immediately.
///
/// No-ops when the message is ineligible, a title is already cached, generation
/// is already running/failed for this thread, or all concurrency slots are
/// busy (the caller will retry on the next scan).
pub fn maybe_generate(thread_id: &str, first_message: &str) {
    if !is_eligible(first_message) {
        return;
    }
    if cached_title(thread_id).is_some() {
        return;
    }
    if is_in_flight(thread_id) {
        return;
    }
    // Backpressure: if every slot is taken, leave this thread un-flagged so it
    // is retried next tick rather than dropped.
    if ACTIVE.load(Ordering::SeqCst) >= MAX_CONCURRENT {
        return;
    }

    mark_in_flight(thread_id);
    ACTIVE.fetch_add(1, Ordering::SeqCst);

    let tid = thread_id.to_string();
    let msg = first_message.to_string();
    std::thread::spawn(move || {
        let result = generate_blocking(&msg);
        match result {
            Some(title) => {
                persist(&tid, &title);
                // Success: drop the in-flight marker (the cache now short-circuits).
                clear_in_flight(&tid);
                log_debug(&format!(
                    "[codex_title] generated title for {}: {:?}",
                    &tid[..tid.len().min(12)],
                    title
                ));
            }
            None => {
                // Failure: KEEP the in-flight marker so we don't retry a broken
                // CLI every scan tick. It clears on next app start.
                log_debug(&format!(
                    "[codex_title] generation failed for {}",
                    &tid[..tid.len().min(12)]
                ));
            }
        }
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    });
}

/// Build the naming prompt for a first user message.
fn build_prompt(first_message: &str) -> String {
    let truncated: String = first_message.chars().take(MAX_INPUT_CHARS).collect();
    format!(
        "You are naming a task for a task list. Below is the first message a user sent to a \
         coding assistant. Produce a SHORT title (at most 8 words) that captures the task. \
         Match the language of the message (if the message is in Chinese, the title must be in \
         Chinese). Do NOT wrap the title in quotes, do NOT add trailing punctuation, and do NOT \
         explain. Reply with ONLY the title on a single line.\n\n---\n{truncated}"
    )
}

/// Normalize a raw model response into a clean title, or `None` if it does not
/// look like a usable title.
fn clean_title(raw: &str) -> Option<String> {
    // Codex `exec` returns the final message only, but be defensive: take the
    // last non-empty line (any preamble would come first).
    let line = raw.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    // Strip surrounding quotes / backticks the model may add despite instructions.
    let trimmed = line
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '“' || c == '”')
        .trim();
    if trimmed.is_empty() {
        return None;
    }
    let capped: String = trimmed.chars().take(MAX_TITLE_CHARS).collect();
    Some(capped.trim().to_string())
}

/// Preference order for the cheapest *working* Codex model.
///
/// NB: the provider's `default_fast_model()` returns `gpt-5.1-codex-mini`, which
/// a ChatGPT-account login rejects with HTTP 400 ("model is not supported when
/// using Codex with a ChatGPT account") — verified live. Fleet's Codex is driven
/// by the ChatGPT quota, so we must pick a model that account can actually run.
/// Luna is Codex's fast/cheap tier; Terra is the balanced fallback. We avoid the
/// `-mini` slugs precisely because the mini tier is the one that 400s.
const CHEAP_MODEL_PREFS: &[&str] = &["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.5"];

/// Pick the cheapest available Codex model this login can run.
fn cheap_codex_model(provider: &CodexCliProvider) -> String {
    let ids: Vec<String> = provider.list_models().into_iter().map(|m| m.id).collect();
    select_cheap_model(&ids)
}

/// Pure model-selection logic: first preferred slug that is available, else the
/// first available model, else a hardcoded default.
fn select_cheap_model(available: &[String]) -> String {
    CHEAP_MODEL_PREFS
        .iter()
        .find(|pref| available.iter().any(|id| id == **pref))
        .map(|s| s.to_string())
        .or_else(|| available.first().cloned())
        .unwrap_or_else(|| "gpt-5.6-luna".to_string())
}

/// Run one blocking generation via Codex's cheapest model. Returns the cleaned
/// title, or `None` on any failure (Codex unavailable, timeout, empty output).
fn generate_blocking(first_message: &str) -> Option<String> {
    let provider = CodexCliProvider::new();
    if !provider.is_available() {
        return None;
    }
    let model = cheap_codex_model(&provider);
    let prompt = build_prompt(first_message);
    let raw = crate::llm_usage::complete_accounted(
        &provider,
        &prompt,
        &model,
        GENERATION_TIMEOUT,
        crate::llm_usage::SCENARIO_CODEX_TITLE,
    )?;
    clean_title(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_skips_short_and_empty() {
        assert!(!is_eligible(""));
        assert!(!is_eligible("   "));
        assert!(!is_eligible("hi"));
        assert!(!is_eligible("github ci 失败了"));
        // A long, truncation-prone prompt is eligible.
        let long = "帮我将仓库里的命令 tab 将相同的命令 group 起来，支持创建为 shortcut 放到页面顶部";
        assert!(long.chars().count() >= MIN_CHARS_TO_SUMMARIZE);
        assert!(is_eligible(long));
    }

    #[test]
    fn clean_title_strips_quotes_and_takes_last_line() {
        assert_eq!(clean_title("\"Fix login bug\"").as_deref(), Some("Fix login bug"));
        assert_eq!(clean_title("`重构 relay 心跳`").as_deref(), Some("重构 relay 心跳"));
        // Preamble line followed by the real title.
        assert_eq!(
            clean_title("Here is the title:\nMigrate auth middleware").as_deref(),
            Some("Migrate auth middleware")
        );
    }

    #[test]
    fn select_cheap_model_prefers_luna_then_falls_back() {
        let s = |a: &[&str]| select_cheap_model(&a.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        // Luna present → luna wins even when pricier models are listed first.
        assert_eq!(s(&["gpt-5.6-sol", "gpt-5.6-luna", "gpt-5.5"]), "gpt-5.6-luna");
        // No luna → terra is next preference.
        assert_eq!(s(&["gpt-5.6-sol", "gpt-5.6-terra"]), "gpt-5.6-terra");
        // None of the preferred slugs → first available.
        assert_eq!(s(&["gpt-5.4", "gpt-5.4-mini"]), "gpt-5.4");
        // Empty list → hardcoded default.
        assert_eq!(s(&[]), "gpt-5.6-luna");
        // The known-broken mini must NEVER be selected on its own list unless
        // it's the only thing available (degenerate) — here luna is absent so
        // it falls through to first-available, which is acceptable.
        assert_eq!(s(&["gpt-5.1-codex-mini"]), "gpt-5.1-codex-mini");
    }

    #[test]
    fn clean_title_rejects_empty() {
        assert_eq!(clean_title(""), None);
        assert_eq!(clean_title("\n\n  \n"), None);
        assert_eq!(clean_title("\"\""), None);
    }

    #[test]
    fn clean_title_caps_length() {
        let long = "x".repeat(200);
        let out = clean_title(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_TITLE_CHARS);
    }

    /// Live end-to-end: hits the real Codex CLI. Ignored by default (network +
    /// quota + ~15s). Run with `cargo test -p claw-fleet-core -- --ignored
    /// live_generate_blocking --nocapture` to eyeball a real title.
    #[test]
    #[ignore]
    fn live_generate_blocking() {
        let msg = "帮我将仓库里的命令 tab，将相同的命令 group 起来，支持创建为 shortcut 放到页面顶部，并且支持跨应用重启持久保存";
        let title = generate_blocking(msg);
        println!("LIVE TITLE = {title:?}");
        let title = title.expect("codex should return a title");
        assert!(!title.is_empty());
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
    }

    #[test]
    fn cache_roundtrips_through_disk() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());
        // Reset in-memory state so we read from our temp dir.
        *CACHE.lock().unwrap() = None;

        assert_eq!(cached_title("thread-abc"), None);
        persist("thread-abc", "Migrate auth middleware");
        assert_eq!(
            cached_title("thread-abc").as_deref(),
            Some("Migrate auth middleware")
        );

        // A fresh in-memory load must still see it (proves disk persistence).
        *CACHE.lock().unwrap() = None;
        assert_eq!(
            cached_title("thread-abc").as_deref(),
            Some("Migrate auth middleware")
        );

        std::env::remove_var("FLEET_HOME");
    }

    #[test]
    fn persist_merges_without_clobbering_other_entries() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());
        *CACHE.lock().unwrap() = None;

        persist("a", "title a");
        // Simulate another writer adding "b" directly to disk.
        let path = cache_path().unwrap();
        let mut disk: HashMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        disk.insert("b".into(), "title b".into());
        std::fs::write(&path, serde_json::to_string_pretty(&disk).unwrap()).unwrap();

        // Our next persist must not drop "b".
        persist("c", "title c");
        let final_disk: HashMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(final_disk.get("a").map(String::as_str), Some("title a"));
        assert_eq!(final_disk.get("b").map(String::as_str), Some("title b"));
        assert_eq!(final_disk.get("c").map(String::as_str), Some("title c"));

        std::env::remove_var("FLEET_HOME");
    }
}
