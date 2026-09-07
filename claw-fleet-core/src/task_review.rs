//! Per-**task** retrospective: when a task reaches a terminal state, review that
//! task's whole trace once, while it is fresh.
//!
//! ## Why this exists alongside the daily report
//!
//! The daily report reviews a *calendar day*: every session that was active,
//! folded together, with no idea which of them worked out. That framing has two
//! costs. It cannot tell a lesson learned from a task that succeeded apart from
//! one learned from a task that was abandoned — the single most informative bit
//! about a trace — and it fires up to 24 hours after the fact.
//!
//! The v3 decision card supplies the missing bit: [`TaskOutcome`] is stamped the
//! moment the user presses 结束任务 / 放弃任务. So the review runs *there*, on
//! that one task, knowing how it ended. The daily report then aggregates these
//! finished reviews instead of re-deriving everything from raw transcripts.
//!
//! ## What counts as one task
//!
//! A **handoff chain**, not a session. `fleet handoff` splits one piece of work
//! across N sessions on purpose; reviewing each hop separately would produce N
//! disconnected reviews of what the user experienced as one task, and the
//! macro goal — the thing worth learning about — lives across the seam. So the
//! chain is resolved at review time and its hops' traces are concatenated in
//! hop order. A session with no chain is a one-hop task.
//!
//! ## Where it runs
//!
//! [`on_task_terminated`] returns immediately and does the work on a detached
//! thread: it is called from the decision-card response path, which is
//! unblocking a waiting agent and must not sit through an LLM round trip.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::daily_report::{extract_conversation_pairs, ConversationPair, Lesson};
use crate::log_debug;
use crate::task_outcome::TaskOutcome;

/// How long the review LLM call may take. Same order as the daily report's
/// lessons pass, which reads a comparable volume of transcript.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(180);

/// Cap on decision-card evidence folded into one review.
const MAX_DECISION_SIGNALS: usize = 20;

/// Cap on conversation pairs. A long chain can carry hundreds; the tail of a
/// task is where the outcome was decided, so the *last* N are kept, not the
/// first.
const MAX_PAIRS: usize = 60;

/// A finished retrospective for one task.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskReview {
    /// First session of the handoff chain — the task's stable identity, and the
    /// primary key. Re-terminating the same task overwrites its review rather
    /// than accumulating duplicates.
    pub root_session_id: String,
    /// Every session in the chain, in hop order. One element for a task that
    /// never handed off.
    pub session_ids: Vec<String>,
    pub workspace_name: String,
    pub workspace_path: String,
    /// How it ended, per the user's press.
    pub outcome: TaskOutcome,
    /// What the agent claimed on the card that ended it (`taskComplete`).
    /// `true` here with `outcome: abandoned` is the disagreement signal.
    pub agent_claimed_complete: bool,
    /// One-line restatement of what the task was, from the LLM.
    pub title: String,
    /// The retrospective prose: what happened and why it ended this way.
    pub summary: String,
    /// Transferable lessons, in the same shape the daily report uses so both
    /// feed the one Memory panel and the one `lessons_store`.
    pub lessons: Vec<Lesson>,
    /// Epoch ms the terminal state was stamped.
    pub terminated_at: u64,
    /// Epoch ms the review finished.
    pub generated_at: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Storage ──────────────────────────────────────────────────────────────────

/// Task reviews live in the same `fleet-reports.db` as the daily reports: the
/// daily report *reads* them, and keeping one database means one file to back
/// up, one WAL, and no cross-database join to write.
pub struct TaskReviewStore {
    conn: Connection,
}

impl TaskReviewStore {
    pub fn open() -> Result<Self, String> {
        let db_path = crate::session::real_home_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?
            .join(".fleet")
            .join("fleet-reports.db");
        Self::open_at(&db_path)
    }

    pub fn open_at(db_path: &std::path::Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path).map_err(|e| format!("sqlite open: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| format!("sqlite pragma: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_reviews (
                 root_session_id       TEXT PRIMARY KEY,
                 session_ids           TEXT NOT NULL,
                 workspace_name        TEXT NOT NULL,
                 workspace_path        TEXT NOT NULL,
                 outcome               TEXT NOT NULL,
                 agent_claimed_complete INTEGER NOT NULL,
                 title                 TEXT NOT NULL,
                 summary               TEXT NOT NULL,
                 lessons               TEXT NOT NULL,
                 terminated_at         INTEGER NOT NULL,
                 generated_at          INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS task_reviews_terminated
                 ON task_reviews (terminated_at);",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;
        Ok(Self { conn })
    }

    pub fn save(&self, review: &TaskReview) -> Result<(), String> {
        let ids = serde_json::to_string(&review.session_ids)
            .map_err(|e| format!("json encode session_ids: {e}"))?;
        let lessons =
            serde_json::to_string(&review.lessons).map_err(|e| format!("json encode lessons: {e}"))?;
        let outcome = match review.outcome {
            TaskOutcome::Completed => "completed",
            TaskOutcome::Abandoned => "abandoned",
        };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO task_reviews
                 (root_session_id, session_ids, workspace_name, workspace_path, outcome,
                  agent_claimed_complete, title, summary, lessons, terminated_at, generated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    review.root_session_id,
                    ids,
                    review.workspace_name,
                    review.workspace_path,
                    outcome,
                    review.agent_claimed_complete as i32,
                    review.title,
                    review.summary,
                    lessons,
                    review.terminated_at,
                    review.generated_at,
                ],
            )
            .map_err(|e| format!("insert task_review: {e}"))?;
        Ok(())
    }

    /// Reviews whose task terminated within `[from_ms, to_ms)`, newest first.
    /// This is the daily report's read path.
    pub fn list_in_range(&self, from_ms: u64, to_ms: u64) -> Result<Vec<TaskReview>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT root_session_id, session_ids, workspace_name, workspace_path, outcome,
                        agent_claimed_complete, title, summary, lessons, terminated_at, generated_at
                 FROM task_reviews
                 WHERE terminated_at >= ?1 AND terminated_at < ?2
                 ORDER BY terminated_at DESC",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(params![from_ms, to_ms], row_to_review)
            .map_err(|e| format!("query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("row: {e}"))
    }

    /// One review by its task identity. Queries the primary key rather than
    /// scanning a range: `list_in_range(0, u64::MAX)` looks equivalent but is
    /// not — rusqlite binds a `u64` as SQLite's signed 64-bit INTEGER, so
    /// `u64::MAX` arrives as `-1` and the `terminated_at < ?` bound excludes
    /// every row.
    pub fn get(&self, root_session_id: &str) -> Option<TaskReview> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT root_session_id, session_ids, workspace_name, workspace_path, outcome,
                        agent_claimed_complete, title, summary, lessons, terminated_at, generated_at
                 FROM task_reviews WHERE root_session_id = ?1",
            )
            .ok()?;
        stmt.query_row(params![root_session_id], row_to_review).ok()
    }
}

/// Shared row → [`TaskReview`] mapper for every query in this module. Column
/// order is fixed by the `SELECT` lists above.
fn row_to_review(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskReview> {
    let ids: String = row.get(1)?;
    let outcome: String = row.get(4)?;
    let lessons: String = row.get(8)?;
    Ok(TaskReview {
        root_session_id: row.get(0)?,
        session_ids: serde_json::from_str(&ids).unwrap_or_default(),
        workspace_name: row.get(2)?,
        workspace_path: row.get(3)?,
        outcome: if outcome == "completed" {
            TaskOutcome::Completed
        } else {
            TaskOutcome::Abandoned
        },
        agent_claimed_complete: row.get::<_, i32>(5)? != 0,
        title: row.get(6)?,
        summary: row.get(7)?,
        lessons: serde_json::from_str(&lessons).unwrap_or_default(),
        terminated_at: row.get(9)?,
        generated_at: row.get(10)?,
    })
}

// ── Trigger ──────────────────────────────────────────────────────────────────

/// Kick off the retrospective for a task that just reached a terminal state.
///
/// Returns immediately — the caller is on the decision-card response path,
/// unblocking a waiting agent. Everything below happens on a detached thread and
/// every failure is logged, never propagated: a missing LLM provider must not
/// make ending a task fail.
pub fn on_task_terminated(session_id: &str, workspace_path: &str, outcome: TaskOutcome) {
    let session_id = session_id.to_string();
    let workspace_path = workspace_path.to_string();
    std::thread::spawn(move || {
        if let Err(e) = run_review(&session_id, &workspace_path, outcome) {
            log_debug(&format!("[task_review] {session_id}: {e}"));
        }
    });
}

/// The sessions that make up this task, in hop order. A handoff chain is one
/// task; a session that never handed off is a one-hop task.
pub fn task_sessions(session_id: &str) -> Vec<String> {
    match crate::handoff::chain_containing(session_id) {
        Some(chain) => {
            let ids = chain.session_ids();
            if ids.is_empty() {
                vec![session_id.to_string()]
            } else {
                ids
            }
        }
        None => vec![session_id.to_string()],
    }
}

fn run_review(
    session_id: &str,
    workspace_path: &str,
    outcome: TaskOutcome,
) -> Result<(), String> {
    let session_ids = task_sessions(session_id);
    let root = session_ids
        .first()
        .cloned()
        .unwrap_or_else(|| session_id.to_string());
    let agent_claimed_complete = crate::task_outcome::read(session_id)
        .map(|r| r.agent_claimed_complete)
        .unwrap_or(false);
    let workspace_name = crate::session::workspace_name(workspace_path);

    // Trace: conversation pairs across every hop, oldest hop first, then the
    // decision cards where the user overrode what the agent offered.
    let mut pairs: Vec<ConversationPair> = Vec::new();
    for sid in &session_ids {
        let Some(path) = crate::session::find_session_jsonl(sid) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        pairs.extend(extract_conversation_pairs(&content, sid, &workspace_name));
    }
    if pairs.len() > MAX_PAIRS {
        // Keep the tail: the end of a task is where its outcome was decided.
        pairs = pairs.split_off(pairs.len() - MAX_PAIRS);
    }
    let other_picks =
        crate::decision_history::collect_other_picks_for_sessions(&session_ids, MAX_DECISION_SIGNALS);

    if pairs.is_empty() && other_picks.is_empty() {
        return Err("no trace to review".into());
    }

    let prompt = build_review_prompt(
        &session_ids,
        &workspace_name,
        outcome,
        agent_claimed_complete,
        &pairs,
        &other_picks,
    );

    let config = crate::llm_provider::LlmConfig::load();
    let mut raw: Option<String> = None;
    for route in crate::llm_provider::daily_report_routes(&config) {
        raw = crate::llm_usage::complete_accounted(
            route.provider.as_ref(),
            &prompt,
            &route.model,
            REVIEW_TIMEOUT,
            SCENARIO_TASK_REVIEW,
        );
        if raw.is_some() {
            break;
        }
    }
    let raw = raw.ok_or("no LLM provider produced a review")?;

    let parsed = parse_review(&raw, &session_ids, &workspace_name);
    let review = TaskReview {
        root_session_id: root,
        session_ids,
        workspace_name,
        workspace_path: workspace_path.to_string(),
        outcome,
        agent_claimed_complete,
        title: parsed.title,
        summary: parsed.summary,
        lessons: parsed.lessons,
        terminated_at: crate::task_outcome::read(session_id)
            .map(|r| r.updated)
            .unwrap_or_else(now_ms),
        generated_at: now_ms(),
    };
    TaskReviewStore::open()?.save(&review)
}

/// Usage-accounting scenario tag for the per-task retrospective, so its spend
/// shows up separately from the daily report's in the usage panel.
pub const SCENARIO_TASK_REVIEW: &str = "task_review";

// ── Prompt + parsing ─────────────────────────────────────────────────────────

fn build_review_prompt(
    session_ids: &[String],
    workspace_name: &str,
    outcome: TaskOutcome,
    agent_claimed_complete: bool,
    pairs: &[ConversationPair],
    other_picks: &[crate::decision_history::OtherPickContext],
) -> String {
    let verdict = match outcome {
        TaskOutcome::Completed => {
            "The user pressed 结束任务 — they judged this task COMPLETE and successful."
        }
        TaskOutcome::Abandoned => {
            "The user pressed 放弃任务 — they GAVE UP on this task, unfinished."
        }
    };
    // The disagreement between what the agent claimed and what the user decided
    // is the single most informative fact available here, so it is stated
    // explicitly rather than left for the model to infer from the trace.
    let disagreement = match (outcome, agent_claimed_complete) {
        (TaskOutcome::Abandoned, true) => {
            "IMPORTANT — the agent had flagged this card `taskComplete: true`, i.e. it \
             believed the work was finished, and the user abandoned it anyway. Something \
             the agent thought it had delivered was not actually delivered, or not what \
             was asked for. Finding that gap is the primary job of this review.\n\n"
        }
        (TaskOutcome::Completed, false) => {
            "Note — the agent did NOT flag the task complete, yet the user ended it as \
             done. Either the agent under-reported its own progress, or the user was \
             satisfied with less than the agent thought was required.\n\n"
        }
        _ => "",
    };

    let mut trace = String::new();
    for (i, p) in pairs.iter().enumerate() {
        let a: String = p.assistant_text().chars().take(1200).collect();
        let u: String = p.user_text().chars().take(1200).collect();
        trace.push_str(&format!("--- Turn {} ---\nAGENT: {a}\nUSER: {u}\n\n", i + 1));
    }

    let mut signals = String::new();
    for (i, ctx) in other_picks.iter().enumerate() {
        let q: String = ctx.question.chars().take(400).collect();
        signals.push_str(&format!("--- Card {} [{}] ---\n  AI raised: {q}\n", i + 1, ctx.card_type));
        if ctx.user_choice.trim().is_empty() {
            signals.push_str("  User REJECTED the AI's proposal.\n\n");
        } else {
            let c: String = ctx.user_choice.chars().take(300).collect();
            signals.push_str(&format!("  User answered \"Other\" instead: {c}\n\n"));
        }
    }

    format!(
        "You are reviewing ONE finished task end to end, to learn from how it went.\n\n\
         Workspace: {workspace_name}\n\
         Sessions in this task (a handoff chain is one task): {n} hop(s)\n\
         OUTCOME: {verdict}\n\n\
         {disagreement}\
         Below is the task's trace: the agent/user turns across every hop, then the \
         decision cards where the user overrode what the agent offered.\n\n\
         <trace>\n{trace}</trace>\n\n\
         <decision_signals>\n{signals}</decision_signals>\n\n\
         Answer in EXACTLY this format, nothing else:\n\n\
         TITLE: <one line naming what this task actually was>\n\
         SUMMARY: <2-5 sentences: what happened, and specifically WHY it ended the way \
         it did. For an abandoned task, name the concrete thing that blocked it or the \
         concrete way the agent went wrong. For a completed one, name what actually \
         made it work — skip generic praise.>\n\
         LESSON: <a general, transferable rule the agent should follow next time>\n\
         REASON: <what went wrong here and what it cost — the evidence for that rule>\n\
         (repeat LESSON/REASON for each qualifying lesson, at most 3)\n\n\
         CRITICAL FILTER for lessons — only emit one if ALL hold:\n\
         1. It generalises beyond this project (not \"rename foo to bar in this repo\").\n\
         2. It explains WHY, citing what actually went wrong in this trace.\n\
         3. It is not an obvious coding standard everyone already knows.\n\
         If the task went cleanly and there is nothing transferable to learn, emit no \
         LESSON lines at all — TITLE and SUMMARY alone are a valid answer. Do not invent \
         a lesson to fill the slot.\n",
        n = session_ids.len(),
    )
}

struct ParsedReview {
    title: String,
    summary: String,
    lessons: Vec<Lesson>,
}

/// Parse the fixed `TITLE:` / `SUMMARY:` / `LESSON:` + `REASON:` shape. Tolerant
/// by design: a missing section yields an empty string / no lessons rather than
/// discarding an otherwise usable review.
fn parse_review(raw: &str, session_ids: &[String], workspace_name: &str) -> ParsedReview {
    let session_id = session_ids.last().cloned().unwrap_or_default();
    let mut title = String::new();
    let mut summary = String::new();
    let mut lessons: Vec<Lesson> = Vec::new();
    let mut pending: Option<String> = None;

    for line in raw.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("TITLE:") {
            title = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("SUMMARY:") {
            summary = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("LESSON:") {
            // A LESSON with no REASON after it is dropped by the filter below,
            // so stash it and only commit on the matching REASON.
            pending = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("REASON:") {
            if let Some(content) = pending.take() {
                if !content.is_empty() {
                    lessons.push(Lesson {
                        content,
                        reason: rest.trim().to_string(),
                        workspace_name: workspace_name.to_string(),
                        session_id: session_id.clone(),
                    });
                }
            }
        } else if !line.is_empty() && !summary.is_empty() && title.is_empty() {
            // Defensive: a model that leads with prose before TITLE.
            continue;
        }
    }
    ParsedReview {
        title,
        summary,
        lessons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdb(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-task-review-{tag}-{}-{}.db",
            std::process::id(),
            now_ms()
        ))
    }

    fn sample(root: &str, outcome: TaskOutcome, terminated_at: u64) -> TaskReview {
        TaskReview {
            root_session_id: root.into(),
            session_ids: vec![root.into(), format!("{root}-hop2")],
            workspace_name: "repo".into(),
            workspace_path: "/ws/repo".into(),
            outcome,
            agent_claimed_complete: true,
            title: "Ship the thing".into(),
            summary: "It went like this.".into(),
            lessons: vec![Lesson {
                content: "Verify before claiming done".into(),
                reason: "The agent said done and the user abandoned it".into(),
                workspace_name: "repo".into(),
                session_id: root.into(),
            }],
            terminated_at,
            generated_at: terminated_at + 5,
        }
    }

    #[test]
    fn save_and_range_roundtrip() {
        let path = tmpdb("crud");
        let store = TaskReviewStore::open_at(&path).unwrap();
        store.save(&sample("s1", TaskOutcome::Abandoned, 1_000)).unwrap();
        store.save(&sample("s2", TaskOutcome::Completed, 2_000)).unwrap();

        let all = store.list_in_range(0, 10_000).unwrap();
        assert_eq!(all.len(), 2);
        // Newest first.
        assert_eq!(all[0].root_session_id, "s2");
        assert_eq!(all[0].outcome, TaskOutcome::Completed);
        assert_eq!(all[1].outcome, TaskOutcome::Abandoned);
        assert_eq!(all[1].lessons.len(), 1);
        assert_eq!(all[1].session_ids.len(), 2);

        // Range is half-open [from, to).
        let window = store.list_in_range(2_000, 3_000).unwrap();
        assert_eq!(window.len(), 1);
        assert_eq!(window[0].root_session_id, "s2");

        // Re-terminating overwrites rather than duplicating.
        store.save(&sample("s1", TaskOutcome::Completed, 1_000)).unwrap();
        assert_eq!(store.list_in_range(0, 10_000).unwrap().len(), 2);
        assert_eq!(store.get("s1").unwrap().outcome, TaskOutcome::Completed);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parses_title_summary_and_lesson_pairs() {
        let raw = "TITLE: Wire the relay\n\
                   SUMMARY: The agent shipped half of it and stopped.\n\
                   LESSON: Enumerate every surface before claiming done\n\
                   REASON: Only the desktop path was changed; the phone kept the old button.\n\
                   LESSON: Second rule\n\
                   REASON: Second reason\n";
        let p = parse_review(raw, &["a".into(), "b".into()], "repo");
        assert_eq!(p.title, "Wire the relay");
        assert!(p.summary.starts_with("The agent shipped"));
        assert_eq!(p.lessons.len(), 2);
        assert_eq!(p.lessons[0].content, "Enumerate every surface before claiming done");
        // Lessons attach to the last hop — the session the user was looking at.
        assert_eq!(p.lessons[0].session_id, "b");
    }

    #[test]
    fn lesson_without_reason_is_dropped() {
        // A bare LESSON line carries no evidence, which is exactly what the
        // prompt's filter forbids; committing it would smuggle an unjustified
        // rule into the user's CLAUDE.md.
        let raw = "TITLE: T\nSUMMARY: S\nLESSON: unbacked claim\n";
        let p = parse_review(raw, &["a".into()], "repo");
        assert!(p.lessons.is_empty());
        assert_eq!(p.title, "T");
    }

    #[test]
    fn prompt_names_the_agent_user_disagreement() {
        let prompt = build_review_prompt(
            &["s1".into()],
            "repo",
            TaskOutcome::Abandoned,
            true,
            &[],
            &[],
        );
        assert!(
            prompt.contains("`taskComplete: true`"),
            "an abandoned-but-claimed-complete task must state the disagreement"
        );
        assert!(prompt.contains("GAVE UP"));

        let clean = build_review_prompt(
            &["s1".into()],
            "repo",
            TaskOutcome::Completed,
            true,
            &[],
            &[],
        );
        assert!(!clean.contains("`taskComplete: true`"));
    }
}
