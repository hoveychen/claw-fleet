//! Daily report generation: types, SQLite storage, metrics extraction, and lessons.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::llm_provider::LlmProvider;
use crate::log_debug;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DailyReport {
    pub date: String,
    pub timezone: String,
    pub generated_at: u64,
    pub metrics: DailyMetrics,
    /// Legacy: the per-day AI summary is no longer generated (it only
    /// regrouped session titles). Kept so stored reports still deserialize.
    pub ai_summary: Option<String>,
    pub ai_summary_generated_at: Option<u64>,
    pub session_ids: Vec<String>,
    pub lessons: Option<Vec<Lesson>>,
    pub lessons_generated_at: Option<u64>,
}

/// Bump when the token-accounting methodology (or any metrics-fold logic) changes, so
/// [`run_backfill_check`] knows a cached past-day report was computed under an
/// older basis and must be re-scanned. History:
///   0 — implicit for reports predating this field (last-turn input snapshot).
///   1 — cumulative input incl. cache (input + cache_creation + cache_read),
///       matching cost and the sidebar counter's methodology.
///   2 — usage attributed by finalized turn timestamp, including sessions that
///       crossed midnight and live Claude/Codex sources.
///   3 — only sessions Fleet launched (the launch registry) are counted.
pub const CURRENT_METRICS_VERSION: u32 = 3;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DailyMetrics {
    /// Accounting methodology version these metrics were computed under. Missing (⇒ 0) in reports
    /// generated before the field existed. See [`CURRENT_METRICS_VERSION`].
    #[serde(default)]
    pub metrics_version: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_cache_creation_tokens: u64,
    #[serde(default)]
    pub total_cache_read_tokens: u64,
    #[serde(default)]
    pub total_web_search_requests: u64,
    #[serde(default)]
    pub total_cost_usd: f64,
    pub total_sessions: u32,
    pub total_subagents: u32,
    pub total_tool_calls: u32,
    pub tool_call_breakdown: HashMap<String, u32>,
    pub model_breakdown: HashMap<String, ModelTokens>,
    pub projects: Vec<ProjectMetrics>,
    pub source_breakdown: HashMap<String, u32>,
    pub hourly_activity: [u32; 24],
    /// Per-type decision-card analytics for the day (elicitation / fleet-ask /
    /// plan-approval). Defaults to empty for reports generated before this
    /// field existed.
    #[serde(default)]
    pub decision_cards: crate::decision_history::DecisionCardStats,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    /// **Total** tokens sent to the API: `Σ(input + cache_write + cache_read)`,
    /// on the same accounting methodology as `cost_usd` — NOT net input. Consumers that itemise
    /// input separately from the cache rows must subtract the two cache figures
    /// (see `today_usage::fold_report_days`).
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    /// The 1-hour-TTL subset of `cache_creation_tokens` (billed at 2× input, vs
    /// 1.25× for 5-minute writes). Absent (0) in reports written before TTL-aware
    /// pricing landed — those days price every write at the 5-minute rate.
    #[serde(default)]
    pub cache_creation_1h_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProjectMetrics {
    pub workspace_path: String,
    pub workspace_name: String,
    pub session_count: u32,
    pub subagent_count: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_cache_creation_tokens: u64,
    #[serde(default)]
    pub total_cache_read_tokens: u64,
    #[serde(default)]
    pub total_web_search_requests: u64,
    #[serde(default)]
    pub total_cost_usd: f64,
    pub tool_calls: u32,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(rename = "ReportSessionSummary"))]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub last_message: Option<String>,
    pub model: Option<String>,
    pub is_subagent: bool,
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
    pub agent_source: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DailyReportStats {
    pub date: String,
    pub total_tokens: u64,
    pub total_sessions: u32,
    pub total_tool_calls: u32,
    pub total_projects: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Lesson {
    /// The lesson content (actionable instruction).
    pub content: String,
    /// Why this lesson was identified (brief explanation).
    pub reason: String,
    /// Workspace where the mistake occurred.
    pub workspace_name: String,
    /// Session ID where the mistake occurred.
    pub session_id: String,
    /// Every session the pattern was seen in. A lesson shown to the user cites
    /// at least two (see [`gate_lessons`]); empty on lessons stored before the
    /// recurrence gate existed and on per-task review lessons.
    #[serde(default)]
    pub evidence_session_ids: Vec<String>,
}

/// An adopted lesson (in `~/.claude/fleet-lessons.md`) that an agent still
/// acted against: evidence the rule is not doing its job.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LessonViolation {
    /// Id of the adopted lesson (`ManagedLesson::id`).
    pub lesson_id: String,
    pub lesson_content: String,
    /// Sessions on that day that violated it.
    pub session_ids: Vec<String>,
    /// What the agent did, in one sentence.
    pub note: String,
}

/// What one day's lessons pass produced, split by how much evidence backs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LessonsOutcome {
    /// Patterns seen in two or more sessions, at least one of them that day.
    /// The only lessons shown to the user.
    pub lessons: Vec<Lesson>,
    /// Single-session candidates. Not shown; kept so later days can match
    /// against them and promote a pattern once it recurs.
    pub candidates: Vec<Lesson>,
    pub violations: Vec<LessonViolation>,
}

/// A user text turn paired with the immediately preceding assistant turn.
pub struct ConversationPair {
    assistant_text: String,
    user_text: String,
    session_id: String,
    workspace_name: String,
}

impl ConversationPair {
    /// The assistant turn immediately preceding the user's reply. Read by
    /// `task_review`, which renders the same trace for a single task.
    pub fn assistant_text(&self) -> &str {
        &self.assistant_text
    }
    /// The user turn that followed it.
    pub fn user_text(&self) -> &str {
        &self.user_text
    }
}

// ── Raw metrics from a single session's JSONL ────────────────────────────────

pub struct SessionMetricsRaw {
    /// Cumulative input tokens across all unique assistant turns
    /// (`Σ input + cache_creation + cache_read`, cache re-reads included) — the
    /// "tokens sent to the API" total, on the same accounting methodology as `cost_usd` and as the
    /// live scan's `SessionInfo.total_input_tokens`. NOT the last-turn
    /// context-window snapshot.
    pub input_tokens: u64,
    /// Summed output tokens across all unique assistant turns.
    pub output_tokens: u64,
    /// Summed cache-creation tokens, both TTLs (for billing).
    pub cache_creation_tokens: u64,
    /// The 1-hour-TTL subset of `cache_creation_tokens`, billed at 2× input
    /// instead of 1.25×. Stored per model in the report so a later receipt can
    /// itemise the two write rates separately.
    pub cache_creation_1h_tokens: u64,
    /// Summed cache-read tokens (for billing).
    pub cache_read_tokens: u64,
    /// Summed web-search requests (for billing).
    pub web_search_requests: u64,
    /// Summed USD cost across all turns, computed per-turn with the model
    /// reported on each turn (matches Claude Code's own `total_cost_usd`).
    pub cost_usd: f64,
    pub tool_calls: HashMap<String, u32>,
    pub model: Option<String>,
}

// ── ReportStore ──────────────────────────────────────────────────────────────

pub struct ReportStore {
    conn: Connection,
}

impl ReportStore {
    /// Open (or create) the report database at `~/.fleet/fleet-reports.db`.
    pub fn open() -> Result<Self, String> {
        let db_path = crate::session::real_home_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?
            .join(".fleet")
            .join("fleet-reports.db");
        Self::open_at(&db_path)
    }

    /// Open (or create) the report database at a custom path.
    pub fn open_at(db_path: &Path) -> Result<Self, String> {
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
            "CREATE TABLE IF NOT EXISTS daily_reports (
                 date         TEXT PRIMARY KEY,
                 timezone     TEXT NOT NULL,
                 generated_at INTEGER NOT NULL,
                 metrics      TEXT NOT NULL,
                 ai_summary   TEXT,
                 ai_summary_generated_at INTEGER,
                 session_ids  TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS daily_stats (
                 date             TEXT PRIMARY KEY,
                 total_tokens     INTEGER NOT NULL,
                 total_sessions   INTEGER NOT NULL,
                 total_tool_calls INTEGER NOT NULL,
                 total_projects   INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;

        // Migrations: add lessons columns if they don't exist yet. Each ALTER
        // is its own statement, NOT one `execute_batch`: SQLite aborts a batch
        // at the first statement's error, so a db that already has `lessons`
        // (older / partially-migrated schema) would fail the first ALTER as
        // "duplicate column" and never run the second — leaving
        // `lessons_generated_at` missing and every save dying on it. Ignoring
        // each error independently makes the migration idempotent per column.
        let _ = conn.execute("ALTER TABLE daily_reports ADD COLUMN lessons TEXT;", []);
        let _ = conn.execute(
            "ALTER TABLE daily_reports ADD COLUMN lessons_generated_at INTEGER;",
            [],
        );

        // Lesson evidence that is not shown on the report itself: single-session
        // candidates (the recurrence pool) and adopted-lesson violations.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS lesson_pool (
                 date         TEXT PRIMARY KEY,
                 candidates   TEXT NOT NULL,
                 violations   TEXT NOT NULL,
                 generated_at INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;

        Ok(Self { conn })
    }

    /// Save (INSERT OR REPLACE) a report into both tables.
    pub fn save_report(&self, report: &DailyReport) -> Result<(), String> {
        let metrics_json =
            serde_json::to_string(&report.metrics).map_err(|e| format!("json encode: {e}"))?;
        let session_ids_json =
            serde_json::to_string(&report.session_ids).map_err(|e| format!("json encode: {e}"))?;
        let lessons_json = match &report.lessons {
            Some(l) => Some(serde_json::to_string(l).map_err(|e| format!("json encode: {e}"))?),
            None => None,
        };

        self.conn
            .execute(
                "INSERT OR REPLACE INTO daily_reports
                 (date, timezone, generated_at, metrics, ai_summary, ai_summary_generated_at, session_ids, lessons, lessons_generated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    report.date,
                    report.timezone,
                    report.generated_at,
                    metrics_json,
                    report.ai_summary,
                    report.ai_summary_generated_at,
                    session_ids_json,
                    lessons_json,
                    report.lessons_generated_at,
                ],
            )
            .map_err(|e| format!("insert report: {e}"))?;

        let total_tokens = report.metrics.total_input_tokens + report.metrics.total_output_tokens;

        self.conn
            .execute(
                "INSERT OR REPLACE INTO daily_stats
                 (date, total_tokens, total_sessions, total_tool_calls, total_projects)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    report.date,
                    total_tokens,
                    report.metrics.total_sessions,
                    report.metrics.total_tool_calls,
                    report.metrics.projects.len() as u32,
                ],
            )
            .map_err(|e| format!("insert stats: {e}"))?;

        Ok(())
    }

    /// Retrieve a report by date.
    pub fn get_report(&self, date: &str) -> Result<Option<DailyReport>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT date, timezone, generated_at, metrics, ai_summary,
                        ai_summary_generated_at, session_ids, lessons, lessons_generated_at
                 FROM daily_reports WHERE date = ?1",
            )
            .map_err(|e| format!("prepare: {e}"))?;

        let result = stmt
            .query_row(params![date], |row| {
                let date: String = row.get(0)?;
                let timezone: String = row.get(1)?;
                let generated_at: u64 = row.get(2)?;
                let metrics_json: String = row.get(3)?;
                let ai_summary: Option<String> = row.get(4)?;
                let ai_summary_generated_at: Option<u64> = row.get(5)?;
                let session_ids_json: String = row.get(6)?;
                let lessons_json: Option<String> = row.get(7)?;
                let lessons_generated_at: Option<u64> = row.get(8)?;
                Ok((
                    date,
                    timezone,
                    generated_at,
                    metrics_json,
                    ai_summary,
                    ai_summary_generated_at,
                    session_ids_json,
                    lessons_json,
                    lessons_generated_at,
                ))
            })
            .ok();

        match result {
            None => Ok(None),
            Some((
                date,
                timezone,
                generated_at,
                metrics_json,
                ai_summary,
                ai_summary_generated_at,
                session_ids_json,
                lessons_json,
                lessons_generated_at,
            )) => {
                let metrics: DailyMetrics = serde_json::from_str(&metrics_json)
                    .map_err(|e| format!("json decode metrics: {e}"))?;
                let session_ids: Vec<String> = serde_json::from_str(&session_ids_json)
                    .map_err(|e| format!("json decode session_ids: {e}"))?;
                let lessons: Option<Vec<Lesson>> = match lessons_json {
                    Some(j) => Some(
                        serde_json::from_str(&j)
                            .map_err(|e| format!("json decode lessons: {e}"))?,
                    ),
                    None => None,
                };
                Ok(Some(DailyReport {
                    date,
                    timezone,
                    generated_at,
                    metrics,
                    ai_summary,
                    ai_summary_generated_at,
                    session_ids,
                    lessons,
                    lessons_generated_at,
                }))
            }
        }
    }

    /// List stats for dates in range [from, to] inclusive.
    pub fn list_stats(&self, from: &str, to: &str) -> Result<Vec<DailyReportStats>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT date, total_tokens, total_sessions, total_tool_calls, total_projects
                 FROM daily_stats
                 WHERE date BETWEEN ?1 AND ?2
                 ORDER BY date",
            )
            .map_err(|e| format!("prepare: {e}"))?;

        let rows = stmt
            .query_map(params![from, to], |row| {
                Ok(DailyReportStats {
                    date: row.get(0)?,
                    total_tokens: row.get(1)?,
                    total_sessions: row.get(2)?,
                    total_tool_calls: row.get(3)?,
                    total_projects: row.get(4)?,
                })
            })
            .map_err(|e| format!("query: {e}"))?;

        let mut stats = Vec::new();
        for row in rows {
            stats.push(row.map_err(|e| format!("row: {e}"))?);
        }
        Ok(stats)
    }

    /// Update the lessons list for an existing report.
    pub fn update_lessons(&self, date: &str, lessons: &[Lesson]) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let lessons_json =
            serde_json::to_string(lessons).map_err(|e| format!("json encode lessons: {e}"))?;

        self.conn
            .execute(
                "UPDATE daily_reports SET lessons = ?1, lessons_generated_at = ?2 WHERE date = ?3",
                params![lessons_json, now_ms, date],
            )
            .map_err(|e| format!("update lessons: {e}"))?;
        Ok(())
    }

    /// Persist a lessons pass: the gated lessons onto the report, the rest into
    /// `lesson_pool`.
    pub fn save_lessons_outcome(&self, date: &str, outcome: &LessonsOutcome) -> Result<(), String> {
        self.update_lessons(date, &outcome.lessons)?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let candidates = serde_json::to_string(&outcome.candidates)
            .map_err(|e| format!("json encode candidates: {e}"))?;
        let violations = serde_json::to_string(&outcome.violations)
            .map_err(|e| format!("json encode violations: {e}"))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO lesson_pool (date, candidates, violations, generated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![date, candidates, violations, now_ms],
            )
            .map_err(|e| format!("save lesson pool: {e}"))?;
        Ok(())
    }

    /// A day's single-session candidates and adopted-lesson violations, or
    /// `None` when that day's pass has not run under the recurrence gate.
    pub fn get_lesson_pool(
        &self,
        date: &str,
    ) -> Result<Option<(Vec<Lesson>, Vec<LessonViolation>)>, String> {
        let row = self
            .conn
            .query_row(
                "SELECT candidates, violations FROM lesson_pool WHERE date = ?1",
                params![date],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("query lesson pool: {e}"))?;
        Ok(row.map(|(c, v)| {
            (
                serde_json::from_str(&c).unwrap_or_default(),
                serde_json::from_str(&v).unwrap_or_default(),
            )
        }))
    }

    /// List all dates that have reports, ordered ascending.
    pub fn list_dates(&self) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT date FROM daily_reports ORDER BY date")
            .map_err(|e| format!("prepare: {e}"))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("query: {e}"))?;

        let mut dates = Vec::new();
        for row in rows {
            dates.push(row.map_err(|e| format!("row: {e}"))?);
        }
        Ok(dates)
    }
}

// ── Metrics extraction ───────────────────────────────────────────────────────

/// Extract metrics for a single session from its JSONL content.
pub fn extract_session_metrics(jsonl_content: &str) -> SessionMetricsRaw {
    use crate::model_cost::{turn_cost_usd, TurnUsage};

    let mut total_output: u64 = 0;
    let mut sum_input: u64 = 0;
    let mut sum_cache_create: u64 = 0;
    let mut sum_cache_create_1h: u64 = 0;
    let mut sum_cache_read: u64 = 0;
    let mut sum_web_search: u64 = 0;
    let mut sum_cost: f64 = 0.0;
    let mut tool_calls: HashMap<String, u32> = HashMap::new();
    let mut model: Option<String> = None;
    let mut seen_msg_ids: HashSet<String> = HashSet::new();

    for line in jsonl_content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };

        // Dedup by message id
        let msg_id = msg
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string();
        if !msg_id.is_empty() {
            if seen_msg_ids.contains(&msg_id) {
                continue;
            }
            seen_msg_ids.insert(msg_id);
        }

        let usage = msg.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_create = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let web_search = usage
            .and_then(|u| u.get("server_tool_use"))
            .and_then(|s| s.get("web_search_requests"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        let cache_create_1h = crate::model_cost::parse_cache_creation_1h(usage);

        total_output += output_tokens;
        sum_cache_create += cache_create;
        sum_cache_create_1h += cache_create_1h;
        sum_cache_read += cache_read;
        sum_web_search += web_search;

        // Cumulative input across turns (cache re-reads included), matching
        // `cost_usd` and the live scan's `SessionInfo.total_input_tokens` — not
        // the last-turn context-window snapshot.
        sum_input += input + cache_create + cache_read;

        // Per-turn cost uses this turn's own model (falls back to the
        // most recently seen model if this line omits it).
        // A `<synthetic>` / `unknown` turn is a CC-injected control message, not
        // a model — adopting it would book the whole session's spend under a
        // placeholder (see `session::is_real_model_id`).
        let turn_model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| crate::session::is_real_model_id(m));
        if let Some(m) = turn_model {
            model = Some(m.to_string());
        }
        let cost_model = turn_model.or(model.as_deref()).unwrap_or("");
        sum_cost += turn_cost_usd(
            cost_model,
            &TurnUsage {
                input_tokens: input,
                output_tokens,
                cache_creation_tokens: cache_create,
                cache_creation_1h_tokens: cache_create_1h,
                cache_read_tokens: cache_read,
                web_search_requests: web_search,
            },
        );

        // Tool calls: count tool_use blocks in content
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                        *tool_calls.entry(name.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    SessionMetricsRaw {
        input_tokens: sum_input,
        output_tokens: total_output,
        cache_creation_tokens: sum_cache_create,
        cache_creation_1h_tokens: sum_cache_create_1h,
        cache_read_tokens: sum_cache_read,
        web_search_requests: sum_web_search,
        cost_usd: sum_cost,
        tool_calls,
        model,
    }
}

/// Extract the non-token activity that belongs to one local calendar day.
/// Token and cost fields come from `today_usage::sessions_usage_for_date`; this
/// companion fold keeps report-only tool/search counters on the same boundary.
fn extract_session_activity_for_date(
    jsonl_content: &str,
    date: &str,
) -> (HashMap<String, u32>, u64) {
    let mut tool_calls = HashMap::new();
    let mut web_search_requests = 0u64;
    let mut seen_msg_ids = HashSet::new();

    for line in jsonl_content.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = v.get("message").and_then(Value::as_object) else {
            continue;
        };
        if message.get("stop_reason").map_or(true, Value::is_null) {
            continue;
        }
        let Some(turn_date) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
        else {
            continue;
        };
        if turn_date != date {
            continue;
        }
        let msg_id = message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !msg_id.is_empty() && !seen_msg_ids.insert(msg_id.to_string()) {
            continue;
        }

        web_search_requests = web_search_requests.saturating_add(
            message
                .get("usage")
                .and_then(|u| u.get("server_tool_use"))
                .and_then(|u| u.get("web_search_requests"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        if let Some(content) = message.get("content").and_then(Value::as_array) {
            for block in content {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    *tool_calls.entry(name.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    (tool_calls, web_search_requests)
}

// ── Report generation ────────────────────────────────────────────────────────

/// Generate a daily report from a list of SessionInfo and their JSONL paths.
pub fn generate_report_from_sessions(
    date: &str,
    timezone: &str,
    sessions: &[&crate::session::SessionInfo],
) -> DailyReport {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Per-session extracted metrics, keyed by session index
    struct SessionData {
        metrics: SessionMetricsRaw,
        model_lines: Vec<crate::today_usage::ModelReceiptLine>,
        info: usize, // index into sessions
    }

    let mut session_data: Vec<SessionData> = Vec::new();
    let daily_usage = crate::today_usage::sessions_usage_for_date(sessions, date);
    for ((i, si), model_lines) in sessions.iter().enumerate().zip(daily_usage) {
        let jsonl_content = if si.agent_source == "claude-code" {
            std::fs::read_to_string(&si.jsonl_path).unwrap_or_default()
        } else {
            String::new()
        };
        let (tool_calls, web_search_requests) =
            extract_session_activity_for_date(&jsonl_content, date);

        let mut metrics = SessionMetricsRaw {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests,
            cost_usd: 0.0,
            tool_calls,
            model: None,
        };
        for line in &model_lines {
            let cache_creation = line
                .cache_creation_tokens
                .saturating_add(line.cache_creation_1h_tokens);
            metrics.input_tokens = metrics
                .input_tokens
                .saturating_add(line.input_tokens)
                .saturating_add(cache_creation)
                .saturating_add(line.cache_read_tokens);
            metrics.output_tokens = metrics.output_tokens.saturating_add(line.output_tokens);
            metrics.cache_creation_tokens =
                metrics.cache_creation_tokens.saturating_add(cache_creation);
            metrics.cache_creation_1h_tokens = metrics
                .cache_creation_1h_tokens
                .saturating_add(line.cache_creation_1h_tokens);
            metrics.cache_read_tokens = metrics
                .cache_read_tokens
                .saturating_add(line.cache_read_tokens);
            metrics.cost_usd += line.cost_usd;
        }
        metrics.model = model_lines
            .iter()
            .max_by(|a, b| a.cost_usd.total_cmp(&b.cost_usd))
            .map(|line| line.model.clone());

        if model_lines.is_empty() && metrics.tool_calls.is_empty() {
            continue;
        }
        session_data.push(SessionData {
            metrics,
            model_lines,
            info: i,
        });
    }

    // Group by workspace_path
    let mut project_map: HashMap<String, Vec<usize>> = HashMap::new(); // workspace_path -> indices into session_data
    for (idx, sd) in session_data.iter().enumerate() {
        let si = sessions[sd.info];
        project_map
            .entry(si.workspace_path.clone())
            .or_default()
            .push(idx);
    }

    // Build ProjectMetrics
    let mut projects: Vec<ProjectMetrics> = Vec::new();
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut total_cache_creation_tokens: u64 = 0;
    let mut total_cache_read_tokens: u64 = 0;
    let mut total_web_search_requests: u64 = 0;
    let mut total_cost_usd: f64 = 0.0;
    let mut total_tool_calls: u32 = 0;
    let mut total_subagents: u32 = 0;
    let mut tool_call_breakdown: HashMap<String, u32> = HashMap::new();
    let mut model_breakdown: HashMap<String, ModelTokens> = HashMap::new();
    let mut source_breakdown: HashMap<String, u32> = HashMap::new();
    let mut hourly_activity: [u32; 24] = [0; 24];

    for (workspace_path, indices) in &project_map {
        let mut proj = ProjectMetrics {
            workspace_path: workspace_path.clone(),
            workspace_name: String::new(),
            session_count: 0,
            subagent_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            total_web_search_requests: 0,
            total_cost_usd: 0.0,
            tool_calls: 0,
            sessions: Vec::new(),
        };

        for &idx in indices {
            let sd = &session_data[idx];
            let si = sessions[sd.info];

            if proj.workspace_name.is_empty() {
                proj.workspace_name = si.workspace_name.clone();
            }

            proj.session_count += 1;
            if si.is_subagent {
                proj.subagent_count += 1;
                total_subagents += 1;
            }

            proj.total_input_tokens += sd.metrics.input_tokens;
            proj.total_output_tokens += sd.metrics.output_tokens;
            proj.total_cache_creation_tokens += sd.metrics.cache_creation_tokens;
            proj.total_cache_read_tokens += sd.metrics.cache_read_tokens;
            proj.total_web_search_requests += sd.metrics.web_search_requests;
            proj.total_cost_usd += sd.metrics.cost_usd;

            let session_tool_total: u32 = sd.metrics.tool_calls.values().sum();
            proj.tool_calls += session_tool_total;

            // Use model from extracted metrics, fall back to SessionInfo.model
            let effective_model = sd
                .metrics
                .model
                .as_deref()
                .or(si.model.as_deref())
                .unwrap_or("unknown")
                .to_string();

            proj.sessions.push(SessionSummary {
                id: si.id.clone(),
                title: si.ai_title.clone().or_else(|| si.slug.clone()),
                last_message: si.last_message_preview.clone(),
                model: Some(effective_model.clone()),
                is_subagent: si.is_subagent,
                output_tokens: sd.metrics.output_tokens,
                cost_usd: sd.metrics.cost_usd,
                agent_source: si.agent_source.clone(),
            });

            // Aggregate into totals
            total_input_tokens += sd.metrics.input_tokens;
            total_output_tokens += sd.metrics.output_tokens;
            total_cache_creation_tokens += sd.metrics.cache_creation_tokens;
            total_cache_read_tokens += sd.metrics.cache_read_tokens;
            total_web_search_requests += sd.metrics.web_search_requests;
            total_cost_usd += sd.metrics.cost_usd;
            total_tool_calls += session_tool_total;

            for (tool, count) in &sd.metrics.tool_calls {
                *tool_call_breakdown.entry(tool.clone()).or_insert(0) += count;
            }

            for line in &sd.model_lines {
                let cache_creation = line
                    .cache_creation_tokens
                    .saturating_add(line.cache_creation_1h_tokens);
                let entry = model_breakdown
                    .entry(line.model.clone())
                    .or_insert(ModelTokens {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_tokens: 0,
                        cache_creation_1h_tokens: 0,
                        cache_read_tokens: 0,
                        cost_usd: 0.0,
                    });
                entry.input_tokens = entry
                    .input_tokens
                    .saturating_add(line.input_tokens)
                    .saturating_add(cache_creation)
                    .saturating_add(line.cache_read_tokens);
                entry.output_tokens = entry.output_tokens.saturating_add(line.output_tokens);
                entry.cache_creation_tokens =
                    entry.cache_creation_tokens.saturating_add(cache_creation);
                entry.cache_creation_1h_tokens = entry
                    .cache_creation_1h_tokens
                    .saturating_add(line.cache_creation_1h_tokens);
                entry.cache_read_tokens = entry
                    .cache_read_tokens
                    .saturating_add(line.cache_read_tokens);
                entry.cost_usd += line.cost_usd;
            }

            *source_breakdown.entry(si.agent_source.clone()).or_insert(0) += 1;

            // Hourly activity from created_at_ms
            if si.created_at_ms > 0 {
                let secs = (si.created_at_ms / 1000) as i64;
                if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                    let local = dt.with_timezone(&chrono::Local);
                    let hour = local.format("%H").to_string().parse::<usize>().unwrap_or(0);
                    if hour < 24 {
                        hourly_activity[hour] += 1;
                    }
                }
            }
        }

        projects.push(proj);
    }

    // Sort projects by session count descending
    projects.sort_by(|a, b| b.session_count.cmp(&a.session_count));

    let session_ids: Vec<String> = session_data
        .iter()
        .map(|sd| sessions[sd.info].id.clone())
        .collect();

    DailyReport {
        date: date.to_string(),
        timezone: timezone.to_string(),
        generated_at: now_ms,
        metrics: DailyMetrics {
            metrics_version: CURRENT_METRICS_VERSION,
            total_input_tokens,
            total_output_tokens,
            total_cache_creation_tokens,
            total_cache_read_tokens,
            total_web_search_requests,
            total_cost_usd,
            total_sessions: session_data.len() as u32,
            total_subagents,
            total_tool_calls,
            tool_call_breakdown,
            model_breakdown,
            projects,
            source_breakdown,
            hourly_activity,
            decision_cards: crate::decision_history::compute_stats_for_date(date),
        },
        ai_summary: None,
        ai_summary_generated_at: None,
        session_ids,
        lessons: None,
        lessons_generated_at: None,
    }
}

// ── Lessons extraction ───────────────────────────────────────────────────────

const LESSONS_TIMEOUT: Duration = Duration::from_secs(180);

/// Extract conversation pairs (preceding assistant text + user text) from a JSONL session.
/// Only processes main-agent sessions with at least 2 user text turns.
pub fn extract_conversation_pairs(
    jsonl_content: &str,
    session_id: &str,
    workspace_name: &str,
) -> Vec<ConversationPair> {
    let mut pairs = Vec::new();
    let mut last_assistant_text: Option<String> = None;

    for line in jsonl_content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };

        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                // Collect text blocks from the assistant message
                let text: String = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    last_assistant_text = Some(text);
                }
            }
            Some("user") => {
                // Collect only text blocks (skip tool_result blocks)
                let user_text: String = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();

                let user_text = user_text.trim().to_string();
                if !user_text.is_empty() {
                    if let Some(assistant_text) = last_assistant_text.take() {
                        pairs.push(ConversationPair {
                            assistant_text,
                            user_text,
                            session_id: session_id.to_string(),
                            workspace_name: workspace_name.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    pairs
}

/// Collect existing rules from global `~/.claude/CLAUDE.md` and per-workspace
/// `CLAUDE.md` files so the lesson generator can avoid producing duplicates.
fn collect_existing_rules(workspace_paths: &[String]) -> String {
    let mut sections = Vec::new();
    let truncate = |s: &str| -> String { s.chars().take(2000).collect() };

    // 1. Global ~/.claude/CLAUDE.md
    if let Some(claude_dir) = crate::session::get_claude_dir() {
        let global = claude_dir.join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&global) {
            if !content.trim().is_empty() {
                sections.push(format!("[~/.claude/CLAUDE.md]\n{}", truncate(&content)));
            }
        }
    }

    // 2. Per-workspace CLAUDE.md
    let mut seen = HashSet::new();
    for wp in workspace_paths {
        if !seen.insert(wp.clone()) {
            continue;
        }
        // Skip TCC-protected workspaces (e.g. ~/Downloads) to avoid macOS permission dialogs.
        if crate::tcc::is_tcc_protected(std::path::Path::new(wp)) {
            continue;
        }
        let p = std::path::Path::new(wp).join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&p) {
            if !content.trim().is_empty() {
                sections.push(format!("[{}/CLAUDE.md]\n{}", wp, truncate(&content)));
            }
        }
    }

    sections.join("\n\n")
}

/// Render the day's "Other"-answered / rejected decision cards as a prompt
/// section. Returns an empty string when there are none.
fn build_decision_signals_section(
    other_picks: &[crate::decision_history::OtherPickContext],
) -> String {
    if other_picks.is_empty() {
        return String::new();
    }
    let mut body = String::new();
    for (i, ctx) in other_picks.iter().enumerate() {
        body.push_str(&format!(
            "--- Card {} [{}] (workspace: {}, session: {}) ---\n",
            i + 1,
            ctx.card_type,
            ctx.workspace_name,
            ctx.session_id,
        ));
        let question: String = ctx.question.chars().take(500).collect();
        body.push_str(&format!("  AI raised: {question}\n"));
        if !ctx.options.is_empty() {
            body.push_str("  Options the AI offered:\n");
            for opt in &ctx.options {
                let opt: String = opt.chars().take(200).collect();
                body.push_str(&format!("    - {opt}\n"));
            }
        }
        if ctx.user_choice.trim().is_empty() {
            body.push_str("  User REJECTED the AI's proposal.\n\n");
        } else {
            let choice: String = ctx.user_choice.chars().take(400).collect();
            body.push_str(&format!(
                "  User instead answered (\"Other\"): {choice}\n\n"
            ));
        }
    }

    format!(
        "DECISION-CARD SIGNALS — On this day the user declined the AI's offered \
         choices in the cards below: they typed their own answer via the \"Other\" \
         escape hatch instead of picking an option (or rejected a proposed plan). \
         Each is strong evidence the AI misframed the decision — offered the wrong \
         options, recommended the wrong thing, missed the obvious choice, or asked \
         when it should have just acted. Treat these as candidate evidence for \
         lessons, held to the SAME critical filter below (general, transferable, \
         explains WHY). When the mismatch is only project-specific, skip it.\n\
         \n\
         <decision_signals>\n\
         {body}</decision_signals>\n\n"
    )
}

/// How far back the recurrence pool reaches, and how many past candidates it
/// shows the model. A failure pattern that recurs less than once a fortnight
/// is not worth a standing rule in every session's system prompt.
const POOL_DAYS: i64 = 14;
const POOL_MAX: usize = 80;

/// A single-session candidate from an earlier day, offered to the model so a
/// pattern seen once on Monday and once on Thursday is recognised as recurring.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolEntry {
    pub content: String,
    pub session_id: String,
    pub workspace_name: String,
}

/// One block the model emitted, before evidence validation.
#[derive(Default, Debug)]
struct RawBlock {
    lesson: Option<String>,
    violated: Option<String>,
    reason: Option<String>,
    workspace: Option<String>,
    sessions: Vec<String>,
    note: Option<String>,
}

fn build_lessons_prompt(
    pairs: &[ConversationPair],
    locale: &str,
    existing_rules: &str,
    adopted: &[crate::lessons_store::ManagedLesson],
    pool: &[PoolEntry],
    other_picks: &[crate::decision_history::OtherPickContext],
    task_reviews: &[crate::task_review::TaskReview],
) -> String {
    let lang_instruction = match locale {
        "zh" => "LESSON / REASON / NOTE 的内容请用中文撰写（字段名保持英文）。",
        _ => "Write the field contents in English.",
    };
    let decision_signals = build_decision_signals_section(other_picks);
    let finished_tasks = build_task_review_section(task_reviews);

    let rules_section = if existing_rules.is_empty() {
        String::new()
    } else {
        format!(
            "EXISTING RULES — already in the user's CLAUDE.md files. Do NOT output a \
             LESSON that overlaps with or restates these, even if phrased differently.\n\
             <existing_rules>\n{existing_rules}\n</existing_rules>\n\n"
        )
    };

    let adopted_section = if adopted.is_empty() {
        String::new()
    } else {
        let mut body = String::new();
        for l in adopted {
            let content: String = l.content.chars().take(300).collect();
            body.push_str(&format!("[{}] {}\n", l.id, content));
        }
        format!(
            "ADOPTED LESSONS — the user already adopted these; they are injected into \
             every session. Never output a LESSON that restates one. Instead, when \
             today's evidence shows an agent acting against one, output a VIOLATED \
             block for it (format below).\n\
             <adopted_lessons>\n{body}</adopted_lessons>\n\n"
        )
    };

    let pool_section = if pool.is_empty() {
        String::new()
    } else {
        let mut body = String::new();
        for p in pool {
            let content: String = p.content.chars().take(240).collect();
            body.push_str(&format!(
                "- {content} (workspace: {}, session: {})\n",
                p.workspace_name, p.session_id
            ));
        }
        format!(
            "RECENT CANDIDATES — single-session candidates from the previous {POOL_DAYS} \
             days. When today's evidence shows the SAME failure pattern as one of these, \
             cite that candidate's session id alongside today's.\n\
             <recent_candidates>\n{body}</recent_candidates>\n\n"
        )
    };

    let mut sections = String::new();
    for (i, pair) in pairs.iter().enumerate() {
        let assistant_truncated: String = pair.assistant_text.chars().take(800).collect();
        let user_truncated: String = pair.user_text.chars().take(400).collect();
        sections.push_str(&format!(
            "--- Turn {} (workspace: {}, session: {}) ---\n\
             [AI said]: {}\n\
             [User replied]: {}\n\n",
            i + 1,
            pair.workspace_name,
            pair.session_id,
            assistant_truncated,
            user_truncated,
        ));
    }

    format!(
        "Below is evidence from one day of AI-coding sessions: conversation turns (what \
         the AI said, then the user's reply) and, when present, decision cards where the \
         user declined the AI's offered options, and the day's finished-task reviews.\n\n\
         Your job is NOT to list every correction. A one-off mistake is noise; only a \
         failure pattern that RECURS across sessions earns a standing rule. Work in three \
         steps:\n\
         1. Find each case where the user corrected the AI, rejected an approach, pointed \
            out a mistake, repeated a requirement the AI ignored, or — in a decision card — \
            was offered the wrong choices or asked something that should not have been asked.\n\
         2. Group cases that share the SAME underlying failure (same root cause in the AI's \
            behaviour, not merely the same topic or project), across today's sessions AND \
            the RECENT CANDIDATES.\n\
         3. Output one block per pattern, listing EVERY session it was seen in.\n\n\
         CRITICAL FILTER — only output a pattern if ALL of these hold:\n\
         1. It is a GENERAL principle applicable to any project, not a fix specific to one \
            codebase (a wrong config value or class name is project-specific — skip it).\n\
         2. It explains WHY it matters (what went wrong, what it cost).\n\
         3. An AI would plausibly repeat it in future work.\n\n\
         {rules_section}\
         {adopted_section}\
         {pool_section}\
         Output format, one block per pattern:\n\
         LESSON: <one-sentence actionable rule>\n\
         REASON: <one or two sentences on WHY — what happened and what it cost>\n\
         WORKSPACE: <workspace of the most recent occurrence>\n\
         SESSIONS: <comma-separated session ids where this pattern occurred>\n\n\
         Output a pattern seen in only one session too (it becomes a candidate later days \
         can match), but NEVER invent a session id: list only ids that appear verbatim in \
         the evidence or the RECENT CANDIDATES.\n\n\
         For an adopted lesson the agent acted against today:\n\
         VIOLATED: <adopted lesson id, the bracketed value>\n\
         SESSIONS: <comma-separated session ids from today's evidence>\n\
         NOTE: <one sentence: what the agent did>\n\n\
         If nothing qualifies, output NONE.\n\n\
         {lang_instruction}\n\n\
         {finished_tasks}\
         {decision_signals}\
         ---\n\
         {sections}",
    )
}

fn parse_blocks(output: &str) -> Vec<RawBlock> {
    let mut blocks: Vec<RawBlock> = Vec::new();
    for line in output.lines() {
        let line = line.trim().trim_start_matches(['-', '*']).trim();
        if let Some(rest) = line.strip_prefix("LESSON:") {
            blocks.push(RawBlock {
                lesson: Some(rest.trim().to_string()),
                ..Default::default()
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("VIOLATED:") {
            blocks.push(RawBlock {
                violated: Some(rest.trim().trim_matches(['[', ']']).to_string()),
                ..Default::default()
            });
            continue;
        }
        let Some(cur) = blocks.last_mut() else { continue };
        if let Some(rest) = line.strip_prefix("REASON:") {
            cur.reason = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("WORKSPACE:") {
            cur.workspace = Some(rest.trim().to_string());
        } else if let Some(rest) = line
            .strip_prefix("SESSIONS:")
            .or_else(|| line.strip_prefix("SESSION:"))
        {
            cur.sessions.extend(
                rest.split([',', ' ', ';'])
                    .map(|s| s.trim().trim_matches(['`', '[', ']']))
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        } else if let Some(rest) = line.strip_prefix("NOTE:") {
            cur.note = Some(rest.trim().to_string());
        }
    }
    blocks
}

/// Validate the model's blocks against the evidence it was actually shown and
/// split them by how much evidence backs them.
///
/// The recurrence gate is enforced here, not trusted to the prompt: a lesson
/// reaches the user only when it cites at least two distinct sessions the model
/// really saw, at least one of them from `today_sessions` (otherwise an old
/// pattern would be re-announced every day from the pool alone). Session ids the
/// model invented are dropped before counting.
pub(crate) fn gate_lessons(
    output: &str,
    today_sessions: &HashMap<String, String>,
    pool: &[PoolEntry],
    adopted: &[crate::lessons_store::ManagedLesson],
) -> LessonsOutcome {
    let mut known: HashMap<&str, &str> = today_sessions
        .iter()
        .map(|(id, ws)| (id.as_str(), ws.as_str()))
        .collect();
    for p in pool {
        known
            .entry(p.session_id.as_str())
            .or_insert(p.workspace_name.as_str());
    }

    let mut out = LessonsOutcome::default();
    for b in parse_blocks(output) {
        let mut seen = HashSet::new();
        let evidence: Vec<String> = b
            .sessions
            .iter()
            .filter(|s| known.contains_key(s.as_str()))
            .filter(|s| seen.insert(s.as_str()))
            .cloned()
            .collect();
        let has_today = evidence.iter().any(|s| today_sessions.contains_key(s));

        if let Some(id) = b.violated {
            let Some(adopted_lesson) = adopted.iter().find(|l| l.id == id) else {
                continue;
            };
            let today_ids: Vec<String> = evidence
                .into_iter()
                .filter(|s| today_sessions.contains_key(s))
                .collect();
            if today_ids.is_empty() {
                continue;
            }
            out.violations.push(LessonViolation {
                lesson_id: id,
                lesson_content: adopted_lesson.content.clone(),
                session_ids: today_ids,
                note: b.note.unwrap_or_default(),
            });
            continue;
        }

        let (Some(content), Some(reason)) = (b.lesson, b.reason) else {
            continue;
        };
        if content.is_empty() || !has_today {
            continue;
        }
        let latest_today = evidence
            .iter()
            .find(|s| today_sessions.contains_key(*s))
            .cloned()
            .unwrap_or_default();
        let workspace_name = b
            .workspace
            .filter(|w| !w.is_empty())
            .or_else(|| known.get(latest_today.as_str()).map(|w| w.to_string()))
            .unwrap_or_default();
        let lesson = Lesson {
            content,
            reason,
            workspace_name,
            session_id: latest_today,
            evidence_session_ids: evidence.clone(),
        };
        if evidence.len() >= 2 {
            out.lessons.push(lesson);
        } else {
            out.candidates.push(lesson);
        }
    }
    out
}

/// Earlier days' lessons and candidates, newest first, deduplicated by content:
/// the pool a new day's evidence is matched against for recurrence.
pub fn recent_lesson_pool(date: &str) -> Vec<PoolEntry> {
    let Some(day) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok() else {
        return Vec::new();
    };
    let store = ReportStore::open().ok();
    let mut seen = HashSet::new();
    let mut pool = Vec::new();
    let mut push = |l: &Lesson, pool: &mut Vec<PoolEntry>| {
        if l.session_id.is_empty() || !seen.insert(l.content.clone()) {
            return;
        }
        pool.push(PoolEntry {
            content: l.content.clone(),
            session_id: l.session_id.clone(),
            workspace_name: l.workspace_name.clone(),
        });
    };
    for back in 1..=POOL_DAYS {
        let d = (day - chrono::Duration::days(back))
            .format("%Y-%m-%d")
            .to_string();
        if let Some(store) = &store {
            if let Ok(Some(r)) = store.get_report(&d) {
                for l in r.lessons.iter().flatten() {
                    push(l, &mut pool);
                }
            }
            if let Ok(Some((candidates, _))) = store.get_lesson_pool(&d) {
                for l in &candidates {
                    push(l, &mut pool);
                }
            }
        }
        for r in task_reviews_for_date(&d) {
            for l in &r.lessons {
                push(l, &mut pool);
            }
        }
    }
    pool.truncate(POOL_MAX);
    pool
}

/// Generate lessons for a daily report from its session JSONL files.
/// Returns None if the provider is unavailable or the call failed.
pub fn generate_lessons(
    provider: &dyn LlmProvider,
    model: &str,
    report: &DailyReport,
    locale: &str,
) -> Option<LessonsOutcome> {
    if !provider.is_available() {
        log_debug(&format!(
            "[daily_report] provider '{}' not available for lessons",
            provider.name()
        ));
        return None;
    }

    // Collect conversation pairs from all non-subagent sessions
    let mut all_pairs: Vec<ConversationPair> = Vec::new();
    // Every session id the model may legitimately cite as today's evidence,
    // mapped to its workspace.
    let mut today_sessions: HashMap<String, String> = HashMap::new();

    // We only have session_ids in the report; re-scan to find paths
    let sessions = scan_sessions_for_date(&report.date);
    for si in &sessions {
        if si.is_subagent {
            continue;
        }
        let content = match std::fs::read_to_string(&si.jsonl_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let pairs = extract_conversation_pairs(&content, &si.id, &si.workspace_name);
        // Only include sessions with >= 2 user text turns (pairs)
        if pairs.len() >= 2 {
            today_sessions.insert(si.id.clone(), si.workspace_name.clone());
            all_pairs.extend(pairs);
        }
    }

    // Decision cards where the user overrode the AI's offered choices are an
    // independent evidence source — collect them even when there are no
    // conversation pairs (a session can be all decision cards, no chat).
    const MAX_DECISION_SIGNALS: usize = 40;
    let other_picks =
        crate::decision_history::collect_other_picks_for_date(&report.date, MAX_DECISION_SIGNALS);
    for p in &other_picks {
        today_sessions
            .entry(p.session_id.clone())
            .or_insert_with(|| p.workspace_name.clone());
    }

    // Tasks that ended today were reviewed on their own, knowing the outcome.
    // Their lessons are single-session evidence like any other: they reach the
    // user only if the same pattern shows up in a second session.
    let task_reviews = task_reviews_for_date(&report.date);
    for r in &task_reviews {
        for sid in &r.session_ids {
            today_sessions
                .entry(sid.clone())
                .or_insert_with(|| r.workspace_name.clone());
        }
    }

    if all_pairs.is_empty() && other_picks.is_empty() && task_reviews.is_empty() {
        log_debug("[daily_report] no evidence found for lessons");
        return Some(LessonsOutcome::default());
    }

    let workspace_paths: Vec<String> = sessions
        .iter()
        .filter(|si| !si.is_subagent)
        .map(|si| si.workspace_path.clone())
        .collect();
    let existing_rules = collect_existing_rules(&workspace_paths);
    let adopted = crate::lessons_store::list_lessons();
    let pool = recent_lesson_pool(&report.date);

    let prompt = build_lessons_prompt(
        &all_pairs,
        locale,
        &existing_rules,
        &adopted,
        &pool,
        &other_picks,
        &task_reviews,
    );

    let raw = crate::llm_usage::complete_accounted(
        provider,
        &prompt,
        model,
        LESSONS_TIMEOUT,
        crate::llm_usage::SCENARIO_DAILY_REPORT_LESSONS,
    )?;

    if raw.is_empty() || raw.trim().eq_ignore_ascii_case("NONE") {
        return Some(LessonsOutcome::default());
    }
    Some(gate_lessons(&raw, &today_sessions, &pool, &adopted))
}

/// The task retrospectives whose task ended on `date` (local time). Soft: an
/// unreadable / absent store yields none, and the day-level pass proceeds as it
/// did before per-task reviews existed.
///
/// Public because the report UI reads the same set through the `Backend` trait:
/// "which reviews belong to this date" must have exactly one definition, or the
/// panel and the lessons pass would disagree about a task that ended near
/// midnight.
pub fn task_reviews_for_date(date: &str) -> Vec<crate::task_review::TaskReview> {
    let Some((from_ms, to_ms)) = local_day_bounds_ms(date) else {
        return Vec::new();
    };
    crate::task_review::TaskReviewStore::open()
        .and_then(|s| s.list_in_range(from_ms, to_ms))
        .unwrap_or_default()
}

/// `[start, start + 24h)` in epoch ms for local calendar day `date`
/// (`YYYY-MM-DD`): the one definition of "which instants belong to this day"
/// for everything the report attributes by timestamp.
pub fn local_day_bounds_ms(date: &str) -> Option<(u64, u64)> {
    use chrono::TimeZone;
    let start = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).single())?;
    let from_ms = start.timestamp_millis().max(0) as u64;
    Some((from_ms, from_ms + 24 * 60 * 60 * 1000))
}

/// Render the day's finished task retrospectives for the lessons prompt: how
/// many tasks ended, how they ended, and the single-task lessons each review
/// drew — evidence the day-level pass groups with everything else.
fn build_task_review_section(reviews: &[crate::task_review::TaskReview]) -> String {
    if reviews.is_empty() {
        return String::new();
    }
    let done = reviews.iter().filter(|r| r.outcome.is_success()).count();
    let abandoned = reviews.len() - done;
    let mut body = String::new();
    for r in reviews {
        let verdict = if r.outcome.is_success() {
            "COMPLETED"
        } else {
            "ABANDONED"
        };
        body.push_str(&format!(
            "--- [{verdict}] {} (workspace: {}, sessions: {}) ---\n  {}\n",
            r.title,
            r.workspace_name,
            r.session_ids.join(", "),
            r.summary
        ));
        if r.outcome.is_success() != r.agent_claimed_complete {
            body.push_str(
                "  NOTE: the agent's own completion claim disagreed with the user's verdict.\n",
            );
        }
        for l in &r.lessons {
            body.push_str(&format!(
                "  TASK LESSON (session {}): {}\n",
                l.session_id, l.content
            ));
        }
        body.push('\n');
    }
    format!(
        "FINISHED TASKS — {n} task(s) reached a terminal state today ({done} completed, \
         {abandoned} abandoned), each reviewed on its own with its outcome known. A TASK \
         LESSON is single-session evidence like any other: group it with matching cases \
         from other sessions, and cite its session id when you do.\n\
         \n\
         <finished_tasks>\n{body}</finished_tasks>\n\n",
        n = reviews.len(),
    )
}

pub fn generate_lessons_routed(
    config: &crate::llm_provider::LlmConfig,
    report: &DailyReport,
    locale: &str,
) -> Option<LessonsOutcome> {
    for route in crate::llm_provider::daily_report_routes(config) {
        log_debug(&format!(
            "[daily_report] trying lessons provider '{}' model '{}'",
            route.provider.name(),
            route.model
        ));
        if let Some(lessons) =
            generate_lessons(route.provider.as_ref(), &route.model, report, locale)
        {
            return Some(lessons);
        }
    }
    None
}

// ── Attention ────────────────────────────────────────────────────────────────

/// Everything from one day that needs the user's judgment. The report leads
/// with this; a day where it is empty pushes nothing.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DailyAttention {
    pub date: String,
    /// Relay chains an outsider judged to be polishing or off-goal that day,
    /// one (the latest) per chain.
    pub drift: Vec<crate::drift_check::DriftCheck>,
    /// Lessons seen in two or more sessions, not yet adopted.
    pub lessons: Vec<Lesson>,
    /// Adopted lessons an agent still acted against.
    pub violations: Vec<LessonViolation>,
}

impl DailyAttention {
    pub fn is_empty(&self) -> bool {
        self.drift.is_empty() && self.lessons.is_empty() && self.violations.is_empty()
    }
}

/// Assemble a day's [`DailyAttention`] from the stored drift checks, lessons
/// and lesson pool. Pure reads, no LLM.
pub fn attention_for_date(date: &str) -> DailyAttention {
    let adopted: HashSet<String> = crate::lessons_store::list_lessons()
        .into_iter()
        .map(|l| l.content)
        .collect();
    let store = ReportStore::open().ok();
    let lessons = store
        .as_ref()
        .and_then(|s| s.get_report(date).ok().flatten())
        .and_then(|r| r.lessons)
        .unwrap_or_default()
        .into_iter()
        // Pre-gate lessons cite no evidence; they are history, not a finding.
        .filter(|l| l.evidence_session_ids.len() >= 2)
        .filter(|l| !adopted.contains(&l.content))
        .collect();
    let violations = store
        .as_ref()
        .and_then(|s| s.get_lesson_pool(date).ok().flatten())
        .map(|(_, v)| v)
        .unwrap_or_default();
    DailyAttention {
        date: date.to_string(),
        drift: flagged_drift(crate::drift_check::checks_for_date(date)),
        lessons,
        violations,
    }
}

/// The latest check per chain, kept only when it needs attention.
fn flagged_drift(checks: Vec<crate::drift_check::DriftCheck>) -> Vec<crate::drift_check::DriftCheck> {
    let mut latest: HashMap<String, crate::drift_check::DriftCheck> = HashMap::new();
    for c in checks {
        match latest.get(&c.chain_id) {
            Some(prev) if prev.checked_at >= c.checked_at => {}
            _ => {
                latest.insert(c.chain_id.clone(), c);
            }
        }
    }
    let mut out: Vec<_> = latest
        .into_values()
        .filter(|c| c.verdict.needs_attention())
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.checked_at));
    out
}

/// Add a single lesson to the user's global Claude guidance.
///
/// Delegates to [`crate::lessons_store`], which records the lesson as a
/// sentinel-wrapped block in the managed `~/.claude/fleet-lessons.md` file and
/// ensures a single `@import` of that file is present in `~/.claude/CLAUDE.md`.
/// This replaces the old behaviour of appending a bare `# Lesson (…)` block
/// directly into CLAUDE.md body (which could be neither enumerated nor undone).
pub fn append_lesson_to_claude_md(lesson: &Lesson) -> Result<(), String> {
    crate::lessons_store::add_lesson(lesson).map(|_| ())
}

// ── Session scanning for a specific date ────────────────────────────────────

/// Scan `~/.claude/projects/` for JSONL files with finalized assistant activity
/// on `date` (YYYY-MM-DD) in the local timezone. Unlike the normal session
/// scanner, this has no age limit and is suitable for backfill. Only sessions
/// in the launch registry count — a `claude` the user opened by hand is not
/// Fleet's spend — and a subagent counts when its parent session does.
pub fn scan_sessions_for_date(date: &str) -> Vec<crate::session::SessionInfo> {
    use crate::session::decode_workspace_path_with_parts;

    let registry = crate::launch_spec::registry();
    let registered = |p: &std::path::Path| {
        p.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|id| registry.contains_key(id))
    };

    let projects_dir = match crate::session::get_claude_dir() {
        Some(d) => d.join("projects"),
        None => return vec![],
    };
    let Ok(workspace_entries) = std::fs::read_dir(&projects_dir) else {
        return vec![];
    };

    let mut sessions = Vec::new();

    for ws_entry in workspace_entries.flatten() {
        let ws_path = ws_entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let encoded_name = ws_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if encoded_name.is_empty() {
            continue;
        }

        // Decode workspace path from directory name
        let stripped = encoded_name.trim_start_matches('-');
        let parts: Vec<&str> = stripped.split('-').collect();
        let workspace_path =
            crate::session::heal_workspace_path(&ws_path, decode_workspace_path_with_parts(&parts));
        // Shared helper: collapse `.worktrees/<task-id>` to the repo so a repo's
        // worktree sessions group under one project, matching the session list.
        let workspace_name = crate::session::workspace_name(&workspace_path);

        // Scan JSONL files in this workspace directory (main-agent sessions)
        let Ok(entries) = std::fs::read_dir(&ws_path) else {
            continue;
        };

        for entry in entries.flatten() {
            let file_path = entry.path();

            // Top-level JSONL = main-agent session
            if file_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if !registered(&file_path) {
                    continue;
                }
                if let Some(si) = make_session_info_for_date(
                    &file_path,
                    date,
                    &workspace_path,
                    &workspace_name,
                    false,
                ) {
                    sessions.push(si);
                }
                continue;
            }

            // Sub-directory named <session-uuid>: contains subagents/agent-*.jsonl
            if !file_path.is_dir() || !registered(&file_path) {
                continue;
            }
            let subagents_dir = file_path.join("subagents");
            let Ok(sub_entries) = std::fs::read_dir(&subagents_dir) else {
                continue;
            };
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Some(si) = make_session_info_for_date(
                    &sub_path,
                    date,
                    &workspace_path,
                    &workspace_name,
                    true,
                ) {
                    sessions.push(si);
                }
            }
        }
    }

    sessions
}

/// Build a `SessionInfo` for a JSONL file only if one finalized assistant turn
/// belongs to `date`. Metadata dates cheaply prune files that cannot overlap.
fn make_session_info_for_date(
    file_path: &std::path::Path,
    date: &str,
    workspace_path: &str,
    workspace_name: &str,
    is_subagent: bool,
) -> Option<crate::session::SessionInfo> {
    use crate::session::SessionStatus;

    let meta = file_path.metadata().ok()?;
    let created_time = meta.created().or_else(|_| meta.modified()).ok()?;
    let modified_time = meta.modified().unwrap_or(created_time);
    let created_ms = created_time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let modified_ms = modified_time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let created_date = local_date_from_ms(created_ms)?;
    let modified_date = local_date_from_ms(modified_ms)?;
    if date < created_date.as_str() || date > modified_date.as_str() {
        return None;
    }

    let session_id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())?;

    let jsonl_path = file_path.to_string_lossy().to_string();

    // Confirm activity and extract title from the same disk read.
    let content = std::fs::read_to_string(file_path).unwrap_or_default();
    let mut ai_title: Option<String> = None;
    let mut slug: Option<String> = None;
    let mut active_on_date = false;
    for line in content.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("type").and_then(Value::as_str) == Some("assistant")
                && v.get("message")
                    .and_then(|m| m.get("stop_reason"))
                    .is_some_and(|reason| !reason.is_null())
                && v.get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d")
                            .to_string()
                    })
                    .as_deref()
                    == Some(date)
            {
                active_on_date = true;
            }
            if v.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
                if let Some(t) = v.get("aiTitle").and_then(|t| t.as_str()) {
                    ai_title = Some(t.to_string());
                }
            }
            if let Some(s) = v.get("slug").and_then(|s| s.as_str()) {
                slug = Some(s.to_string());
            }
        }
    }
    if !active_on_date {
        return None;
    }

    Some(crate::session::SessionInfo {
        id: session_id,
        workspace_path: workspace_path.to_string(),
        workspace_name: workspace_name.to_string(),
        entrypoint: None,
        is_subagent,
        // Reporting projection with no entrypoint — never a launchpad task.
        fleet_spawned: false,
        parent_session_id: None,
        agent_type: None,
        agent_description: None,
        slug,
        ai_title,
        status: SessionStatus::Idle,
        token_speed: 0.0,
        agent_token_speed: 0.0,
        total_output_tokens: 0,
        reasoning_output_tokens: 0,
        total_input_tokens: 0,
        total_cost_usd: 0.0,
        agent_total_cost_usd: 0.0,
        cost_speed_usd_per_min: 0.0,
        last_message_preview: None,
        last_activity_ms: 0,
        agent_last_activity_ms: 0,
        running_subagent_count: 0,
        created_at_ms: created_ms,
        jsonl_path,
        model: None,
        thinking_level: None,
        effort: None,
        pid: None,
        pid_precise: false,
        proc_alive: false,
        pending_tool_batch: false,
        stuck_tool: None,
        last_skill: None,
        context_percent: None,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
        rate_limit: None,
        todos: None,
        background_tasks: Vec::new(),
        task_plan: None,
        handoff: None,
        user_mark: None,
        task_outcome: None,
        title_override: None,
        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
        pending_messages: Vec::new(),
        watches: Vec::new(),
        remote_disconnect: None,
        mirror_write: None,
        out_of_credits: None,
    })
}

fn local_date_from_ms(ms: u64) -> Option<String> {
    chrono::DateTime::from_timestamp((ms / 1000) as i64, 0).map(|dt| {
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    })
}

/// Cheap overlap gate for the live multi-source session cache. Exact inclusion
/// is decided later by the per-turn projection, so false positives are harmless.
pub fn session_overlaps_date(si: &crate::session::SessionInfo, date: &str) -> bool {
    let Some(created) = local_date_from_ms(si.created_at_ms) else {
        return false;
    };
    let Some(last) = local_date_from_ms(si.last_activity_ms.max(si.created_at_ms)) else {
        return false;
    };
    created.as_str() <= date && date <= last.as_str()
}

// ── Report scheduler ────────────────────────────────────────────────────────

/// Called with a date (`YYYY-MM-DD`) when that day first has something that
/// needs the user's judgment. See [`start_report_scheduler`].
pub type ReportReadyHook = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Start the background report scheduler thread.
/// Checks every 10 minutes for missing reports and generates them.
///
/// `running` is a shared cancellation flag. The caller flips it to false
/// (typically from their `Drop` impl) to signal the thread to exit — otherwise
/// successive backend swaps would stack up zombie scheduler threads.
///
/// `on_report_ready` fires once per date, and only when that date produced
/// something that needs the user's judgment — a quiet day pushes nothing. It is
/// deliberately NOT wired to pass 1: today's metrics are recomputed on every
/// 10-minute tick, so a caller that popped a window on "report saved" would
/// pop dozens of times a day. Fired from the scheduler thread; keep the
/// closure cheap and non-blocking.
pub fn start_report_scheduler(
    report_store: std::sync::Arc<std::sync::Mutex<ReportStore>>,
    locale: std::sync::Arc<std::sync::Mutex<String>>,
    llm_config: std::sync::Arc<std::sync::Mutex<crate::llm_provider::LlmConfig>>,
    live_sessions: std::sync::Arc<std::sync::Mutex<Vec<crate::session::SessionInfo>>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    on_report_ready: Option<ReportReadyHook>,
) {
    use std::sync::atomic::Ordering;

    // Sleep in 1s chunks so cancellation latency stays bounded.
    fn sleep_checked(total: Duration, running: &std::sync::atomic::AtomicBool) -> bool {
        let step = Duration::from_secs(1);
        let mut remaining = total;
        while remaining > Duration::ZERO {
            if !running.load(Ordering::SeqCst) {
                return false;
            }
            let chunk = remaining.min(step);
            std::thread::sleep(chunk);
            remaining = remaining.saturating_sub(chunk);
        }
        running.load(Ordering::SeqCst)
    }

    std::thread::Builder::new()
        .name("report-scheduler".into())
        .spawn(move || {
            // Short initial delay to let the app start, then generate immediately
            if !sleep_checked(Duration::from_secs(10), &running) {
                return;
            }

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                let lang = locale.lock().unwrap().clone();
                let rs = report_store.clone();
                let cfg = llm_config.lock().unwrap().clone();
                let session_snapshot = live_sessions.lock().unwrap().clone();
                let hook = on_report_ready.clone();
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_backfill_check(&rs, &lang, &cfg, &session_snapshot, hook.as_ref());
                })) {
                    Ok(()) => {}
                    Err(e) => {
                        let msg = if let Some(s) = e.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = e.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        log_debug(&format!("[report-scheduler] PANIC in backfill: {msg}"));
                    }
                }
                // Check every 10 minutes so today's report stays fresh
                if !sleep_checked(Duration::from_secs(10 * 60), &running) {
                    break;
                }
            }
        })
        .expect("spawn report-scheduler");
}

/// Tracks AI generation failures to avoid retrying on every scheduler pass.
/// Key = date string, value = timestamp of last failed attempt.
static AI_FAILURE_COOLDOWN: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Cooldown before retrying failed AI generation for a given date.
const AI_RETRY_COOLDOWN: Duration = Duration::from_secs(2 * 3600); // 2 hours

/// Helper to lock report_store, recovering from poison (a prior panic while
/// the lock was held).
fn lock_store(
    store: &std::sync::Arc<std::sync::Mutex<ReportStore>>,
) -> std::sync::MutexGuard<'_, ReportStore> {
    store.lock().unwrap_or_else(|poisoned| {
        log_debug("[report-scheduler] recovering from poisoned report_store mutex");
        poisoned.into_inner()
    })
}

/// The timezone tag stored on a report: the UTC offset that `date`'s **local
/// midnight** had, e.g. `-0400`. This is the thing that actually decides which
/// turns land in the day, so it is what a cached report must be checked against.
///
/// Deliberately not `%Z` (the abbreviation, `EDT` / `CST`), which the field used
/// to hold: an abbreviation flips twice a year at every DST boundary without the
/// bucketing changing at all (`chrono::Local` resolves each historical timestamp
/// with *that day's* offset), so comparing abbreviations would re-scan 90 days of
/// transcripts every spring and autumn for nothing. An offset pinned to the day
/// changes only when the machine really moves to another timezone.
pub fn local_tz_tag(date: &str) -> String {
    use chrono::TimeZone;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).earliest())
        .map(|dt| dt.format("%z").to_string())
        .unwrap_or_else(|| chrono::Local::now().format("%z").to_string())
}

/// Whether a **past** day's report must be (re)generated during backfill.
///
/// Regenerate when there is no cached report, when the cached one was computed
/// under an older metrics methodology ([`DailyMetrics::metrics_version`] <
/// [`CURRENT_METRICS_VERSION`]) — otherwise a methodology change (e.g. switching token
/// totals to cumulative-incl-cache) would never reach historical reports, which
/// are skipped on every pass once cached — **or** when the machine's timezone
/// moved since that report was written, which shifts the day boundary and so
/// changes which turns belong to the day. `days_ago == 0` (today) is handled by
/// its own always-regenerate path and never routed through here.
///
/// Reports written before the tz field held an offset carry an abbreviation
/// (`EDT`) or a placeholder (`local`); those are treated as unknown and left
/// alone, so introducing this check does not trigger one full 90-day re-scan.
pub(crate) fn past_report_needs_regen(existing: Option<&DailyReport>, expected_tz: &str) -> bool {
    let Some(r) = existing else { return true };
    if r.metrics.metrics_version < CURRENT_METRICS_VERSION {
        return true;
    }
    is_tz_offset_tag(&r.timezone) && r.timezone != expected_tz
}

/// `+0800` / `-0400` — the shape [`local_tz_tag`] writes. Anything else is a
/// legacy value we cannot compare against.
fn is_tz_offset_tag(s: &str) -> bool {
    s.len() == 5
        && matches!(s.as_bytes()[0], b'+' | b'-')
        && s[1..].bytes().all(|b| b.is_ascii_digit())
}

fn run_backfill_check(
    report_store: &std::sync::Arc<std::sync::Mutex<ReportStore>>,
    locale: &str,
    llm_config: &crate::llm_provider::LlmConfig,
    live_sessions: &[crate::session::SessionInfo],
    on_report_ready: Option<&ReportReadyHook>,
) {
    let today = chrono::Local::now();
    log_debug("[report-scheduler] backfill pass started");

    // ── Pass 1: Generate basic metrics reports (fast) ────────────────────────
    // This pass MUST complete quickly so that reports are always available
    // when the user opens the UI.
    for days_ago in 0..=90 {
        let date = (today - chrono::Duration::days(days_ago))
            .format("%Y-%m-%d")
            .to_string();

        let existing = {
            let store = lock_store(report_store);
            store.get_report(&date).ok().flatten()
        };

        let tz = local_tz_tag(&date);

        // For today, always regenerate (new sessions keep arriving). For past
        // days, regenerate only when there's no cached report, the cached one
        // was computed under an older metrics methodology (so a methodology change backfills
        // into history instead of stopping at today), or the machine's timezone
        // moved (which moves the day boundary under the cached numbers).
        if days_ago > 0 && !past_report_needs_regen(existing.as_ref(), &tz) {
            continue;
        }

        let mut sessions = scan_sessions_for_date(&date);
        let mut known: HashSet<(String, String)> = sessions
            .iter()
            .map(|s| (s.agent_source.clone(), s.id.clone()))
            .collect();
        for session in live_sessions {
            let key = (session.agent_source.clone(), session.id.clone());
            if session_overlaps_date(session, &date) && known.insert(key) {
                sessions.push(session.clone());
            }
        }
        // A cached day with no Fleet session left still gets rewritten, or the
        // hand-opened spend a v2 report counted would stay on it forever.
        if sessions.is_empty() && existing.is_none() {
            continue;
        }
        let session_refs: Vec<&crate::session::SessionInfo> = sessions.iter().collect();
        let mut r = generate_report_from_sessions(&date, &tz, &session_refs);
        // Preserve the (expensive, LLM-generated) legacy summary and lessons from a
        // stale report we're re-scanning purely to refresh token metrics —
        // regeneration only recomputes the deterministic JSONL fold, not the AI
        // outputs, so carry those forward rather than dropping them.
        if let Some(prev) = &existing {
            r.ai_summary = prev.ai_summary.clone();
            r.ai_summary_generated_at = prev.ai_summary_generated_at;
            r.lessons = prev.lessons.clone();
            r.lessons_generated_at = prev.lessons_generated_at;
        }
        {
            let store = lock_store(report_store);
            if let Err(e) = store.save_report(&r) {
                log_debug(&format!(
                    "[report-scheduler] save report for {date} failed: {e}"
                ));
                continue;
            }
        }
        if days_ago == 0 {
            log_debug(&format!(
                "[report-scheduler] refreshed today's report: {} sessions",
                r.metrics.total_sessions
            ));
        } else {
            log_debug(&format!(
                "[report-scheduler] generated report for {}: {} sessions",
                date, r.metrics.total_sessions
            ));
        }
    }

    // ── Pass 2: Generate lessons for recent days (slow) ──────────────────────
    // This is separated so that slow/failing AI generation never blocks
    // basic report availability.  Starts at 1 (yesterday) because today's
    // data is incomplete.
    for days_ago in 1..=7 {
        let date = (today - chrono::Duration::days(days_ago))
            .format("%Y-%m-%d")
            .to_string();

        let report = {
            let store = lock_store(report_store);
            store.get_report(&date).ok().flatten()
        };
        let Some(report) = report else { continue };

        if report.lessons.is_some() {
            continue;
        }

        // Check cooldown: don't retry if we failed recently
        {
            let cooldowns = AI_FAILURE_COOLDOWN
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(last_failure) = cooldowns.get(&date) {
                if last_failure.elapsed() < AI_RETRY_COOLDOWN {
                    continue;
                }
            }
        }

        let mut any_failed = false;
        // Whether *this* pass produced something worth the user's attention.
        // Only a first-time generation counts: on later passes the `is_some()`
        // guard above skips the date entirely, so the hook can never re-fire.
        let mut became_readable = false;

        if report.lessons.is_none() {
            log_debug(&format!(
                "[report-scheduler] generating lessons for {date}..."
            ));
            if let Some(outcome) = generate_lessons_routed(llm_config, &report, locale) {
                let store = lock_store(report_store);
                store.save_lessons_outcome(&date, &outcome).ok();
                became_readable = !outcome.lessons.is_empty() || !outcome.violations.is_empty();
                log_debug(&format!(
                    "[report-scheduler] lessons for {date} done ({} recurring, {} candidates, {} violations)",
                    outcome.lessons.len(),
                    outcome.candidates.len(),
                    outcome.violations.len()
                ));
            } else {
                log_debug(&format!("[report-scheduler] lessons for {date} failed"));
                any_failed = true;
            }
        }

        if any_failed {
            AI_FAILURE_COOLDOWN
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(date.clone(), std::time::Instant::now());
        }

        // Announce only when there is something to act on: a quiet day pushes
        // nothing.
        if became_readable {
            if let Some(hook) = on_report_ready {
                hook(&date);
            }
        }
    }

    // ── Pass 3: Drift checks on active relay chains ──────────────────────────
    // Throttled per chain inside `chains_due`, so most ticks make no LLM call.
    let flagged = crate::drift_check::run_due_checks(llm_config, locale);
    if !flagged.is_empty() {
        if let Some(hook) = on_report_ready {
            hook(&today.format("%Y-%m-%d").to_string());
        }
    }

    log_debug("[report-scheduler] backfill pass finished");
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_past_report_is_regenerated_but_current_is_kept() {
        // Regression: after the token-accounting methodology changed, historical daily
        // reports were never re-scanned because backfill skipped any past day
        // that already had a cached report — so old last-turn-snapshot numbers
        // never got backfilled to the new cumulative methodology.

        // No cached report → must generate.
        assert!(
            past_report_needs_regen(None, "-0400"),
            "missing report must generate"
        );

        // Cached under the current methodology → leave it alone (no needless re-scan).
        let mut current = make_test_report("2026-07-10");
        current.metrics.metrics_version = CURRENT_METRICS_VERSION;
        assert!(
            !past_report_needs_regen(Some(&current), "-0400"),
            "up-to-date report must NOT be regenerated"
        );

        // Cached under the immediately previous methodology (version 1 used whole
        // creation-day sessions) →
        // must be regenerated so its token totals move to the new basis.
        let mut stale = make_test_report("2026-07-09");
        stale.metrics.metrics_version = CURRENT_METRICS_VERSION - 1;
        assert!(
            past_report_needs_regen(Some(&stale), "-0400"),
            "stale-口径 report must be regenerated"
        );
    }

    #[test]
    fn timezone_move_regenerates_history_but_dst_and_legacy_do_not() {
        // A report's day boundary is the local midnight of the machine that
        // wrote it. Move the machine to another timezone and the boundary moves
        // with it, so every cached day is bucketing turns by a rule that no
        // longer holds — those days must be re-scanned.
        let mut moved = make_test_report("2026-07-09");
        moved.metrics.metrics_version = CURRENT_METRICS_VERSION;
        moved.timezone = "+0800".to_string();
        assert!(
            past_report_needs_regen(Some(&moved), "-0400"),
            "a report written in another timezone must be regenerated"
        );

        // Same offset → nothing moved, no re-scan.
        moved.timezone = "-0400".to_string();
        assert!(
            !past_report_needs_regen(Some(&moved), "-0400"),
            "same offset must NOT trigger a re-scan"
        );

        // Legacy reports stored the %Z abbreviation (or a placeholder). We
        // cannot tell whether the boundary moved, and re-scanning all of them
        // once would cost a full 90-day transcript sweep for nothing — leave
        // them until some other methodology change picks them up.
        for legacy in ["EDT", "UTC", "local", "CST", ""] {
            moved.timezone = legacy.to_string();
            assert!(
                !past_report_needs_regen(Some(&moved), "-0400"),
                "legacy tz value {legacy:?} must not trigger a re-scan"
            );
        }
    }

    #[test]
    fn local_tz_tag_is_pinned_to_that_days_offset() {
        // Whatever the machine's timezone is, the tag must be a fixed-shape
        // offset (so `past_report_needs_regen` can compare it) and must be the
        // offset *of that date*, not of today — otherwise every DST boundary
        // would look like a timezone move.
        let january = local_tz_tag("2026-01-15");
        let july = local_tz_tag("2026-07-15");
        assert!(is_tz_offset_tag(&january), "got {january:?}");
        assert!(is_tz_offset_tag(&july), "got {july:?}");
        // Recomputing is stable — the value depends only on the date.
        assert_eq!(january, local_tz_tag("2026-01-15"));

        // A malformed date still yields a usable tag rather than panicking.
        assert!(is_tz_offset_tag(&local_tz_tag("not-a-date")));
    }

    fn make_test_report(date: &str) -> DailyReport {
        DailyReport {
            date: date.to_string(),
            timezone: "UTC".to_string(),
            generated_at: 1000000,
            metrics: DailyMetrics {
                metrics_version: CURRENT_METRICS_VERSION,
                total_input_tokens: 5000,
                total_output_tokens: 3000,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
                total_web_search_requests: 0,
                total_cost_usd: 0.0,
                total_sessions: 2,
                total_subagents: 1,
                total_tool_calls: 10,
                tool_call_breakdown: {
                    let mut m = HashMap::new();
                    m.insert("Edit".to_string(), 5);
                    m.insert("Bash".to_string(), 5);
                    m
                },
                model_breakdown: {
                    let mut m = HashMap::new();
                    m.insert(
                        "claude-sonnet-4-20250514".to_string(),
                        ModelTokens {
                            input_tokens: 5000,
                            output_tokens: 3000,
                            cache_creation_tokens: 0,
                            cache_creation_1h_tokens: 0,
                            cache_read_tokens: 0,
                            cost_usd: 0.0,
                        },
                    );
                    m
                },
                projects: vec![ProjectMetrics {
                    workspace_path: "/home/user/project".to_string(),
                    workspace_name: "project".to_string(),
                    session_count: 2,
                    subagent_count: 1,
                    total_input_tokens: 5000,
                    total_output_tokens: 3000,
                    total_cache_creation_tokens: 0,
                    total_cache_read_tokens: 0,
                    total_web_search_requests: 0,
                    total_cost_usd: 0.0,
                    tool_calls: 10,
                    sessions: vec![SessionSummary {
                        id: "sess-1".to_string(),
                        title: Some("Fix bug".to_string()),
                        last_message: Some("Done fixing".to_string()),
                        model: Some("claude-sonnet-4-20250514".to_string()),
                        is_subagent: false,
                        output_tokens: 2000,
                        cost_usd: 0.0,
                        agent_source: "claude-code".to_string(),
                    }],
                }],
                source_breakdown: {
                    let mut m = HashMap::new();
                    m.insert("claude-code".to_string(), 2);
                    m
                },
                hourly_activity: [0; 24],
                decision_cards: Default::default(),
            },
            ai_summary: None,
            ai_summary_generated_at: None,
            session_ids: vec!["sess-1".to_string(), "sess-2".to_string()],
            lessons: None,
            lessons_generated_at: None,
        }
    }

    #[test]
    fn decision_signals_section_empty_when_no_picks() {
        assert!(build_decision_signals_section(&[]).is_empty());
    }

    #[test]
    fn decision_signals_section_renders_question_options_and_choice() {
        use crate::decision_history::OtherPickContext;
        let picks = vec![
            OtherPickContext {
                card_type: "elicitation".into(),
                workspace_name: "claude-fleet".into(),
                session_id: "s1".into(),
                question: "Which approach?".into(),
                options: vec![
                    "Do it inline (Recommended) — fast".into(),
                    "Refactor first — clean".into(),
                ],
                user_choice: "just rewrite it".into(),
            },
            OtherPickContext {
                card_type: "plan-approval".into(),
                workspace_name: "claude-fleet".into(),
                session_id: "s2".into(),
                question: "Proposed plan (rejected):\ndelete everything".into(),
                options: vec![],
                user_choice: String::new(),
            },
        ];
        let out = build_decision_signals_section(&picks);
        assert!(out.contains("DECISION-CARD SIGNALS"));
        assert!(out.contains("Which approach?"));
        assert!(out.contains("Do it inline (Recommended) — fast"));
        assert!(out.contains("just rewrite it"));
        // Plan rejection with no free-text feedback renders the REJECTED marker.
        assert!(out.contains("User REJECTED the AI's proposal."));
        assert!(out.contains("[plan-approval]"));
    }

    /// A db path no other test in this process can collide with.
    ///
    /// The counter is what makes that true, and it is not redundant with the
    /// timestamp: `SystemTime` is only microsecond-granular on macOS (measured:
    /// the smallest nonzero step between two consecutive `now()` calls is
    /// 1000ns, and 97% of consecutive calls return the *same* value), so two of
    /// these tests entering this function in the same microsecond used to get
    /// byte-identical paths. They then opened the same sqlite file, and
    /// whichever finished first deleted it in its own cleanup — the other one
    /// failed its next statement with "disk I/O error". Intermittent, and only
    /// under the parallelism of a full `cargo test` run.
    fn temp_db_path() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("fleet_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!(
            "test_{}_{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    // ── ReportStore tests ────────────────────────────────────────────────────

    #[test]
    fn test_save_and_get_report() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();
        let report = make_test_report("2026-03-31");
        store.save_report(&report).unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(loaded.date, "2026-03-31");
        assert_eq!(loaded.timezone, "UTC");
        assert_eq!(loaded.generated_at, 1000000);
        assert_eq!(loaded.metrics.total_input_tokens, 5000);
        assert_eq!(loaded.metrics.total_output_tokens, 3000);
        assert_eq!(loaded.metrics.total_sessions, 2);
        assert_eq!(loaded.metrics.total_subagents, 1);
        assert_eq!(loaded.metrics.total_tool_calls, 10);
        assert_eq!(loaded.metrics.projects.len(), 1);
        assert_eq!(loaded.session_ids, vec!["sess-1", "sess-2"]);
        assert!(loaded.ai_summary.is_none());
        assert!(loaded.ai_summary_generated_at.is_none());

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_get_nonexistent_report() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();
        let result = store.get_report("2099-01-01").unwrap();
        assert!(result.is_none());

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_list_stats_range() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();

        for date in &["2026-03-29", "2026-03-30", "2026-03-31"] {
            let report = make_test_report(date);
            store.save_report(&report).unwrap();
        }

        let stats = store.list_stats("2026-03-29", "2026-03-30").unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].date, "2026-03-29");
        assert_eq!(stats[1].date, "2026-03-30");
        assert_eq!(stats[0].total_tokens, 8000); // 5000 + 3000
        assert_eq!(stats[0].total_sessions, 2);
        assert_eq!(stats[0].total_tool_calls, 10);
        assert_eq!(stats[0].total_projects, 1);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_save_overwrites() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();

        let mut report = make_test_report("2026-03-31");
        store.save_report(&report).unwrap();

        // Update and save again
        report.metrics.total_input_tokens = 9999;
        report.generated_at = 2000000;
        store.save_report(&report).unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(loaded.metrics.total_input_tokens, 9999);
        assert_eq!(loaded.generated_at, 2000000);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_list_dates() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();

        for date in &["2026-03-31", "2026-03-29", "2026-03-30"] {
            store.save_report(&make_test_report(date)).unwrap();
        }

        let dates = store.list_dates().unwrap();
        assert_eq!(dates, vec!["2026-03-29", "2026-03-30", "2026-03-31"]);

        let _ = std::fs::remove_file(&db_path);
    }

    // ── Metrics extraction tests ─────────────────────────────────────────────

    #[test]
    fn test_extract_empty_content() {
        let m = extract_session_metrics("");
        assert_eq!(m.input_tokens, 0);
        assert_eq!(m.output_tokens, 0);
        assert!(m.tool_calls.is_empty());
        assert!(m.model.is_none());
    }

    #[test]
    fn test_extract_single_assistant_message() {
        let line = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"hello"},{"type":"tool_use","name":"Edit","id":"tu_1","input":{}}],"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;
        let m = extract_session_metrics(line);
        assert_eq!(m.input_tokens, 115); // 100 + 10 + 5
        assert_eq!(m.output_tokens, 50);
        assert_eq!(m.tool_calls.get("Edit"), Some(&1));
        assert_eq!(m.model.as_deref(), Some("claude-sonnet-4-20250514"));
    }

    /// `<synthetic>` is Claude Code's marker for injected control/error turns
    /// ("No response requested.", "Failed to authenticate. API Error: 403") —
    /// not a model. The effective model is the LAST one seen, so a session that
    /// ends on such a turn books its ENTIRE spend under `<synthetic>` in the
    /// report's model_breakdown, where the receipt then prices it at the
    /// unknown-model fallback. Real data: $53.93 over 30 days, and $62.42 booked
    /// that way on 2026-07-24 alone.
    #[test]
    fn synthetic_control_turn_does_not_become_the_effective_model() {
        let lines = [
            r#"{"type":"assistant","message":{"id":"m1","content":[],"model":"claude-opus-4-8","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}}}"#,
            r#"{"type":"assistant","message":{"id":"m2","content":[],"model":"<synthetic>","stop_reason":"end_turn","usage":{"input_tokens":0,"output_tokens":0}}}"#,
        ];
        let m = extract_session_metrics(&lines.join("\n"));
        assert_eq!(
            m.model.as_deref(),
            Some("claude-opus-4-8"),
            "a trailing control turn must not claim the session's spend"
        );
    }

    /// The stored per-day cost is what the 30d/All receipt shows as its
    /// subtotal, so it has to bill 1-hour cache writes at 2× input, and it has
    /// to persist the 1h subset so the receipt can itemise the two write rates.
    /// Sonnet 5: 1M input ($2) + 1M output ($10) + 1M 1h writes ($4) = $16.
    #[test]
    fn report_metrics_price_one_hour_cache_writes_at_2x() {
        let line = concat!(
            r#"{"type":"assistant","message":{"id":"msg_1","content":[],"model":"claude-sonnet-5","stop_reason":"end_turn","#,
            r#""usage":{"input_tokens":1000000,"output_tokens":1000000,"cache_creation_input_tokens":1000000,"#,
            r#""cache_read_input_tokens":0,"cache_creation":{"ephemeral_1h_input_tokens":1000000,"ephemeral_5m_input_tokens":0}}}}"#,
        );
        let m = extract_session_metrics(line);
        assert_eq!(m.cache_creation_tokens, 1_000_000);
        assert_eq!(
            m.cache_creation_1h_tokens, 1_000_000,
            "1h subset must persist"
        );
        assert!(
            (m.cost_usd - 16.0).abs() < 1e-9,
            "expected $16.00 at the 1h rate, got ${}",
            m.cost_usd
        );
    }

    #[test]
    fn test_extract_multiple_messages() {
        let lines = [
            r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"tool_use","name":"Bash","id":"tu_1","input":{}}],"usage":{"input_tokens":100,"output_tokens":30},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#,
            r#"{"type":"assistant","message":{"id":"msg_2","content":[{"type":"tool_use","name":"Edit","id":"tu_2","input":{}},{"type":"tool_use","name":"Bash","id":"tu_3","input":{}}],"usage":{"input_tokens":200,"output_tokens":60,"cache_creation_input_tokens":20},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#,
        ];
        let content = lines.join("\n");
        let m = extract_session_metrics(&content);

        // input_tokens: cumulative across turns = (100) + (200 + 20) = 320
        assert_eq!(m.input_tokens, 320);
        // output_tokens: 30 + 60 = 90
        assert_eq!(m.output_tokens, 90);
        assert_eq!(m.tool_calls.get("Bash"), Some(&2));
        assert_eq!(m.tool_calls.get("Edit"), Some(&1));
    }

    #[test]
    fn test_extract_dedup_message_ids() {
        let line = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"hello"}],"usage":{"input_tokens":100,"output_tokens":50},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;
        // Same message twice
        let content = format!("{line}\n{line}");
        let m = extract_session_metrics(&content);
        assert_eq!(m.output_tokens, 50); // not 100
    }

    #[test]
    fn test_extract_no_tool_calls() {
        let line = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"Just text, no tools."}],"usage":{"input_tokens":80,"output_tokens":25},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;
        let m = extract_session_metrics(line);
        assert_eq!(m.output_tokens, 25);
        assert!(m.tool_calls.is_empty());
    }

    // ── Report generation tests ──────────────────────────────────────────────

    #[test]
    fn test_generate_report_groups_by_project() {
        // Create temp JSONL files for two sessions in different workspaces
        let dir = std::env::temp_dir().join(format!("fleet_gen_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let jsonl1_path = dir.join("session1.jsonl");
        let jsonl2_path = dir.join("session2.jsonl");

        let line1 = r#"{"type":"assistant","timestamp":"2026-03-31T12:00:00Z","message":{"id":"msg_1","content":[{"type":"tool_use","name":"Edit","id":"tu_1","input":{}}],"usage":{"input_tokens":100,"output_tokens":50},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;
        let previous_day = r#"{"type":"assistant","timestamp":"2026-03-30T12:00:00Z","message":{"id":"msg_old","content":[{"type":"tool_use","name":"Read","id":"tu_old","input":{}}],"usage":{"input_tokens":900,"output_tokens":400},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;
        let line2 = r#"{"type":"assistant","timestamp":"2026-03-31T13:00:00Z","message":{"id":"msg_2","content":[{"type":"tool_use","name":"Bash","id":"tu_2","input":{}}],"usage":{"input_tokens":200,"output_tokens":80},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;

        std::fs::write(&jsonl1_path, format!("{previous_day}\n{line1}")).unwrap();
        std::fs::write(&jsonl2_path, line2).unwrap();

        let s1 = crate::session::SessionInfo {
            id: "s1".to_string(),
            workspace_path: "/project-a".to_string(),
            workspace_name: "project-a".to_string(),
            entrypoint: None,
            is_subagent: false,
            fleet_spawned: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: Some("fix-bug".to_string()),
            ai_title: None,
            status: crate::session::SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 50,
            reasoning_output_tokens: 0,
            total_input_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: Some("Fixed the bug".to_string()),
            last_activity_ms: 0,
            agent_last_activity_ms: 0,
            running_subagent_count: 0,
            created_at_ms: 1743400000000, // some timestamp
            jsonl_path: jsonl1_path.to_string_lossy().to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            thinking_level: None,
            effort: None,
            pid: None,
            pid_precise: false,
            proc_alive: false,
            pending_tool_batch: false,
            stuck_tool: None,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".to_string(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None,
            handoff: None,
            user_mark: None,
            task_outcome: None,
            title_override: None,
            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
            remote_disconnect: None,
            mirror_write: None,
            out_of_credits: None,
        };

        let s2 = crate::session::SessionInfo {
            id: "s2".to_string(),
            workspace_path: "/project-b".to_string(),
            workspace_name: "project-b".to_string(),
            entrypoint: None,
            is_subagent: true,
            fleet_spawned: false,
            parent_session_id: Some("s1".to_string()),
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: Some("Add feature".to_string()),
            status: crate::session::SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 80,
            reasoning_output_tokens: 0,
            total_input_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            agent_last_activity_ms: 0,
            running_subagent_count: 0,
            created_at_ms: 1743400000000,
            jsonl_path: jsonl2_path.to_string_lossy().to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            thinking_level: None,
            effort: None,
            pid: None,
            pid_precise: false,
            proc_alive: false,
            pending_tool_batch: false,
            stuck_tool: None,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".to_string(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None,
            handoff: None,
            user_mark: None,
            task_outcome: None,
            title_override: None,
            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
            remote_disconnect: None,
            mirror_write: None,
            out_of_credits: None,
        };

        let codex_path = dir.join("codex-rollout.jsonl");
        let codex_lines = [
            serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5.6-sol"}})
                .to_string(),
            serde_json::json!({
                "type":"event_msg",
                "timestamp":"2026-03-31T14:00:00Z",
                "payload":{"type":"token_count","info":{"total_token_usage":{
                    "input_tokens":1000,"cached_input_tokens":600,"output_tokens":10
                }}}
            })
            .to_string(),
        ];
        std::fs::write(&codex_path, codex_lines.join("\n")).unwrap();
        let mut s3 = s1.clone();
        s3.id = "s3".to_string();
        s3.workspace_path = "/project-a".to_string();
        s3.workspace_name = "project-a".to_string();
        s3.jsonl_path = format!("codex://{}", codex_path.to_string_lossy());
        s3.agent_source = "codex".to_string();

        let sessions: Vec<&crate::session::SessionInfo> = vec![&s1, &s2, &s3];
        let report = generate_report_from_sessions("2026-03-31", "UTC", &sessions);

        assert_eq!(report.date, "2026-03-31");
        assert_eq!(report.metrics.total_sessions, 3);
        assert_eq!(report.metrics.total_subagents, 1);
        assert_eq!(report.metrics.projects.len(), 2);
        assert_eq!(report.metrics.total_input_tokens, 1300); // Claude 300 + Codex raw input 1000
        assert_eq!(report.metrics.total_output_tokens, 140); // Claude 130 + Codex 10
        assert_eq!(report.metrics.total_tool_calls, 2); // 1 Edit + 1 Bash
        assert_eq!(report.session_ids, vec!["s1", "s2", "s3"]);

        // Verify source breakdown
        assert_eq!(report.metrics.source_breakdown.get("claude-code"), Some(&2));
        assert_eq!(report.metrics.source_breakdown.get("codex"), Some(&1));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Lessons tests ────────────────────────────────────────────────────────

    #[test]
    fn test_extract_conversation_pairs_basic() {
        let jsonl = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Here is my solution."}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"That's wrong, please fix it."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"Good."}]}}"#,
        ].join("\n");

        let pairs = extract_conversation_pairs(&jsonl, "sess-1", "my-project");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].assistant_text, "Here is my solution.");
        assert_eq!(pairs[0].user_text, "That's wrong, please fix it.");
        assert_eq!(pairs[0].session_id, "sess-1");
        assert_eq!(pairs[0].workspace_name, "my-project");
    }

    #[test]
    fn test_extract_skips_tool_result_only_user_messages() {
        let jsonl = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Running tool..."}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"Thanks"}]}}"#,
        ].join("\n");

        let pairs = extract_conversation_pairs(&jsonl, "sess-2", "proj");
        // Tool-result-only user message should be skipped
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].user_text, "Thanks");
    }

    fn today(ids: &[(&str, &str)]) -> HashMap<String, String> {
        ids.iter()
            .map(|(id, ws)| (id.to_string(), ws.to_string()))
            .collect()
    }

    fn adopted(id: &str, content: &str) -> crate::lessons_store::ManagedLesson {
        crate::lessons_store::ManagedLesson {
            id: id.into(),
            content: content.into(),
            reason: String::new(),
            workspace_name: String::new(),
            session_id: String::new(),
        }
    }

    #[test]
    fn gate_promotes_only_patterns_seen_in_two_sessions() {
        let output = "LESSON: Verify before claiming done\nREASON: Twice the user found it broken\nWORKSPACE: a\nSESSIONS: s1, s2\n\nLESSON: Ask fewer questions\nREASON: Once\nWORKSPACE: a\nSESSIONS: s1";
        let out = gate_lessons(output, &today(&[("s1", "a"), ("s2", "b")]), &[], &[]);
        assert_eq!(out.lessons.len(), 1, "two-session pattern must be shown");
        assert_eq!(out.lessons[0].evidence_session_ids, vec!["s1", "s2"]);
        assert_eq!(out.candidates.len(), 1, "one-session pattern is kept as a candidate only");
        assert_eq!(out.candidates[0].content, "Ask fewer questions");
    }

    #[test]
    fn gate_drops_invented_session_ids_before_counting() {
        // The model claims two sessions but one was never shown to it.
        let output = "LESSON: X\nREASON: Y\nSESSIONS: s1, made-up";
        let out = gate_lessons(output, &today(&[("s1", "a")]), &[], &[]);
        assert!(out.lessons.is_empty(), "an invented id must not satisfy the gate");
        assert_eq!(out.candidates.len(), 1);
        assert_eq!(out.candidates[0].evidence_session_ids, vec!["s1"]);
    }

    #[test]
    fn gate_counts_a_pool_match_but_needs_one_session_from_today() {
        let pool = vec![PoolEntry {
            content: "earlier".into(),
            session_id: "old".into(),
            workspace_name: "w".into(),
        }];
        let recur = "LESSON: X\nREASON: Y\nSESSIONS: s1, old";
        let out = gate_lessons(recur, &today(&[("s1", "a")]), &pool, &[]);
        assert_eq!(out.lessons.len(), 1, "today + an earlier candidate is a recurrence");
        assert_eq!(out.lessons[0].session_id, "s1");

        // Only pool sessions: an old pattern must not be re-announced.
        let stale = "LESSON: X\nREASON: Y\nSESSIONS: old";
        let out = gate_lessons(stale, &today(&[("s1", "a")]), &pool, &[]);
        assert!(out.lessons.is_empty() && out.candidates.is_empty());
    }

    #[test]
    fn gate_keeps_violations_of_known_adopted_lessons_only() {
        let output = "VIOLATED: [abc]\nSESSIONS: s1\nNOTE: claimed done without testing\n\nVIOLATED: nope\nSESSIONS: s1\nNOTE: z";
        let out = gate_lessons(
            output,
            &today(&[("s1", "a")]),
            &[],
            &[adopted("abc", "Test before done")],
        );
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].lesson_id, "abc");
        assert_eq!(out.violations[0].lesson_content, "Test before done");
        assert_eq!(out.violations[0].session_ids, vec!["s1"]);
        assert!(out.lessons.is_empty());
    }

    #[test]
    fn gate_on_none_output_is_empty() {
        let out = gate_lessons("NONE", &today(&[("s1", "a")]), &[], &[]);
        assert_eq!(out, LessonsOutcome::default());
    }

    #[test]
    fn flagged_drift_keeps_only_a_chains_latest_verdict() {
        use crate::drift_check::{DriftCheck, DriftVerdict};
        let mk = |id: &str, at: u64, v: DriftVerdict| DriftCheck {
            chain_id: id.into(),
            workspace_path: String::new(),
            workspace_name: String::new(),
            plan_id: None,
            goal: String::new(),
            session_count: 3,
            latest_session_id: String::new(),
            verdict: v,
            evidence: String::new(),
            question: String::new(),
            checked_at: at,
        };
        let out = flagged_drift(vec![
            mk("a", 1, DriftVerdict::Polishing),
            mk("a", 2, DriftVerdict::OnTrack), // later recovery wins
            mk("b", 1, DriftVerdict::GoalShifted),
            mk("c", 1, DriftVerdict::Unclear),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chain_id, "b");
    }

    #[test]
    fn lesson_pool_round_trips() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();
        store.save_report(&make_test_report("2026-03-31")).unwrap();
        assert!(store.get_lesson_pool("2026-03-31").unwrap().is_none());

        let cand = Lesson {
            content: "c".into(),
            reason: "r".into(),
            workspace_name: "w".into(),
            session_id: "s1".into(),
            evidence_session_ids: vec!["s1".into()],
        };
        let outcome = LessonsOutcome {
            lessons: vec![],
            candidates: vec![cand.clone()],
            violations: vec![LessonViolation {
                lesson_id: "abc".into(),
                lesson_content: "t".into(),
                session_ids: vec!["s1".into()],
                note: "n".into(),
            }],
        };
        store.save_lessons_outcome("2026-03-31", &outcome).unwrap();
        let (c, v) = store.get_lesson_pool("2026-03-31").unwrap().unwrap();
        assert_eq!(c, vec![cand]);
        assert_eq!(v.len(), 1);
        let report = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(report.lessons, Some(vec![]), "gated lessons land on the report");

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_save_and_get_report_with_lessons() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();

        let mut report = make_test_report("2026-03-31");
        report.lessons = Some(vec![Lesson {
            content: "Always test first".to_string(),
            reason: "User asked for TDD".to_string(),
            workspace_name: "project".to_string(),
            session_id: "sess-1".to_string(),
            evidence_session_ids: vec!["sess-1".to_string(), "sess-0".to_string()],
        }]);
        report.lessons_generated_at = Some(9999);
        store.save_report(&report).unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        let lessons = loaded.lessons.unwrap();
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].content, "Always test first");
        assert_eq!(loaded.lessons_generated_at, Some(9999));

        let _ = std::fs::remove_file(&db_path);
    }

    /// A db already carrying `lessons` but not `lessons_generated_at` (an old /
    /// partially-migrated schema) must still gain the missing column on open.
    /// Regression guard for the migration running both `ADD COLUMN`s in a single
    /// `execute_batch`: SQLite aborts the whole batch at the first statement's
    /// error, so once `ADD COLUMN lessons` fails as "duplicate column" the
    /// second `ADD COLUMN lessons_generated_at` never ran — and every later
    /// save died with "table daily_reports has no column named
    /// lessons_generated_at". The two ALTERs must be independent.
    #[test]
    fn open_at_heals_db_missing_only_the_second_lessons_column() {
        let db_path = temp_db_path();
        // Seed the stuck state: base table + only the first lessons column.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE daily_reports (
                     date TEXT PRIMARY KEY,
                     timezone TEXT NOT NULL,
                     generated_at INTEGER NOT NULL,
                     metrics TEXT NOT NULL,
                     ai_summary TEXT,
                     ai_summary_generated_at INTEGER,
                     session_ids TEXT NOT NULL
                 );
                 ALTER TABLE daily_reports ADD COLUMN lessons TEXT;",
            )
            .unwrap();
        }

        // open_at must add the missing column despite the first ALTER erroring.
        let store = ReportStore::open_at(&db_path).unwrap();
        let mut report = make_test_report("2026-03-31");
        report.lessons_generated_at = Some(9999);
        store.save_report(&report).unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(loaded.lessons_generated_at, Some(9999));

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_update_lessons() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();

        let report = make_test_report("2026-03-31");
        store.save_report(&report).unwrap();

        assert!(store
            .get_report("2026-03-31")
            .unwrap()
            .unwrap()
            .lessons
            .is_none());

        let lessons = vec![Lesson {
            content: "Use tests".to_string(),
            reason: "Bugs found in prod".to_string(),
            workspace_name: "proj".to_string(),
            session_id: "s1".to_string(),
            evidence_session_ids: Vec::new(),
        }];
        store.update_lessons("2026-03-31", &lessons).unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(loaded.lessons.unwrap().len(), 1);
        assert!(loaded.lessons_generated_at.is_some());

        let _ = std::fs::remove_file(&db_path);
    }
}
