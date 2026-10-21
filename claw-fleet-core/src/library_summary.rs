//! Background AI summaries + tags for library items that lack them: wiki docs
//! (re-summarized when re-published) and artifacts an agent added without the
//! upload organizer's summary.
//!
//! Driven off the [`crate::library_index`] database, which already knows every
//! item's current version, body text and summary state:
//!
//! - **Never blocks a write.** Publishing or adding only changes files; a
//!   long-lived process (desktop, `fleet serve`) runs [`spawn_worker`], which
//!   syncs the index and picks up whatever needs a summary.
//! - **Bounded.** Calls go through the library's global LLM slot gate, at most
//!   [`MAX_PER_HOUR`] per machine-hour (counted in the shared database, so the
//!   desktop and `fleet serve` share one budget), and a cross-process claim on
//!   each row keeps two workers from summarizing the same item.
//! - **New items only, unless asked.** Items updated after the index was first
//!   created are summarized automatically. Older ones (the backlog) only when
//!   the `backfill` flag is on — see [`set_backfill`] / [`backfill_plan`].
//! - **Lean and accounted.** Text goes through
//!   [`crate::library_ingest::classify::complete_text`] (lean Claude route,
//!   codex/dsh fallback), images through its vision call; both are logged under
//!   [`crate::llm_usage::SCENARIO_LIBRARY_INGEST`].
//!
//! Where the result lands: a doc's `summary.json` sidecar
//! ([`crate::wiki::write_summary_in`]) and an artifact's `meta.json`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifacts;
use crate::library::{Roots, ITEM_ARTIFACT, ITEM_DOC};
use crate::library_index::LibraryIndex;
use crate::library_ingest::classify;
use crate::wiki;

/// Most summaries per machine-hour.
pub const MAX_PER_HOUR: u64 = 60;
/// Characters of an item's body shown to the model.
const EXCERPT_CHARS: usize = 4_000;
/// How long a worker owns a row it claimed.
const CLAIM_MS: u64 = 10 * 60 * 1000;
/// Pause between worker passes.
const PASS_INTERVAL: Duration = Duration::from_secs(30);
/// Items summarized per pass.
const PER_PASS: usize = 4;
/// Time a pass may spend extracting artifact text.
const EXTRACT_BUDGET: Duration = Duration::from_secs(20);

/// Estimated cost of one call, for [`backfill_plan`]. Measured on the lean
/// route with Haiku (see the plan's P3 report); refresh when the model changes.
pub const EST_TEXT_CALL_USD: f64 = 0.003;
pub const EST_VISION_CALL_USD: f64 = 0.006;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One item due for a summary.
#[derive(Clone, Debug)]
struct Due {
    rowid: i64,
    kind: String,
    item_id: String,
    title: String,
    version: String,
    artifact_kind: String,
    body: String,
}

/// How an item would be summarized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Text,
    Vision,
}

/// The SQL condition for "needs a summary": its summary does not describe the
/// current version, and there is something to summarize.
const DUE_WHERE: &str = "summary_version != version
     AND ((body_state = 'done' AND body != '') OR artifact_kind = 'image')";

/// What a full backfill would cost, for the user to approve first.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackfillPlan {
    /// Items older than activation that lack a summary.
    pub docs: usize,
    pub text_artifacts: usize,
    pub image_artifacts: usize,
    /// Items newer than activation still waiting (handled automatically).
    pub new_pending: usize,
    pub est_cost_usd: f64,
    pub enabled: bool,
}

pub fn backfill_plan(index: &LibraryIndex) -> Result<BackfillPlan, String> {
    let activated = activated_ms(index);
    let conn = index.conn();
    let count = |extra: &str| -> Result<usize, String> {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM items WHERE {DUE_WHERE} AND {extra}"),
            params![activated as i64],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .map_err(|e| format!("count: {e}"))
    };
    let docs = count("updated_ms < ?1 AND kind = 'doc'")?;
    let image_artifacts =
        count("updated_ms < ?1 AND kind = 'artifact' AND NOT (body_state = 'done' AND body != '')")?;
    let text_artifacts = count("updated_ms < ?1 AND kind = 'artifact' AND body_state = 'done' AND body != ''")?;
    let new_pending = count("updated_ms >= ?1")?;
    Ok(BackfillPlan {
        docs,
        text_artifacts,
        image_artifacts,
        new_pending,
        est_cost_usd: (docs + text_artifacts) as f64 * EST_TEXT_CALL_USD
            + image_artifacts as f64 * EST_VISION_CALL_USD,
        enabled: backfill_enabled(index),
    })
}

fn activated_ms(index: &LibraryIndex) -> u64 {
    index
        .meta_get("activated_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn backfill_enabled(index: &LibraryIndex) -> bool {
    index.meta_get("backfill").as_deref() == Some("1")
}

/// Turn summarizing the backlog on or off (it drips at [`MAX_PER_HOUR`]).
pub fn set_backfill(index: &LibraryIndex, on: bool) -> Result<(), String> {
    index.meta_set("backfill", if on { "1" } else { "0" })
}

/// Take one call from the shared hourly budget; false when it is spent.
fn take_budget(index: &LibraryIndex, now: u64) -> Result<bool, String> {
    let start: u64 = index
        .meta_get("summary_hour_start")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let used: u64 = index
        .meta_get("summary_hour_count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (start, used) = if now.saturating_sub(start) >= 3_600_000 {
        (now, 0)
    } else {
        (start, used)
    };
    if used >= MAX_PER_HOUR {
        return Ok(false);
    }
    index.meta_set("summary_hour_start", &start.to_string())?;
    index.meta_set("summary_hour_count", &(used + 1).to_string())?;
    Ok(true)
}

fn due(index: &LibraryIndex, now: u64, limit: usize) -> Result<Vec<Due>, String> {
    let floor = if backfill_enabled(index) { 0 } else { activated_ms(index) };
    let mut stmt = index
        .conn()
        .prepare(&format!(
            "SELECT id, kind, item_id, title, version, artifact_kind, body FROM items
             WHERE {DUE_WHERE}
               AND updated_ms >= ?1
               AND summary_claim_until_ms < ?2
               AND summary_retry_ms <= ?2
             ORDER BY updated_ms DESC
             LIMIT ?3"
        ))
        .map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map(params![floor as i64, now as i64, limit as i64], |r| {
            Ok(Due {
                rowid: r.get(0)?,
                kind: r.get(1)?,
                item_id: r.get(2)?,
                title: r.get(3)?,
                version: r.get(4)?,
                artifact_kind: r.get(5)?,
                body: r.get(6)?,
            })
        })
        .map_err(|e| format!("query: {e}"))?;
    Ok(rows.flatten().collect())
}

/// Claim a row for this worker; false when another worker holds it.
fn claim(index: &LibraryIndex, rowid: i64, now: u64) -> Result<bool, String> {
    index
        .conn()
        .execute(
            "UPDATE items SET summary_claim_until_ms = ?2
             WHERE id = ?1 AND summary_claim_until_ms < ?3",
            params![rowid, (now + CLAIM_MS) as i64, now as i64],
        )
        .map(|n| n == 1)
        .map_err(|e| format!("claim: {e}"))
}

fn release(index: &LibraryIndex, rowid: i64, failed: bool, now: u64) {
    let sql = if failed {
        // Back off 1h, 2h, 4h … up to a week.
        "UPDATE items SET summary_claim_until_ms = 0,
             summary_failures = summary_failures + 1,
             summary_retry_ms = ?2 + MIN(604800000, 3600000 * (1 << MIN(summary_failures, 8)))
         WHERE id = ?1"
    } else {
        "UPDATE items SET summary_claim_until_ms = 0, summary_failures = 0, summary_retry_ms = 0
         WHERE id = ?1"
    };
    let _ = index.conn().execute(sql, params![rowid, now as i64]);
}

fn mode_of(d: &Due) -> Option<Mode> {
    if !d.body.trim().is_empty() {
        Some(Mode::Text)
    } else if d.kind == ITEM_ARTIFACT && d.artifact_kind == "image" {
        Some(Mode::Vision)
    } else {
        None
    }
}

/// Prompt for summarizing one item from its text.
pub fn summary_prompt(title: &str, kind_label: &str, body: &str) -> String {
    let excerpt: String = body.chars().take(EXCERPT_CHARS).collect();
    format!(
        "Summarize this {kind_label} for a project library, so someone browsing or searching \
the library knows what it is without opening it.\n\nTitle: {title}\n\nContent (may be cut \
off):\n{excerpt}\n\nReturn \"summary\": 1 to 3 sentences on what it is and its key facts; \
\"tags\": 1 to 5 short keywords. {rule}\n\nReply with one JSON object only:\n\
{{\"summary\":\"...\",\"tags\":[\"...\"]}}\n",
        excerpt = excerpt.trim(),
        rule = classify::LANGUAGE_RULE,
    )
}

fn vision_summary_prompt(file_name: &str, title: &str) -> String {
    format!(
        "Use the Read tool to look at the file `{file_name}` in the current directory. Read \
nothing else. It is stored in a project library under the title \"{title}\".\n\nReturn \
\"summary\": 1 to 3 sentences on what it shows and any key facts visible; \"tags\": 1 to 5 \
short keywords. {rule}\n\nReply with one JSON object only:\n\
{{\"summary\":\"...\",\"tags\":[\"...\"]}}\n",
        rule = classify::LANGUAGE_RULE,
    )
}

/// `(summary, tags)` from a model answer.
pub fn parse_summary(text: &str) -> Option<(String, Vec<String>)> {
    let v: Value = serde_json::from_str(classify::extract_json(text)?).ok()?;
    let v = match v {
        Value::Array(mut a) if !a.is_empty() => a.swap_remove(0),
        v => v,
    };
    let summary = classify::clip(
        v.get("summary").and_then(Value::as_str).unwrap_or(""),
        classify::MAX_SUMMARY_CHARS,
    );
    (!summary.is_empty()).then(|| (summary, classify::tags_field(&v)))
}

/// Summarize one claimed item and store the result. Returns the call's cost.
fn summarize_one(roots: &Roots, d: &Due) -> Result<f64, String> {
    let mode = mode_of(d).ok_or_else(|| "nothing to summarize".to_string())?;
    let outcome = match mode {
        Mode::Text => {
            let label = if d.kind == ITEM_DOC { "document" } else { "file" };
            classify::complete_text(
                crate::llm_provider::ModelSlot::Fast,
                &summary_prompt(&d.title, label, &d.body),
            )
            .ok_or_else(|| "no model answered".to_string())?
        }
        Mode::Vision => {
            let a = artifacts::get_in(&roots.artifacts, &d.item_id)?;
            if crate::library_ingest::extract::vision_image_type(&a.name).is_none() {
                return Err(format!("'{}' is not an image Claude can read", a.name));
            }
            let blob = artifacts::blob_path(&roots.artifacts, &a);
            let dir = blob.parent().ok_or("artifact blob has no directory")?;
            classify::complete_vision(dir, &vision_summary_prompt(&a.name, &d.title))?
        }
    };
    let (summary, tags) =
        parse_summary(&outcome.text).ok_or_else(|| "unparseable summary".to_string())?;
    match d.kind.as_str() {
        ITEM_DOC => wiki::write_summary_in(
            &roots.wiki,
            &d.item_id,
            &wiki::DocSummary {
                summary,
                tags,
                version: d.version.clone(),
                generated_ms: now_ms(),
            },
        )?,
        _ => {
            artifacts::edit_in(&roots.artifacts, &d.item_id, |a| {
                // A summary someone else wrote meanwhile wins.
                if a.summary.trim().is_empty() {
                    a.summary = summary;
                    if a.tags.is_empty() {
                        a.tags = tags;
                    }
                }
            })?;
        }
    }
    Ok(outcome.cost_usd)
}

/// One pass: summarize up to `limit` due items. Returns how many succeeded.
pub fn run_pass(roots: &Roots, index: &LibraryIndex, limit: usize) -> Result<usize, String> {
    let mut done = 0;
    for d in due(index, now_ms(), limit)? {
        if mode_of(&d).is_none() {
            continue;
        }
        let now = now_ms();
        if !take_budget(index, now)? {
            break;
        }
        if !claim(index, d.rowid, now)? {
            continue;
        }
        match summarize_one(roots, &d) {
            Ok(cost) => {
                crate::log_debug(&format!(
                    "[library_summary] {} {} summarized (${cost:.4})",
                    d.kind, d.item_id
                ));
                release(index, d.rowid, false, now_ms());
                done += 1;
            }
            Err(e) => {
                crate::log_debug(&format!(
                    "[library_summary] {} {} failed: {e}",
                    d.kind, d.item_id
                ));
                release(index, d.rowid, true, now_ms());
            }
        }
    }
    Ok(done)
}

/// Whether a row exists and still needs a summary (tests, status).
pub fn is_due(index: &LibraryIndex, kind: &str, id: &str) -> bool {
    index
        .conn()
        .query_row(
            &format!("SELECT 1 FROM items WHERE kind = ?1 AND item_id = ?2 AND {DUE_WHERE}"),
            params![kind, id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
}

static WORKER: AtomicBool = AtomicBool::new(false);

/// Start the background index + summary worker for this process (once). For
/// long-lived processes only — the desktop and `fleet serve`.
pub fn spawn_worker() {
    if WORKER.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("library-summary".into())
        .spawn(|| loop {
            if let Ok(roots) = Roots::home() {
                match LibraryIndex::open_at(&crate::library_index::db_path(&roots)) {
                    Ok(index) => {
                        let _ = index.sync(&roots);
                        let _ = index.extract_pending(&roots, EXTRACT_BUDGET);
                        if crate::llm_provider::shared_config().provider != "none" {
                            let _ = run_pass(&roots, &index, PER_PASS);
                        }
                    }
                    Err(e) => crate::log_debug(&format!("[library_summary] open index: {e}")),
                }
            }
            std::thread::sleep(PASS_INTERVAL);
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_index::SearchOpts;
    use tempfile::TempDir;

    fn roots(tmp: &TempDir) -> (Roots, std::path::PathBuf) {
        let base = tmp.path().canonicalize().unwrap();
        let ws = base.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        (
            Roots {
                library: base.join("library"),
                wiki: base.join("wiki"),
                artifacts: base.join("artifacts"),
            },
            ws,
        )
    }

    #[test]
    fn answers_parse_loosely_and_clip() {
        let (s, t) = parse_summary("```json\n{\"summary\":\"一份装修合同。\",\"tags\":\"合同，装修\"}\n```").unwrap();
        assert_eq!((s.as_str(), t), ("一份装修合同。", vec!["合同".to_string(), "装修".to_string()]));
        assert!(parse_summary("{\"summary\":\"\"}").is_none());
        assert!(parse_summary("no").is_none());
        let p = summary_prompt("T", "document", &"x".repeat(10_000));
        assert!(p.contains("Title: T") && p.len() < 5_000);
    }

    #[test]
    fn only_new_items_are_due_until_backfill_is_on_and_republish_makes_a_doc_due_again() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        let publish = |slug: &str, body: &str| {
            wiki::publish_text_in(&roots.wiki, slug, None, body, &ws, wiki::TextPublishMode::Replace)
                .unwrap()
        };
        let old = publish("old", "old body");
        let index = LibraryIndex::open_at(&crate::library_index::db_path(&roots)).unwrap();
        // Pretend the index was created after `old` was published.
        index.meta_set("activated_ms", &(old.updated_ms + 1).to_string()).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let new = publish("new", "new body");
        index.sync(&roots).unwrap();

        let ids = |i: &LibraryIndex| -> Vec<String> {
            due(i, now_ms(), 10).unwrap().into_iter().map(|d| d.item_id).collect()
        };
        assert_eq!(ids(&index), ["new"]);
        let plan = backfill_plan(&index).unwrap();
        assert_eq!((plan.docs, plan.new_pending, plan.enabled), (1, 1, false));
        assert!(plan.est_cost_usd > 0.0);

        set_backfill(&index, true).unwrap();
        assert_eq!(ids(&index).len(), 2);
        set_backfill(&index, false).unwrap();

        // Summarized: no longer due, and searchable by its tag.
        wiki::write_summary_in(
            &roots.wiki,
            "new",
            &wiki::DocSummary {
                summary: "s".into(),
                tags: vec!["zymurgy".into()],
                version: new.current_version.clone(),
                generated_ms: 1,
            },
        )
        .unwrap();
        index.sync(&roots).unwrap();
        assert!(!is_due(&index, ITEM_DOC, "new"));
        assert_eq!(index.search("zymurgy", &SearchOpts::default()).unwrap().len(), 1);

        // A re-publish makes the summary stale.
        std::thread::sleep(Duration::from_millis(5));
        publish("new", "rewritten body");
        index.sync(&roots).unwrap();
        assert!(is_due(&index, ITEM_DOC, "new"));
    }

    #[test]
    fn claims_are_exclusive_and_the_hourly_budget_is_shared() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        wiki::publish_text_in(&roots.wiki, "a", None, "body", &ws, wiki::TextPublishMode::Replace)
            .unwrap();
        let path = crate::library_index::db_path(&roots);
        let one = LibraryIndex::open_at(&path).unwrap();
        let two = LibraryIndex::open_at(&path).unwrap();
        // The doc predates the index; count it as new.
        one.meta_set("activated_ms", "0").unwrap();
        one.sync(&roots).unwrap();
        let d = &due(&one, now_ms(), 1).unwrap()[0];
        let now = now_ms();
        assert!(claim(&one, d.rowid, now).unwrap());
        assert!(!claim(&two, d.rowid, now).unwrap(), "held by the other worker");
        assert!(due(&two, now, 1).unwrap().is_empty(), "a claimed row is not due");
        release(&one, d.rowid, true, now);
        assert!(due(&two, now, 1).unwrap().is_empty(), "a failure backs off");

        for _ in 0..MAX_PER_HOUR {
            assert!(take_budget(&one, now).unwrap());
        }
        assert!(!take_budget(&two, now).unwrap(), "budget is per database, not per handle");
        assert!(take_budget(&two, now + 3_600_001).unwrap(), "a new hour resets it");
    }
}
