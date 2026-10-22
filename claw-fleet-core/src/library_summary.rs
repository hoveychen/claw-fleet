//! Background AI summaries + tags for library items that lack them: wiki docs
//! (re-summarized when re-published) and artifacts an agent added without the
//! upload organizer's summary. Videos are summarized from their poster frame
//! ([`crate::video_poster`]) plus their title, file name, folder and note.
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
//!   codex/dsh fallback), images and video posters through its vision call;
//!   both are logged under [`crate::llm_usage::SCENARIO_LIBRARY_INGEST`].
//! - **Repairable.** [`language_mismatches`] finds summaries not written in
//!   their project's language; [`resummarize_in`] rewrites them after backing
//!   the old ones up, and [`restore_summaries_in`] puts a backup back.
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
use crate::library_ingest::{classify, Brain, LiveBrain};
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

/// Estimated cost of one call, for [`backfill_plan`]. Measured 2026-10-21 on
/// the lean route with Haiku (`measure_summary_cost`): three real docs whose
/// 4,000-char excerpts ran ~8.4K input tokens cost $0.0095–$0.0146 (most of
/// the store hits the excerpt cap), one image $0.0081. Refresh when the model
/// or the prompt changes.
pub const EST_TEXT_CALL_USD: f64 = 0.011;
pub const EST_VISION_CALL_USD: f64 = 0.0085;

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
    /// The project key, whose language the summary is written in.
    project: String,
}

/// How an item would be summarized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Text,
    Vision,
    /// A video, through its poster frame.
    Poster,
}

/// The SQL condition for "needs a summary": its summary does not describe the
/// current version, and there is something to summarize.
const DUE_WHERE: &str = "summary_version != version
     AND ((body_state = 'done' AND body != '') OR artifact_kind IN ('image', 'video'))";

/// SQL for "summarized through a vision call" (an image or a video poster),
/// for cost estimates.
const VISION_SQL: &str = "artifact_kind IN ('image', 'video') AND NOT (body_state = 'done' AND body != '')";

/// File name a video's poster is handed to the vision call under.
const POSTER_FILE: &str = "poster.jpg";

/// What a full backfill would cost, for the user to approve first.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackfillPlan {
    /// Items older than activation that lack a summary.
    pub docs: usize,
    pub text_artifacts: usize,
    pub image_artifacts: usize,
    #[serde(default)]
    pub video_artifacts: usize,
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
    let image_artifacts = count(&format!("updated_ms < ?1 AND artifact_kind = 'image' AND {VISION_SQL}"))?;
    let video_artifacts = count(&format!("updated_ms < ?1 AND artifact_kind = 'video' AND {VISION_SQL}"))?;
    let text_artifacts = count("updated_ms < ?1 AND kind = 'artifact' AND body_state = 'done' AND body != ''")?;
    let new_pending = count("updated_ms >= ?1")?;
    Ok(BackfillPlan {
        docs,
        text_artifacts,
        image_artifacts,
        video_artifacts,
        new_pending,
        est_cost_usd: (docs + text_artifacts) as f64 * EST_TEXT_CALL_USD
            + (image_artifacts + video_artifacts) as f64 * EST_VISION_CALL_USD,
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
            "SELECT id, kind, item_id, title, version, artifact_kind, body, project FROM items
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
                project: r.get(7)?,
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

/// Take a row out of the due set right after its summary was stored. The next
/// sync re-reads the item and fills in the real summary; until then a worker
/// looping without a sync in between would summarize the same item again.
fn mark_summarized(index: &LibraryIndex, rowid: i64, version: &str) {
    let _ = index.conn().execute(
        "UPDATE items SET summary_version = ?2 WHERE id = ?1",
        params![rowid, version],
    );
}

fn mode_of(d: &Due) -> Option<Mode> {
    if !d.body.trim().is_empty() {
        Some(Mode::Text)
    } else if d.kind == ITEM_ARTIFACT && d.artifact_kind == "image" {
        Some(Mode::Vision)
    } else if d.kind == ITEM_ARTIFACT && d.artifact_kind == "video" {
        Some(Mode::Poster)
    } else {
        None
    }
}

/// Prompt for summarizing one item from its text.
pub fn summary_prompt(title: &str, kind_label: &str, body: &str, lang: classify::Language) -> String {
    let excerpt: String = body.chars().take(EXCERPT_CHARS).collect();
    format!(
        "Summarize this {kind_label} for a project library, so someone browsing or searching \
the library knows what it is without opening it.\n\nTitle: {title}\n\nContent (may be cut \
off):\n{excerpt}\n\nReturn \"summary\": 1 to 3 sentences on what it is and its key facts; \
\"tags\": 1 to 5 short keywords. {rule}\n\nReply with one JSON object only:\n\
{{\"summary\":\"...\",\"tags\":[\"...\"]}}\n",
        excerpt = excerpt.trim(),
        rule = lang.rule(),
    )
}

fn vision_summary_prompt(file_name: &str, title: &str, lang: classify::Language) -> String {
    format!(
        "Use the Read tool to look at the file `{file_name}` in the current directory. Read \
nothing else. It is stored in a project library under the title \"{title}\".\n\nReturn \
\"summary\": 1 to 3 sentences on what it shows and any key facts visible; \"tags\": 1 to 5 \
short keywords. {rule}\n\nReply with one JSON object only:\n\
{{\"summary\":\"...\",\"tags\":[\"...\"]}}\n",
        rule = lang.rule(),
    )
}

/// What a video is summarized from besides its poster frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoBrief {
    pub title: String,
    pub file_name: String,
    /// The library folder it is filed in, or the directory it came from.
    pub folder: String,
    /// What the agent or person who added it wrote about it.
    pub note: String,
}

impl VideoBrief {
    pub fn of(a: &artifacts::Artifact) -> Self {
        Self {
            title: if a.title.trim().is_empty() { a.name.clone() } else { a.title.clone() },
            file_name: a.name.clone(),
            folder: artifacts::effective_directory(a),
            note: a.note.clone(),
        }
    }
}

/// Prompt for summarizing a video from its poster frame ([`POSTER_FILE`])
/// and what is known about it.
pub fn video_summary_prompt(v: &VideoBrief, lang: classify::Language) -> String {
    let mut facts = format!("Title: {}\nFile name: {}\n", v.title.trim(), v.file_name.trim());
    if !v.folder.trim().is_empty() {
        facts.push_str(&format!("Folder: {}\n", v.folder.trim()));
    }
    if !v.note.trim().is_empty() {
        let note: String = v.note.trim().chars().take(EXCERPT_CHARS).collect();
        facts.push_str(&format!("Note from whoever added it: {note}\n"));
    }
    format!(
        "Use the Read tool to look at the file `{POSTER_FILE}` in the current directory. Read \
nothing else. It is one frame taken from near the start of a video stored in a project library:\n\n\
{facts}\nThe frame shows a single moment, so judge what the video is about from the frame and \
the facts above together, and do not claim what a still cannot show (motion, sound, length) \
unless the facts say it.\n\nReturn \"summary\": 1 to 3 sentences on what the video is and its \
key facts; \"tags\": 1 to 5 short keywords. {rule}\n\nReply with one JSON object only:\n\
{{\"summary\":\"...\",\"tags\":[\"...\"]}}\n",
        rule = lang.rule(),
    )
}

/// `(summary, tags)` from a model answer.
pub fn parse_summary(text: &str) -> Option<(String, Vec<String>)> {
    let v: Value = classify::parse_value(text)?;
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

/// How a summarize attempt that did not fail ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Summarized {
    Done,
    /// Nothing to summarize from (a video with no poster frame); no call was
    /// made. Not a failure: retrying would not help until the file changes.
    Skipped(String),
}

/// Summarize one claimed item and store the result. `spent` gets the call's
/// cost whether or not a summary came of it: an answer that does not parse
/// was still paid for. `force` overwrites an artifact summary that is already
/// there (a re-summarize); otherwise one written meanwhile wins.
fn summarize_one(
    roots: &Roots,
    index: &LibraryIndex,
    d: &Due,
    brain: &dyn Brain,
    spent: &mut f64,
    force: bool,
) -> Result<Summarized, String> {
    let mode = mode_of(d).ok_or_else(|| "nothing to summarize".to_string())?;
    // The project's language, not the item's: one library reads in one.
    let lang = index.project_language(&d.project, &[]);
    let outcome = match mode {
        Mode::Text => {
            let label = if d.kind == ITEM_DOC { "document" } else { "file" };
            brain
                .text(
                    crate::llm_provider::ModelSlot::Fast,
                    &summary_prompt(&d.title, label, &d.body, lang),
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
            brain.vision(dir, &vision_summary_prompt(&a.name, &d.title, lang))?
        }
        Mode::Poster => {
            let a = artifacts::get_in(&roots.artifacts, &d.item_id)?;
            let Some(jpeg) = crate::video_poster::poster_jpeg_for(&roots.artifacts, &a) else {
                return Ok(Summarized::Skipped("no poster frame".to_string()));
            };
            let scratch = PosterScratch::new(&jpeg)?;
            brain.vision(&scratch.0, &video_summary_prompt(&VideoBrief::of(&a), lang))?
        }
    };
    *spent = outcome.cost_usd;
    // Keep the start of the answer: the only clue to why it did not parse.
    let (summary, tags) = parse_summary(&outcome.text).ok_or_else(|| {
        format!("unparseable summary: {}", classify::clip(&outcome.text, 160))
    })?;
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
                if force {
                    a.summary = summary;
                    a.tags = tags;
                } else if a.summary.trim().is_empty() {
                    // A summary someone else wrote meanwhile wins.
                    a.summary = summary;
                    if a.tags.is_empty() {
                        a.tags = tags;
                    }
                }
            })?;
        }
    }
    Ok(Summarized::Done)
}

/// A private directory holding just a video's poster, for the vision call
/// (which may read anything in its working directory); removed on drop.
struct PosterScratch(std::path::PathBuf);

impl PosterScratch {
    fn new(jpeg: &[u8]) -> Result<Self, String> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "fleet-video-summary-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("poster scratch dir: {e}"))?;
        let scratch = Self(dir);
        std::fs::write(scratch.0.join(POSTER_FILE), jpeg).map_err(|e| format!("write poster: {e}"))?;
        Ok(scratch)
    }
}

impl Drop for PosterScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether a video row has a poster frame to summarize from (extracting it
/// now if it was never tried).
fn poster_available(roots: &Roots, d: &Due) -> bool {
    artifacts::get_in(&roots.artifacts, &d.item_id)
        .ok()
        .and_then(|a| crate::video_poster::poster_jpeg_for(&roots.artifacts, &a))
        .is_some()
}

/// One pass: summarize up to `limit` due items. Returns how many succeeded.
pub fn run_pass(roots: &Roots, index: &LibraryIndex, limit: usize) -> Result<usize, String> {
    let mut done = 0;
    for d in due(index, now_ms(), limit)? {
        if mode_of(&d).is_none() {
            continue;
        }
        let now = now_ms();
        // A skip costs nothing; only a call takes from the budget. The
        // poster is looked up again inside, from its cache.
        if mode_of(&d) == Some(Mode::Poster) && !poster_available(roots, &d) {
            if claim(index, d.rowid, now)? {
                release(index, d.rowid, false, now);
                mark_summarized(index, d.rowid, &d.version);
            }
            continue;
        }
        if !take_budget(index, now)? {
            break;
        }
        if !claim(index, d.rowid, now)? {
            continue;
        }
        let mut spent = 0.0;
        match summarize_one(roots, index, &d, &LiveBrain, &mut spent, false) {
            Ok(Summarized::Done) => {
                crate::log_debug(&format!(
                    "[library_summary] {} {} summarized (${spent:.4})",
                    d.kind, d.item_id
                ));
                release(index, d.rowid, false, now_ms());
                mark_summarized(index, d.rowid, &d.version);
                done += 1;
            }
            Ok(Summarized::Skipped(why)) => {
                crate::log_debug(&format!("[library_summary] {} {} skipped: {why}", d.kind, d.item_id));
                release(index, d.rowid, false, now_ms());
                // Settled in the index only: a new version of the file is
                // looked at again.
                mark_summarized(index, d.rowid, &d.version);
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

// ── Full backfill run ────────────────────────────────────────────────────────
//
// The worker above drips the backlog at MAX_PER_HOUR. A full backfill the user
// approved instead runs at a bounded concurrency with the hourly cap lifted for
// that run only — it never touches the shared budget, so the drip keeps its
// own. The CLI runs it in the foreground, the desktop / `fleet serve` on a
// background thread. It is resumable by construction: a summarized item is no
// longer due, so a run that was interrupted continues where it stopped, and its
// counters (calls, cost, failures) carry over in `backfill.json` until a run
// ends with nothing left.

/// Most calls a backfill runs at once: the library's LLM slot gate.
pub const BACKFILL_CONCURRENCY: usize = classify::MAX_PARALLEL_CALLS;
/// Failures kept on the run record (the newest).
const MAX_RECORDED_FAILURES: usize = 200;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackfillState {
    Running,
    /// Ran out of due items (failures, if any, are listed).
    Done,
    /// Stopped on request or at `--limit`; the next run resumes it.
    Stopped,
    /// The process running it died; the next run resumes it.
    Interrupted,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackfillFailure {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub error: String,
}

/// One backfill, possibly over several resumed runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackfillRun {
    pub state: BackfillState,
    pub started_ms: u64,
    pub updated_ms: u64,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    /// When the current (or last) run segment started.
    #[serde(default)]
    pub segment_started_ms: u64,
    /// Wall-clock of the finished segments.
    #[serde(default)]
    pub elapsed_ms: u64,
    pub concurrency: usize,
    /// Items due when the backfill first started.
    pub planned: usize,
    pub done: usize,
    pub failed: usize,
    /// Items with nothing to summarize from (videos without a poster).
    #[serde(default)]
    pub skipped: usize,
    pub cost_usd: f64,
    #[serde(default)]
    pub resumes: u32,
    #[serde(default)]
    pub stop_requested: bool,
    #[serde(default)]
    pub failures: Vec<BackfillFailure>,
}

/// What the library's backfill button shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackfillStatus {
    pub plan: BackfillPlan,
    /// Everything due right now, backlog and new — what a run would do.
    pub due: usize,
    /// Estimated cost of `due`.
    pub due_cost_usd: f64,
    pub run: Option<BackfillRun>,
    /// A process is running a backfill right now.
    pub running: bool,
}

#[derive(Clone, Debug)]
pub struct BackfillOpts {
    pub concurrency: usize,
    /// Stop after this many items (a smoke run); the next run resumes.
    pub limit: Option<usize>,
}

impl Default for BackfillOpts {
    fn default() -> Self {
        Self { concurrency: BACKFILL_CONCURRENCY, limit: None }
    }
}

/// One finished item, for a caller printing progress.
#[derive(Clone, Debug)]
pub struct BackfillTick {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub cost_usd: f64,
    pub error: Option<String>,
    /// Why nothing was summarized, for a skipped item.
    pub skipped: Option<String>,
    pub run: BackfillRun,
}

fn run_file(roots: &Roots) -> std::path::PathBuf {
    roots.library.join("backfill.json")
}

fn load_run(roots: &Roots) -> Option<BackfillRun> {
    serde_json::from_str(&std::fs::read_to_string(run_file(roots)).ok()?).ok()
}

fn save_run(roots: &Roots, run: &BackfillRun) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(run).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&run_file(roots), &body)
        .map_err(|e| format!("write backfill record: {e}"))
}

/// Locked read-modify-write of the run record; `None` when there is none.
fn update_run<R>(roots: &Roots, f: impl FnOnce(&mut BackfillRun) -> R) -> Option<R> {
    crate::atomic_json::with_file_lock(&run_file(roots), || {
        let mut run = load_run(roots)?;
        let out = f(&mut run);
        run.updated_ms = now_ms();
        save_run(roots, &run).ok()?;
        Some(out)
    })
}

/// The OS lock a running backfill holds, so it dies with its process.
fn open_run_lock(roots: &Roots) -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(&roots.library);
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(roots.library.join("backfill.lock"))
        .ok()
}

/// How long a busy run lock must stay busy before it counts as a running
/// backfill. A running backfill holds it for minutes; other holders last
/// milliseconds: a child process this process is spawning carries a copy of
/// every descriptor until its exec closes them, and a status probe takes the
/// lock for an instant to test it. Without the grace, a run started right
/// after another ended, or racing a status poll, refused as "already running".
const RUN_LOCK_SETTLE: Duration = Duration::from_millis(250);

/// Take the run lock, retrying through brief holders; `false` when it stayed
/// held for the whole grace.
fn try_lock_settled(lock: &std::fs::File) -> bool {
    let started = std::time::Instant::now();
    loop {
        if lock.try_lock().is_ok() {
            return true;
        }
        if started.elapsed() >= RUN_LOCK_SETTLE {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn backfill_running(roots: &Roots) -> bool {
    match open_run_lock(roots) {
        Some(f) => !try_lock_settled(&f),
        None => false,
    }
}

fn count_due(index: &LibraryIndex) -> Result<(usize, f64), String> {
    index
        .conn()
        .query_row(
            &format!(
                "SELECT COUNT(*), COALESCE(SUM({VISION_SQL}), 0)
                 FROM items WHERE {DUE_WHERE}"
            ),
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )
        .map(|(all, images)| {
            let cost = (all - images) as f64 * EST_TEXT_CALL_USD + images as f64 * EST_VISION_CALL_USD;
            (all, cost)
        })
        .map_err(|e| format!("count: {e}"))
}

/// The backfill button's numbers. Syncs the index and extracts artifact text
/// for up to `extract_budget` first: an artifact whose text is not extracted
/// yet is not counted as due.
pub fn backfill_status_in(roots: &Roots, extract_budget: Duration) -> Result<BackfillStatus, String> {
    let index = LibraryIndex::open_at(&crate::library_index::db_path(roots))?;
    index.sync(roots)?;
    let started = std::time::Instant::now();
    while started.elapsed() < extract_budget
        && index.extract_pending(roots, extract_budget.saturating_sub(started.elapsed()))? > 0
    {}
    let plan = backfill_plan(&index)?;
    let (due, due_cost_usd) = count_due(&index)?;
    let running = backfill_running(roots);
    let mut run = load_run(roots);
    if let Some(r) = run.as_mut() {
        if r.state == BackfillState::Running && !running {
            r.state = BackfillState::Interrupted;
        }
    }
    Ok(BackfillStatus { plan, due, due_cost_usd, run, running })
}

pub fn backfill_status() -> Result<BackfillStatus, String> {
    backfill_status_in(&Roots::home()?, Duration::from_secs(2))
}

/// The next due row nobody holds, claimed. A row this run already finished
/// drops out on its own: a success is marked summarized, a failure gets a
/// back-off (cleared for every due row when the run starts, so each run
/// retries earlier failures once).
fn claim_next(index: &LibraryIndex) -> Result<Option<Due>, String> {
    loop {
        let now = now_ms();
        let rows: Vec<Due> = {
            let mut stmt = index
                .conn()
                .prepare(&format!(
                    "SELECT id, kind, item_id, title, version, artifact_kind, body, project FROM items
                     WHERE {DUE_WHERE} AND summary_claim_until_ms < ?1 AND summary_retry_ms <= ?1
                     ORDER BY updated_ms DESC
                     LIMIT 8"
                ))
                .map_err(|e| format!("prepare: {e}"))?;
            let rows = stmt
                .query_map(params![now as i64], |r| {
                    Ok(Due {
                        rowid: r.get(0)?,
                        kind: r.get(1)?,
                        item_id: r.get(2)?,
                        title: r.get(3)?,
                        version: r.get(4)?,
                        artifact_kind: r.get(5)?,
                        body: r.get(6)?,
                        project: r.get(7)?,
                    })
                })
                .map_err(|e| format!("query: {e}"))?;
            rows.flatten().collect()
        };
        if rows.is_empty() {
            return Ok(None);
        }
        for d in rows {
            if claim(index, d.rowid, now)? {
                return Ok(Some(d));
            }
        }
        // Every candidate was taken by another worker in between: look again.
    }
}

/// Summarize everything due, `opts.concurrency` at a time, without the hourly
/// cap. Refuses when another process is already running a backfill. Resumes an
/// unfinished record. `on_tick` is called after every item.
pub fn run_backfill_in(
    roots: &Roots,
    opts: &BackfillOpts,
    brain: &dyn Brain,
    on_tick: &(dyn Fn(&BackfillTick) + Sync),
) -> Result<BackfillRun, String> {
    let lock = open_run_lock(roots).ok_or("cannot open the backfill lock")?;
    if !try_lock_settled(&lock) {
        return Err("a backfill is already running".to_string());
    }
    let db = crate::library_index::db_path(roots);
    let index = LibraryIndex::open_at(&db)?;
    index.sync(roots)?;
    // An artifact whose text is not extracted yet is not due; extract it all
    // now so the run covers the whole store.
    while index.extract_pending(roots, Duration::from_secs(60))? > 0 {}
    let (due_now, _) = count_due(&index)?;
    index
        .conn()
        .execute(&format!("UPDATE items SET summary_retry_ms = 0 WHERE {DUE_WHERE}"), [])
        .map_err(|e| format!("reset back-off: {e}"))?;

    let now = now_ms();
    let concurrency = opts.concurrency.clamp(1, BACKFILL_CONCURRENCY);
    crate::atomic_json::with_file_lock(&run_file(roots), || {
        let mut run = match load_run(roots) {
            Some(mut r) if r.state != BackfillState::Done => {
                r.resumes += 1;
                r
            }
            _ => BackfillRun {
                state: BackfillState::Running,
                started_ms: now,
                updated_ms: now,
                finished_ms: None,
                segment_started_ms: now,
                elapsed_ms: 0,
                concurrency,
                planned: due_now,
                done: 0,
                failed: 0,
                skipped: 0,
                cost_usd: 0.0,
                resumes: 0,
                stop_requested: false,
                failures: Vec::new(),
            },
        };
        run.state = BackfillState::Running;
        run.stop_requested = false;
        run.concurrency = concurrency;
        run.segment_started_ms = now;
        run.finished_ms = None;
        run.updated_ms = now;
        save_run(roots, &run)
    })?;

    let taken = std::sync::atomic::AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let limited = AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..concurrency {
            s.spawn(|| {
                let Ok(index) = LibraryIndex::open_at(&db) else { return };
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if load_run(roots).is_some_and(|r| r.stop_requested) {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                    if opts.limit.is_some_and(|l| taken.fetch_add(1, Ordering::SeqCst) >= l) {
                        limited.store(true, Ordering::Relaxed);
                        break;
                    }
                    let d = match claim_next(&index) {
                        Ok(Some(d)) => d,
                        Ok(None) => break,
                        Err(e) => {
                            crate::log_debug(&format!("[library_summary] backfill: {e}"));
                            break;
                        }
                    };
                    let mut spent = 0.0;
                    let result = summarize_one(roots, &index, &d, brain, &mut spent, false);
                    let skipped = match &result {
                        Ok(Summarized::Skipped(why)) => Some(why.clone()),
                        _ => None,
                    };
                    let result = result.map(|_| ());
                    match &result {
                        Ok(_) => {
                            release(&index, d.rowid, false, now_ms());
                            mark_summarized(&index, d.rowid, &d.version);
                        }
                        Err(_) => release(&index, d.rowid, true, now_ms()),
                    }
                    let snapshot = update_run(roots, |r| {
                        // `failed` counts items failing now: a resumed run
                        // retries them, and a retry that works clears them.
                        let known = r.failures.iter().position(|f| f.kind == d.kind && f.id == d.item_id);
                        r.cost_usd += spent;
                        match &result {
                            Ok(()) if skipped.is_some() => r.skipped += 1,
                            Ok(()) => {
                                r.done += 1;
                                if let Some(i) = known {
                                    r.failures.remove(i);
                                    r.failed = r.failed.saturating_sub(1);
                                }
                            }
                            Err(e) => {
                                if let Some(i) = known {
                                    r.failures.remove(i);
                                } else {
                                    r.failed += 1;
                                }
                                r.failures.push(BackfillFailure {
                                    kind: d.kind.clone(),
                                    id: d.item_id.clone(),
                                    title: d.title.clone(),
                                    error: e.clone(),
                                });
                                let over = r.failures.len().saturating_sub(MAX_RECORDED_FAILURES);
                                r.failures.drain(..over);
                            }
                        }
                        r.clone()
                    });
                    if let Some(run) = snapshot {
                        on_tick(&BackfillTick {
                            kind: d.kind.clone(),
                            id: d.item_id.clone(),
                            title: d.title.clone(),
                            cost_usd: spent,
                            error: result.err(),
                            skipped,
                            run,
                        });
                    }
                }
            });
        }
    });

    // The summaries written above reach the index's text on this sync.
    let _ = index.sync(roots);
    let stopped = stop.load(Ordering::Relaxed) || limited.load(Ordering::Relaxed);
    let finished = update_run(roots, |r| {
        let end = now_ms();
        r.elapsed_ms += end.saturating_sub(r.segment_started_ms);
        r.state = if stopped { BackfillState::Stopped } else { BackfillState::Done };
        r.finished_ms = (!stopped).then_some(end);
        r.stop_requested = false;
        r.clone()
    })
    .ok_or("the backfill record disappeared")?;
    drop(lock);
    Ok(finished)
}

/// Start a full backfill on a background thread (the desktop / `fleet
/// serve` button). Answers the status as it stands right after starting.
pub fn start_backfill() -> Result<BackfillStatus, String> {
    let roots = Roots::home()?;
    if backfill_running(&roots) {
        return Err("a backfill is already running".to_string());
    }
    let thread_roots = roots.clone();
    std::thread::Builder::new()
        .name("library-backfill".into())
        .spawn(move || {
            if let Err(e) =
                run_backfill_in(&thread_roots, &BackfillOpts::default(), &LiveBrain, &|_| {})
            {
                crate::log_debug(&format!("[library_summary] backfill failed: {e}"));
            }
        })
        .map_err(|e| format!("start backfill: {e}"))?;
    // Let the thread take its lock and write its record before answering.
    for _ in 0..50 {
        if backfill_running(&roots) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    backfill_status_in(&roots, Duration::ZERO)
}

/// Ask a running backfill to stop after the items in flight.
pub fn stop_backfill() -> Result<BackfillStatus, String> {
    let roots = Roots::home()?;
    update_run(&roots, |r| r.stop_requested = true).ok_or("no backfill has run")?;
    backfill_status_in(&roots, Duration::ZERO)
}

// ── Language repair ─────────────────────────────────────────────────────────
//
// Summaries follow their project's language (`LibraryIndex::project_language`).
// Ones written before that rule, under "the language of the file's own
// content", are English in Chinese projects. They are found by comparing each
// AI summary's language with its project's and rewritten in place; the old
// summaries are backed up first so the rewrite can be undone.

/// An AI summary not written in its project's language.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanguageMismatch {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub project: String,
    /// `zh` or `en`.
    pub project_language: String,
    pub summary: String,
}

fn lang_code(l: classify::Language) -> &'static str {
    match l {
        classify::Language::Chinese => "zh",
        classify::Language::English => "en",
    }
}

/// Whether `summary` is clearly written in another language than `project`.
/// A summary too short or too mixed to tell is never a mismatch.
pub fn summary_language_mismatch(summary: &str, project: classify::Language) -> bool {
    classify::text_language(summary).is_some_and(|l| l != project)
}

/// The AI summary of an item: a doc's `summary.json`, an artifact's `summary`
/// (never its note, which the person or agent who added it wrote).
fn ai_summary(roots: &Roots, kind: &str, id: &str) -> Option<(String, Vec<String>)> {
    match kind {
        ITEM_DOC => wiki::read_summary_in(&roots.wiki, id).map(|s| (s.summary, s.tags)),
        _ => artifacts::get_in(&roots.artifacts, id).ok().map(|a| (a.summary, a.tags)),
    }
    .filter(|(s, _)| !s.trim().is_empty())
}

/// Every item whose AI summary is not in its project's language, from the
/// index as last synced.
pub fn language_mismatches(roots: &Roots, index: &LibraryIndex) -> Result<Vec<LanguageMismatch>, String> {
    let rows: Vec<(String, String, String, String)> = {
        let mut stmt = index
            .conn()
            .prepare("SELECT kind, item_id, title, project FROM items WHERE summary_version != '' ORDER BY project, updated_ms DESC")
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| format!("query: {e}"))?;
        rows.flatten().collect()
    };
    let mut langs: std::collections::HashMap<String, classify::Language> = Default::default();
    let mut out = Vec::new();
    for (kind, id, title, project) in rows {
        let Some((summary, _)) = ai_summary(roots, &kind, &id) else { continue };
        let lang = *langs
            .entry(project.clone())
            .or_insert_with(|| index.project_language(&project, &[]));
        if summary_language_mismatch(&summary, lang) {
            out.push(LanguageMismatch {
                kind,
                id,
                title,
                project,
                project_language: lang_code(lang).to_string(),
                summary,
            });
        }
    }
    Ok(out)
}

/// One summary as it was before a re-summarize overwrote it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SummaryBackup {
    pub kind: String,
    pub id: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// A doc's whole sidecar (version and time included); `None` for an
    /// artifact.
    #[serde(default)]
    pub doc: Option<wiki::DocSummary>,
}

/// The result of [`resummarize_in`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResummarizeReport {
    pub planned: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cost_usd: f64,
    /// Where the summaries it replaced are kept; [`restore_summaries_in`]
    /// puts them back. `None` when there was nothing to do.
    pub backup: Option<std::path::PathBuf>,
    pub failures: Vec<BackfillFailure>,
}

/// Back-ups of replaced summaries, one file per re-summarize run.
fn backup_dir(roots: &Roots) -> std::path::PathBuf {
    roots.library.join("summary-backups")
}

/// Rewrite the summaries of `targets` (`(kind, id)`), `concurrency` at a
/// time and at most `limit` of them, after writing every summary it is about
/// to replace to a backup file. Not counted against the hourly budget: the
/// caller asked for exactly these. `on_tick` gets each finished item.
pub fn resummarize_in(
    roots: &Roots,
    targets: &[(String, String)],
    concurrency: usize,
    limit: Option<usize>,
    brain: &dyn Brain,
    on_tick: &(dyn Fn(&BackfillTick) + Sync),
) -> Result<ResummarizeReport, String> {
    let db = crate::library_index::db_path(roots);
    let index = LibraryIndex::open_at(&db)?;
    index.sync(roots)?;
    // A text artifact is summarized from its extracted text.
    while index.extract_pending(roots, Duration::from_secs(60))? > 0 {}
    let mut due: Vec<Due> = Vec::new();
    for (kind, id) in targets.iter().take(limit.unwrap_or(usize::MAX)) {
        let d = index
            .conn()
            .query_row(
                "SELECT id, kind, item_id, title, version, artifact_kind, body, project FROM items
                 WHERE kind = ?1 AND item_id = ?2",
                params![kind, id],
                |r| {
                    Ok(Due {
                        rowid: r.get(0)?,
                        kind: r.get(1)?,
                        item_id: r.get(2)?,
                        title: r.get(3)?,
                        version: r.get(4)?,
                        artifact_kind: r.get(5)?,
                        body: r.get(6)?,
                        project: r.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("look up {kind} {id}: {e}"))?;
        match d {
            Some(d) if mode_of(&d).is_some() => due.push(d),
            _ => return Err(format!("{kind} '{id}' is not in the library or has nothing to summarize")),
        }
    }
    let mut report = ResummarizeReport {
        planned: due.len(),
        done: 0,
        failed: 0,
        skipped: 0,
        cost_usd: 0.0,
        backup: None,
        failures: Vec::new(),
    };
    if due.is_empty() {
        return Ok(report);
    }

    let backups: Vec<SummaryBackup> = due
        .iter()
        .map(|d| {
            let (summary, tags) = ai_summary(roots, &d.kind, &d.item_id).unwrap_or_default();
            SummaryBackup {
                kind: d.kind.clone(),
                id: d.item_id.clone(),
                summary,
                tags,
                doc: (d.kind == ITEM_DOC).then(|| wiki::read_summary_in(&roots.wiki, &d.item_id)).flatten(),
            }
        })
        .collect();
    let path = backup_dir(roots).join(format!("{}.json", now_ms()));
    std::fs::create_dir_all(backup_dir(roots)).map_err(|e| format!("backup dir: {e}"))?;
    let body = serde_json::to_vec_pretty(&backups).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path, &body).map_err(|e| format!("write backup: {e}"))?;
    report.backup = Some(path);

    let next = std::sync::atomic::AtomicUsize::new(0);
    let report = std::sync::Mutex::new(report);
    std::thread::scope(|s| {
        for _ in 0..concurrency.clamp(1, BACKFILL_CONCURRENCY) {
            s.spawn(|| {
                let Ok(index) = LibraryIndex::open_at(&db) else { return };
                loop {
                    let Some(d) = due.get(next.fetch_add(1, Ordering::SeqCst)) else { break };
                    let mut spent = 0.0;
                    let result = summarize_one(roots, &index, d, brain, &mut spent, true);
                    if matches!(result, Ok(Summarized::Done)) {
                        mark_summarized(&index, d.rowid, &d.version);
                    }
                    let mut r = report.lock().unwrap_or_else(|e| e.into_inner());
                    r.cost_usd += spent;
                    let (error, skipped) = match result {
                        Ok(Summarized::Done) => {
                            r.done += 1;
                            (None, None)
                        }
                        Ok(Summarized::Skipped(why)) => {
                            r.skipped += 1;
                            (None, Some(why))
                        }
                        Err(e) => {
                            r.failed += 1;
                            r.failures.push(BackfillFailure {
                                kind: d.kind.clone(),
                                id: d.item_id.clone(),
                                title: d.title.clone(),
                                error: e.clone(),
                            });
                            (Some(e), None)
                        }
                    };
                    let run = BackfillRun {
                        state: BackfillState::Running,
                        started_ms: 0,
                        updated_ms: now_ms(),
                        finished_ms: None,
                        segment_started_ms: 0,
                        elapsed_ms: 0,
                        concurrency,
                        planned: r.planned,
                        done: r.done,
                        failed: r.failed,
                        skipped: r.skipped,
                        cost_usd: r.cost_usd,
                        resumes: 0,
                        stop_requested: false,
                        failures: Vec::new(),
                    };
                    drop(r);
                    on_tick(&BackfillTick {
                        kind: d.kind.clone(),
                        id: d.item_id.clone(),
                        title: d.title.clone(),
                        cost_usd: spent,
                        error,
                        skipped,
                        run,
                    });
                }
            });
        }
    });
    let _ = index.sync(roots);
    Ok(report.into_inner().unwrap_or_else(|e| e.into_inner()))
}

/// Put the summaries a [`resummarize_in`] backup holds back. Returns how many
/// were restored.
pub fn restore_summaries_in(roots: &Roots, backup: &std::path::Path) -> Result<usize, String> {
    let body = std::fs::read_to_string(backup).map_err(|e| format!("read {}: {e}", backup.display()))?;
    let items: Vec<SummaryBackup> = serde_json::from_str(&body).map_err(|e| format!("parse backup: {e}"))?;
    let mut restored = 0;
    for b in &items {
        match b.kind.as_str() {
            ITEM_DOC => {
                let Some(doc) = &b.doc else { continue };
                wiki::write_summary_in(&roots.wiki, &b.id, doc)?;
            }
            _ => {
                artifacts::edit_in(&roots.artifacts, &b.id, |a| {
                    a.summary = b.summary.clone();
                    a.tags = b.tags.clone();
                })?;
            }
        }
        restored += 1;
    }
    if let Ok(index) = LibraryIndex::open_at(&crate::library_index::db_path(roots)) {
        let _ = index.sync(roots);
    }
    Ok(restored)
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
        // Answers that failed the 2026-10-21 backfill: bare quotes inside the summary.
        let (s, _) = parse_summary("```json\n{\"summary\":\"标题为\"新渠道能否带来持续增量\"的记录页。\",\"tags\":[\"投资\"]}\n```").unwrap();
        assert_eq!(s, "标题为\"新渠道能否带来持续增量\"的记录页。");
        let p = summary_prompt("T", "document", &"x".repeat(10_000), classify::Language::Chinese);
        assert!(p.contains("Title: T") && p.len() < 5_000);
        assert!(p.contains(classify::Language::Chinese.rule()));
        let en = summary_prompt("T", "document", "body", classify::Language::English);
        assert!(en.contains(classify::Language::English.rule()) && !en.contains(classify::Language::Chinese.rule()));
        let v = vision_summary_prompt("a.png", "T", classify::Language::English);
        assert!(v.contains(classify::Language::English.rule()));
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
    fn an_artifact_that_already_has_a_summary_is_not_due() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        let add = |name: &str| {
            let src = ws.join(name);
            std::fs::write(&src, b"plain text body").unwrap();
            artifacts::add_in(&roots.artifacts, &src, None, None, &ws, None).unwrap()
        };
        let bare = add("bare.txt");
        let filed = add("filed.txt");
        // The upload organizer wrote this one's summary already.
        artifacts::edit_in(&roots.artifacts, &filed.id, |a| a.summary = "from upload".into()).unwrap();
        let index = LibraryIndex::open_at(&crate::library_index::db_path(&roots)).unwrap();
        index.meta_set("activated_ms", "0").unwrap();
        index.sync(&roots).unwrap();
        index.extract_pending(&roots, Duration::from_secs(10)).unwrap();
        assert!(is_due(&index, ITEM_ARTIFACT, &bare.id));
        assert!(!is_due(&index, ITEM_ARTIFACT, &filed.id));
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

    /// Answers every text prompt with a summary, except prompts naming
    /// `FAILME`, which get a paid-for answer that is not JSON; counts calls.
    struct FakeBrain(std::sync::atomic::AtomicUsize);

    impl Brain for FakeBrain {
        fn text(
            &self,
            _slot: crate::llm_provider::ModelSlot,
            prompt: &str,
        ) -> Option<classify::CallOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            let text = if prompt.contains("FAILME") {
                "Sorry, I cannot summarize this."
            } else {
                r#"{"summary":"A fake summary.","tags":["fake"]}"#
            };
            Some(classify::CallOutcome { text: text.into(), cost_usd: 0.01, ..Default::default() })
        }
        fn vision(&self, _dir: &std::path::Path, _prompt: &str) -> Result<classify::CallOutcome, String> {
            Err("no vision in tests".into())
        }
    }

    #[test]
    fn a_backfill_runs_past_the_hourly_cap_resumes_and_accounts_cost() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        for i in 0..6 {
            wiki::publish_text_in(&roots.wiki, &format!("d{i}"), None, &format!("body {i}"), &ws, wiki::TextPublishMode::Replace)
                .unwrap();
        }
        std::thread::sleep(Duration::from_millis(5));
        wiki::publish_text_in(&roots.wiki, "bad", None, "FAILME", &ws, wiki::TextPublishMode::Replace).unwrap();
        let index = LibraryIndex::open_at(&crate::library_index::db_path(&roots)).unwrap();
        // Everything predates the index: backlog only, which the drip skips.
        index.meta_set("activated_ms", &(now_ms() + 1).to_string()).unwrap();
        // Spend the hour's budget: a backfill must not care.
        for _ in 0..MAX_PER_HOUR {
            take_budget(&index, now_ms()).unwrap();
        }
        let status = backfill_status_in(&roots, Duration::ZERO).unwrap();
        assert_eq!((status.due, status.running, status.run.is_none()), (7, false, true));

        let brain = FakeBrain(Default::default());
        let ticks = std::sync::Mutex::new(0usize);
        let tick = |_: &BackfillTick| *ticks.lock().unwrap() += 1;
        let first = run_backfill_in(&roots, &BackfillOpts { concurrency: 3, limit: Some(3) }, &brain, &tick).unwrap();
        assert_eq!(first.state, BackfillState::Stopped, "a limited run stops, to be resumed");
        assert_eq!(first.done + first.failed, 3);

        let done = run_backfill_in(&roots, &BackfillOpts::default(), &brain, &tick).unwrap();
        assert_eq!(done.state, BackfillState::Done);
        assert_eq!((done.planned, done.done, done.failed, done.resumes), (7, 6, 1, 1), "{done:?}");
        // Six summaries plus two answers for `bad` that did not parse: those
        // were paid for too.
        assert!((done.cost_usd - 0.08).abs() < 1e-9, "cost carried over the resume: {}", done.cost_usd);
        assert_eq!(done.failures[0].id, "bad");
        assert_eq!(done.failures[0].error, "unparseable summary: Sorry, I cannot summarize this.");
        // `bad` is the newest, so the first run tried it; the resumed run
        // retried it once more. Everything else was called exactly once.
        assert_eq!(*ticks.lock().unwrap(), 8);
        assert_eq!(brain.0.load(Ordering::SeqCst), 8);
        assert_eq!(wiki::get_doc_in(&roots.wiki, "d3").unwrap().summary, "A fake summary.");

        // A new run starts a fresh record and retries the failure once.
        let again = run_backfill_in(&roots, &BackfillOpts::default(), &brain, &tick).unwrap();
        assert_eq!((again.planned, again.done, again.failed), (1, 0, 1));
        let status = backfill_status_in(&roots, Duration::ZERO).unwrap();
        assert_eq!(status.due, 1);
    }

    #[test]
    fn a_stop_request_ends_the_run_and_a_held_lock_refuses_a_second_one() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        for i in 0..4 {
            wiki::publish_text_in(&roots.wiki, &format!("d{i}"), None, "body", &ws, wiki::TextPublishMode::Replace)
                .unwrap();
        }
        let brain = FakeBrain(Default::default());
        let stop_after_first = |t: &BackfillTick| {
            if t.run.done == 1 {
                update_run(&roots, |r| r.stop_requested = true);
            }
        };
        let run = run_backfill_in(&roots, &BackfillOpts { concurrency: 1, limit: None }, &brain, &stop_after_first).unwrap();
        assert_eq!((run.state, run.done), (BackfillState::Stopped, 1));

        let held = open_run_lock(&roots).unwrap();
        held.try_lock().unwrap();
        assert!(backfill_running(&roots));
        let err = run_backfill_in(&roots, &BackfillOpts::default(), &brain, &|_| {}).unwrap_err();
        assert!(err.contains("already running"), "{err}");
    }

    #[test]
    fn a_lock_held_only_for_an_instant_does_not_refuse_a_run() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        wiki::publish_text_in(&roots.wiki, "d", None, "body", &ws, wiki::TextPublishMode::Replace).unwrap();
        // What a child process being spawned, or a status probe, does: hold
        // the lock through a descriptor of its own for a moment.
        let brief = open_run_lock(&roots).unwrap();
        brief.try_lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(brief);
        });
        let brain = FakeBrain(Default::default());
        let run = run_backfill_in(&roots, &BackfillOpts::default(), &brain, &|_| {}).unwrap();
        assert_eq!(run.state, BackfillState::Done);
        release.join().unwrap();
    }

    #[test]
    fn a_summary_clearly_in_the_other_language_is_a_mismatch() {
        use classify::Language::{Chinese, English};
        assert!(summary_language_mismatch(
            "A technical spike proposal for Fleet's cloud API and hosted UX architecture.",
            Chinese
        ));
        // Chinese prose full of identifiers is still Chinese.
        assert!(!summary_language_mismatch(
            "酒店收据，由Air Masters Travel Limited（旅行社）为Insnap Technology Limited开具，记录入住信息。",
            Chinese
        ));
        assert!(summary_language_mismatch("一份装修合同，记录了工期与付款节点。", English));
        // Too little to tell either way.
        assert!(!summary_language_mismatch("v4 vs v5", Chinese));
        assert!(!summary_language_mismatch("", English));
    }

    /// Answers every call with a Chinese summary; records the vision prompts
    /// and whether the poster was in the directory it was handed.
    #[derive(Default)]
    struct ChineseBrain {
        calls: std::sync::atomic::AtomicUsize,
        vision_prompts: std::sync::Mutex<Vec<(String, bool)>>,
    }

    const ZH_ANSWER: &str = r#"{"summary":"一段重新生成的中文摘要，说明这份资料的内容。","tags":["中文"]}"#;

    impl Brain for ChineseBrain {
        fn text(&self, _slot: crate::llm_provider::ModelSlot, _prompt: &str) -> Option<classify::CallOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Some(classify::CallOutcome { text: ZH_ANSWER.into(), cost_usd: 0.01, ..Default::default() })
        }
        fn vision(&self, dir: &std::path::Path, prompt: &str) -> Result<classify::CallOutcome, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let poster = dir.join(POSTER_FILE);
            let is_jpeg = std::fs::read(&poster).is_ok_and(|b| b.starts_with(&[0xFF, 0xD8]));
            self.vision_prompts.lock().unwrap().push((prompt.to_string(), is_jpeg));
            Ok(classify::CallOutcome { text: ZH_ANSWER.into(), cost_usd: 0.02, ..Default::default() })
        }
    }

    #[test]
    fn mismatched_summaries_are_found_rewritten_with_a_backup_and_restorable() {
        let tmp = TempDir::new().unwrap();
        let (roots, ws) = roots(&tmp);
        let publish = |slug: &str, title: &str, body: &str| {
            wiki::publish_text_in(&roots.wiki, slug, Some(title), body, &ws, wiki::TextPublishMode::Replace).unwrap()
        };
        let en_doc = publish("spike", "云端架构探索", "Fleet 云端 API 与托管体验的架构探索，记录了几种方案的取舍");
        let zh_doc = publish("notes", "构建笔记", "这是一份关于构建流程的中文笔记，记录了几次失败");
        publish("deploy", "部署记录", "上线步骤与回滚预案，以及值班联系人");
        let write = |slug: &str, version: &str, summary: &str, tag: &str| {
            wiki::write_summary_in(
                &roots.wiki,
                slug,
                &wiki::DocSummary { summary: summary.into(), tags: vec![tag.into()], version: version.into(), generated_ms: 7 },
            )
            .unwrap()
        };
        let en_summary = "A technical spike proposal for the cloud API and hosted UX architecture.";
        write("spike", &en_doc.current_version, en_summary, "architecture");
        write("notes", &zh_doc.current_version, "关于构建流程的中文笔记。", "构建");
        let src = ws.join("store.txt");
        std::fs::write(&src, "Store listing copy for the mobile app, in English, final draft").unwrap();
        let art = artifacts::add_in(&roots.artifacts, &src, Some("商店描述文案"), None, &ws, None).unwrap();
        artifacts::edit_in(&roots.artifacts, &art.id, |a| {
            a.summary = "Store listing copy for Fleet, a mobile app for coding agents.".into();
            a.tags = vec!["store".into()];
        })
        .unwrap();

        let index = LibraryIndex::open_at(&crate::library_index::db_path(&roots)).unwrap();
        index.sync(&roots).unwrap();
        let found = language_mismatches(&roots, &index).unwrap();
        let mut ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, [art.id.as_str(), "spike"], "{found:?}");
        assert!(found.iter().all(|m| m.project_language == "zh"));

        let brain = ChineseBrain::default();
        let targets: Vec<(String, String)> = found.iter().map(|m| (m.kind.clone(), m.id.clone())).collect();
        let report = resummarize_in(&roots, &targets, 2, None, &brain, &|_| {}).unwrap();
        assert_eq!((report.planned, report.done, report.failed), (2, 2, 0), "{report:?}");
        assert!((report.cost_usd - 0.02).abs() < 1e-9);
        let doc = wiki::read_summary_in(&roots.wiki, "spike").unwrap();
        assert!(doc.summary.starts_with("一段重新生成") && doc.version == en_doc.current_version);
        let a = artifacts::get_in(&roots.artifacts, &art.id).unwrap();
        assert_eq!((a.summary.starts_with("一段重新生成"), a.tags), (true, vec!["中文".to_string()]), "forced over the old one");
        assert!(language_mismatches(&roots, &index).unwrap().is_empty());

        let backup = report.backup.unwrap();
        assert_eq!(restore_summaries_in(&roots, &backup).unwrap(), 2);
        let doc = wiki::read_summary_in(&roots.wiki, "spike").unwrap();
        assert_eq!((doc.summary.as_str(), doc.generated_ms), (en_summary, 7));
        assert_eq!(artifacts::get_in(&roots.artifacts, &art.id).unwrap().tags, vec!["store".to_string()]);
        assert_eq!(language_mismatches(&roots, &index).unwrap().len(), 2);
    }

    #[test]
    fn a_video_prompt_carries_its_title_file_folder_and_note() {
        let v = VideoBrief {
            title: "露里子修复前后".into(),
            file_name: "kick_before_after.mp4".into(),
            folder: "录屏/布料".into(),
            note: "上=修前，下=修后".into(),
        };
        let p = video_summary_prompt(&v, classify::Language::Chinese);
        for part in ["露里子修复前后", "kick_before_after.mp4", "录屏/布料", "上=修前，下=修后", POSTER_FILE] {
            assert!(p.contains(part), "missing {part}: {p}");
        }
        assert!(p.contains(classify::Language::Chinese.rule()));
        let bare = video_summary_prompt(&VideoBrief { title: "t".into(), file_name: "t.mp4".into(), ..Default::default() }, classify::Language::English);
        assert!(!bare.contains("Folder:") && !bare.contains("Note from"));
    }

    #[test]
    fn videos_are_summarized_from_their_poster_and_ones_without_one_are_skipped() {
        let tmp = TempDir::new().unwrap();
        let _home = crate::paths::fleet_home_guard(tmp.path().join("home"));
        let (roots, ws) = roots(&tmp);
        let add = |name: &str, bytes: &[u8], note: &str| {
            let src = ws.join(name);
            std::fs::write(&src, bytes).unwrap();
            let a = artifacts::add_in(&roots.artifacts, &src, Some("走路循环对比"), None, &ws, None).unwrap();
            artifacts::edit_in(&roots.artifacts, &a.id, |a| a.note = note.into()).unwrap()
        };
        let broken = add("broken.mp4", b"definitely not an mp4", "坏文件");
        assert_eq!(broken.kind, "video");
        let ffmpeg = crate::process_util::which("ffmpeg");
        let clip = ffmpeg.as_ref().and_then(|ff| {
            let out = ws.join("src.mp4");
            let ok = crate::process_util::command(ff)
                .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x180:rate=10:duration=1"])
                .args(["-pix_fmt", "yuv420p", "-y"])
                .arg(&out)
                .status()
                .ok()?
                .success();
            ok.then(|| add("walk.mp4", &std::fs::read(&out).unwrap(), "左旧右新，同一段动作"))
        });

        let index = LibraryIndex::open_at(&crate::library_index::db_path(&roots)).unwrap();
        index.meta_set("activated_ms", "0").unwrap();
        index.sync(&roots).unwrap();
        assert!(is_due(&index, ITEM_ARTIFACT, &broken.id), "a video is due like an image");
        let plan = backfill_plan(&index).unwrap();
        assert_eq!(plan.image_artifacts, 0);

        let brain = ChineseBrain::default();
        let run = run_backfill_in(&roots, &BackfillOpts::default(), &brain, &|_| {}).unwrap();
        assert_eq!(run.failed, 0, "a video without a poster is not a failure: {run:?}");
        assert_eq!(run.skipped, 1);
        assert!(!is_due(&index, ITEM_ARTIFACT, &broken.id), "settled, not retried every pass");
        assert!(artifacts::get_in(&roots.artifacts, &broken.id).unwrap().summary.is_empty());

        let Some(clip) = clip else { return };
        assert_eq!(run.done, 1);
        let prompts = brain.vision_prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1, "only the decodable video costs a call");
        let (prompt, saw_jpeg) = &prompts[0];
        assert!(saw_jpeg, "the poster was handed over as a JPEG");
        assert!(prompt.contains("walk.mp4") && prompt.contains("左旧右新") && prompt.contains("走路循环对比"));
        let a = artifacts::get_in(&roots.artifacts, &clip.id).unwrap();
        assert!(a.summary.starts_with("一段重新生成") && a.tags == vec!["中文".to_string()]);
        assert!(!is_due(&index, ITEM_ARTIFACT, &clip.id));
    }

    /// Per-call cost of real summaries, read-only: point
    /// `FLEET_LIBRARY_MEASURE_DIR` at the scratch index `measure_real_store`
    /// built. Three doc bodies go through the lean text route; one image is
    /// copied into the scratch dir for the vision route. Nothing is written
    /// back to either store.
    #[test]
    #[ignore]
    fn measure_summary_cost() {
        let Ok(dir) = std::env::var("FLEET_LIBRARY_MEASURE_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let home = Roots::home().unwrap();
        let roots = Roots { library: dir.clone(), wiki: home.wiki.clone(), artifacts: home.artifacts.clone() };
        let index = LibraryIndex::open_at(&crate::library_index::db_path(&roots)).unwrap();
        let docs: Vec<(String, String)> = {
            let mut stmt = index
                .conn()
                .prepare(
                    "SELECT title, body FROM items WHERE kind = 'doc' AND length(body) > 2000
                     ORDER BY updated_ms DESC LIMIT 3",
                )
                .unwrap();
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            rows.flatten().collect()
        };
        for (title, body) in docs {
            let t = std::time::Instant::now();
            let out = classify::complete_text(
                crate::llm_provider::ModelSlot::Fast,
                &summary_prompt(&title, "document", &body, classify::Language::Chinese),
            );
            match out {
                Some(o) => println!(
                    "text  {:.1}s ${:.5} {}/{} body {} chars -> {:?}",
                    t.elapsed().as_secs_f64(), o.cost_usd, o.provider, o.model,
                    body.chars().count(), parse_summary(&o.text)
                ),
                None => println!("text  no answer for {title}"),
            }
        }
        let img: Option<(String, String)> = index
            .conn()
            .query_row(
                "SELECT item_id, title FROM items WHERE artifact_kind = 'image'
                 ORDER BY updated_ms DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap();
        if let Some((id, title)) = img {
            let a = artifacts::get_in(&roots.artifacts, &id).unwrap();
            let scratch = dir.join("vision-sample");
            std::fs::create_dir_all(&scratch).unwrap();
            std::fs::copy(artifacts::blob_path(&roots.artifacts, &a), scratch.join(&a.name)).unwrap();
            let t = std::time::Instant::now();
            match classify::complete_vision(&scratch, &vision_summary_prompt(&a.name, &title, classify::Language::Chinese)) {
                Ok(o) => println!(
                    "vision {:.1}s ${:.5} {} -> {:?}",
                    t.elapsed().as_secs_f64(), o.cost_usd, o.model, parse_summary(&o.text)
                ),
                Err(e) => println!("vision failed: {e}"),
            }
        }
    }
}
