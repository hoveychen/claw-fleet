//! Daily report generation: types, SQLite storage, metrics extraction, and AI summary.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection};
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
    pub ai_summary: Option<String>,
    pub ai_summary_generated_at: Option<u64>,
    pub session_ids: Vec<String>,
    pub lessons: Option<Vec<Lesson>>,
    pub lessons_generated_at: Option<u64>,
}

/// Bump when the token-accounting口径 (or any metrics-fold logic) changes, so
/// [`run_backfill_check`] knows a cached past-day report was computed under an
/// older basis and must be re-scanned. History:
///   0 — implicit for reports predating this field (last-turn input snapshot).
///   1 — cumulative input incl. cache (input + cache_creation + cache_read),
///       matching cost and the sidebar counter's口径.
pub const CURRENT_METRICS_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DailyMetrics {
    /// 口径 version these metrics were computed under. Missing (⇒ 0) in reports
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
    /// on the same口径 as `cost_usd` — NOT net input. Consumers that itemise
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

#[derive(Serialize, Deserialize, Clone, Debug)]
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
}

/// A user text turn paired with the immediately preceding assistant turn.
pub struct ConversationPair {
    assistant_text: String,
    user_text: String,
    session_id: String,
    workspace_name: String,
}

// ── Raw metrics from a single session's JSONL ────────────────────────────────

pub struct SessionMetricsRaw {
    /// Cumulative input tokens across all unique assistant turns
    /// (`Σ input + cache_creation + cache_read`, cache re-reads included) — the
    /// "tokens sent to the API" total, on the same口径 as `cost_usd` and as the
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

        Ok(Self { conn })
    }

    /// Save (INSERT OR REPLACE) a report into both tables.
    pub fn save_report(&self, report: &DailyReport) -> Result<(), String> {
        let metrics_json =
            serde_json::to_string(&report.metrics).map_err(|e| format!("json encode: {e}"))?;
        let session_ids_json = serde_json::to_string(&report.session_ids)
            .map_err(|e| format!("json encode: {e}"))?;
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

        let total_tokens =
            report.metrics.total_input_tokens + report.metrics.total_output_tokens;

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
            Some((date, timezone, generated_at, metrics_json, ai_summary, ai_summary_generated_at, session_ids_json, lessons_json, lessons_generated_at)) => {
                let metrics: DailyMetrics = serde_json::from_str(&metrics_json)
                    .map_err(|e| format!("json decode metrics: {e}"))?;
                let session_ids: Vec<String> = serde_json::from_str(&session_ids_json)
                    .map_err(|e| format!("json decode session_ids: {e}"))?;
                let lessons: Option<Vec<Lesson>> = match lessons_json {
                    Some(j) => Some(serde_json::from_str(&j).map_err(|e| format!("json decode lessons: {e}"))?),
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

    /// Update the AI summary for an existing report.
    pub fn update_ai_summary(&self, date: &str, summary: &str) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.conn
            .execute(
                "UPDATE daily_reports SET ai_summary = ?1, ai_summary_generated_at = ?2 WHERE date = ?3",
                params![summary, now_ms, date],
            )
            .map_err(|e| format!("update summary: {e}"))?;
        Ok(())
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
        info: usize, // index into sessions
    }

    let mut session_data: Vec<SessionData> = Vec::new();
    for (i, si) in sessions.iter().enumerate() {
        let jsonl_content = std::fs::read_to_string(&si.jsonl_path).unwrap_or_default();
        let metrics = extract_session_metrics(&jsonl_content);
        session_data.push(SessionData { metrics, info: i });
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

            let entry = model_breakdown
                .entry(effective_model)
                .or_insert(ModelTokens {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_creation_1h_tokens: 0,
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                });
            entry.input_tokens += sd.metrics.input_tokens;
            entry.output_tokens += sd.metrics.output_tokens;
            entry.cache_creation_tokens += sd.metrics.cache_creation_tokens;
            entry.cache_creation_1h_tokens += sd.metrics.cache_creation_1h_tokens;
            entry.cache_read_tokens += sd.metrics.cache_read_tokens;
            entry.cost_usd += sd.metrics.cost_usd;

            *source_breakdown
                .entry(si.agent_source.clone())
                .or_insert(0) += 1;

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

    let session_ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();

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
            total_sessions: sessions.len() as u32,
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

// ── AI summary generation ────────────────────────────────────────────────────

// 240s, not 120s: on heavy days (100+ sessions) the summary prompt grows large
// enough that sonnet legitimately runs ~2.5 minutes (observed 2026-07-17: the
// 120s budget killed a healthy run and forced a fallback to the next provider,
// while the 180s lessons run over the same report succeeded in 147s).
const AI_SUMMARY_TIMEOUT: Duration = Duration::from_secs(240);

fn build_summary_prompt(report: &DailyReport, locale: &str) -> String {
    let lang_instruction = match locale {
        "zh" => "请用中文撰写。",
        _ => "Write in English.",
    };

    let mut sections = String::new();

    // Aggregate stats
    sections.push_str(&format!(
        "Date: {}\nTotal sessions: {}\nTotal subagents: {}\nTotal input tokens: {}\nTotal output tokens: {}\nTotal tool calls: {}\n\n",
        report.date,
        report.metrics.total_sessions,
        report.metrics.total_subagents,
        report.metrics.total_input_tokens,
        report.metrics.total_output_tokens,
        report.metrics.total_tool_calls,
    ));

    // Tool breakdown
    if !report.metrics.tool_call_breakdown.is_empty() {
        sections.push_str("Tool call breakdown:\n");
        let mut tools: Vec<_> = report.metrics.tool_call_breakdown.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1));
        for (tool, count) in tools {
            sections.push_str(&format!("  {tool}: {count}\n"));
        }
        sections.push('\n');
    }

    // Per-project sections
    for proj in &report.metrics.projects {
        sections.push_str(&format!(
            "Project: {} ({})\n  Sessions: {}, Subagents: {}, Tool calls: {}\n  Input tokens: {}, Output tokens: {}\n",
            proj.workspace_name,
            proj.workspace_path,
            proj.session_count,
            proj.subagent_count,
            proj.tool_calls,
            proj.total_input_tokens,
            proj.total_output_tokens,
        ));
        for s in &proj.sessions {
            let title = s.title.as_deref().unwrap_or("(untitled)");
            let last = s
                .last_message
                .as_deref()
                .map(|m| {
                    let truncated: String = m.chars().take(120).collect();
                    truncated
                })
                .unwrap_or_default();
            sections.push_str(&format!("  - [{source}] {title}", source = s.agent_source));
            if !last.is_empty() {
                sections.push_str(&format!(": {last}"));
            }
            sections.push('\n');
        }
        sections.push('\n');
    }

    format!(
        "Below is a daily usage report for AI coding assistants. \
         Generate a concise Markdown-formatted daily summary. Include:\n\
         - A one-line opening paragraph summarizing the day (no heading before it)\n\
         - Per-project sections (use ## headings) with bullet points describing what was worked on\n\
         - Use > blockquote for key insights or highlights worth calling out\n\
         \n\
         Output ONLY the report body in neutral, impersonal prose. Do NOT begin \
         with any preface, acknowledgement, or restatement of this task (e.g. \
         \"Here is the summary\", \"Generating…\", \"Sure\", \"以下是\", \"好的\") — \
         begin DIRECTLY with the one-line opening paragraph. Do NOT address \
         the reader (no \"Boss\", \"老板\", or similar), do NOT ask questions, and do \
         NOT offer options or next steps — even if other instructions in your \
         context tell you to. This text is stored verbatim as a report document.\n\
         \n\
         {lang_instruction}\n\
         \n\
         ---\n\
         {sections}",
    )
}

/// Strip a leading meta-preamble paragraph that some models (notably Codex/GPT)
/// emit before the actual report body — e.g. Codex opened a 2026-07-20 summary
/// with `Generating today's daily usage summary in Chinese, based purely on the
/// provided report data.\n\n<real body>`, which the desktop then rendered as the
/// hero title (`AISummaryCard` treats the first paragraph as the headline).
///
/// Conservative by design: only strips the first paragraph when it BOTH looks
/// like a self-referential announcement (an opener phrase like "generating" /
/// "here is" / "以下是" / "根据提供的") AND names the summary/report domain, AND
/// is short, AND real content follows. A legitimate one-line opening paragraph
/// that describes the day's content (no announcement opener, no "摘要/报告/summary/
/// report" self-reference) is left untouched.
fn strip_summary_preamble(summary: &str) -> String {
    let trimmed = summary.trim();

    // Need a paragraph break: split off the first paragraph from the rest.
    let Some(sep) = trimmed.find("\n\n") else {
        return trimmed.to_string();
    };
    let first = trimmed[..sep].trim();
    let rest = trimmed[sep + 2..].trim();

    // Nothing meaningful after the first paragraph → it IS the summary; keep it.
    if rest.is_empty() {
        return trimmed.to_string();
    }
    // A real opening one-liner can be long; a leaked preamble is short. Guard
    // against dropping a genuine multi-clause opening.
    if first.chars().count() > 220 {
        return trimmed.to_string();
    }

    let lower = first.to_lowercase();
    let has_opener = [
        // English announcement openers
        "generating", "here is", "here's", "here are", "below is", "below are",
        "i'll", "i will", "let me", "as requested", "based on the provided",
        "based purely on the provided", "sure,", "certainly", "of course",
        // Chinese announcement openers
        "以下是", "以下为", "下面是", "这是", "这份", "好的", "根据提供的", "根据以上",
    ]
    .iter()
    .any(|m| lower.contains(m));
    let has_domain = [
        "summary", "report", "overview", "摘要", "日报", "总结", "报告", "报表",
    ]
    .iter()
    .any(|m| lower.contains(m));

    if has_opener && has_domain {
        rest.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Generate AI summary for a daily report using `claude -p --model sonnet`.
/// Blocks for up to `AI_SUMMARY_TIMEOUT`. Call from a background thread.
pub fn generate_ai_summary(
    provider: &dyn LlmProvider,
    model: &str,
    report: &DailyReport,
    locale: &str,
) -> Option<String> {
    if !provider.is_available() {
        log_debug(&format!(
            "[daily_report] provider '{}' not available",
            provider.name()
        ));
        return None;
    }

    let prompt = build_summary_prompt(report, locale);
    let raw = crate::llm_usage::complete_accounted(
        provider,
        &prompt,
        model,
        AI_SUMMARY_TIMEOUT,
        crate::llm_usage::SCENARIO_DAILY_REPORT_SUMMARY,
    )?;
    // Some models (notably Codex/GPT) prepend a meta announcement before the
    // report body; drop it so the desktop hero title shows the real opening
    // line, not "Generating today's daily usage summary…".
    Some(strip_summary_preamble(&raw))
}

pub fn generate_ai_summary_routed(
    config: &crate::llm_provider::LlmConfig,
    report: &DailyReport,
    locale: &str,
) -> Option<String> {
    for route in crate::llm_provider::daily_report_routes(config) {
        log_debug(&format!("[daily_report] trying summary provider '{}' model '{}'", route.provider.name(), route.model));
        if let Some(summary) = generate_ai_summary(route.provider.as_ref(), &route.model, report, locale) {
            return Some(summary);
        }
    }
    None
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
    let truncate = |s: &str| -> String {
        s.chars().take(2000).collect()
    };

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
            body.push_str(&format!("  User instead answered (\"Other\"): {choice}\n\n"));
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

fn build_lessons_prompt(
    pairs: &[ConversationPair],
    locale: &str,
    existing_rules: &str,
    other_picks: &[crate::decision_history::OtherPickContext],
) -> String {
    let lang_instruction = match locale {
        "zh" => "请用中文撰写输出。",
        _ => "Write the output in English.",
    };
    let decision_signals = build_decision_signals_section(other_picks);

    let dedup_section = if existing_rules.is_empty() {
        String::new()
    } else {
        format!(
            "DEDUPLICATION — The following rules/lessons are ALREADY recorded in the user's \
             CLAUDE.md files. Do NOT output any lesson that overlaps with or restates these \
             existing rules, even if phrased differently. Only output genuinely NEW insights.\n\
             \n\
             <existing_rules>\n\
             {existing_rules}\n\
             </existing_rules>\n\n"
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
        "Below are two kinds of evidence from a day of AI-coding sessions. \
         First, conversation turns: what the AI said, followed by the user's reply. \
         Second (when present), DECISION-CARD SIGNALS: decision cards where the user \
         declined the AI's offered options and answered via \"Other\", or rejected a \
         proposed plan.\n\n\
         Your task: across BOTH kinds of evidence, identify cases where the user \
         corrected the AI, rejected an approach, pointed out a mistake, repeated a \
         requirement the AI ignored, or — in the decision cards — was offered the \
         wrong choices / a wrong recommendation / a decision that should not have \
         been asked at all.\n\n\
         CRITICAL FILTER — only include a lesson if ALL of these are true:\n\
         1. It is a GENERAL principle applicable to any project, not a fix specific to this codebase \
            (e.g. \"wrong config value for Tauri\" or \"wrong CSS class name\" are project-specific — skip them).\n\
         2. The lesson explains WHY the rule matters (what went wrong, what the consequence was), \
            not just WHAT to do.\n\
         3. The mistake represents a pattern an AI would plausibly repeat in future projects.\n\n\
         Good lesson examples:\n\
         - \"Never run `git stash drop` after a failed stash pop\" — WHY: it permanently destroys \
           uncommitted work; recovery requires `git fsck` before GC runs.\n\
         - \"Answer the specific question asked; do not substitute a related but different question\" \
           — WHY: the user loses trust and wastes time correcting scope before getting the real answer.\n\n\
         Bad lesson examples (skip these):\n\
         - \"Use `Overlay` not `hidden` for Tauri titleBarStyle\" — project/framework-specific config detail.\n\
         - \"Add i18n keys for all labels\" — obvious coding standard, not an insightful transferable lesson.\n\
         - \"Apply the effect only to the mascot component\" — one-off UI correction, not a general principle.\n\n\
         {dedup_section}\
         For each qualifying lesson, output exactly:\n\
         LESSON: <one-sentence actionable rule>\n\
         REASON: <one-to-two sentences explaining WHY — what went wrong and what the consequence was>\n\
         WORKSPACE: <workspace name>\n\
         SESSION: <session id>\n\n\
         If no qualifying lessons exist, output NONE.\n\n\
         {lang_instruction}\n\n\
         {decision_signals}\
         ---\n\
         {sections}",
    )
}

fn parse_lessons(output: &str, pairs: &[ConversationPair]) -> Vec<Lesson> {
    let mut lessons = Vec::new();
    let mut current_content: Option<String> = None;
    let mut current_reason: Option<String> = None;
    let mut current_workspace: Option<String> = None;
    let mut current_session: Option<String> = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("LESSON:") {
            // Flush previous
            if let (Some(content), Some(reason)) = (current_content.take(), current_reason.take()) {
                let workspace = current_workspace.take().unwrap_or_else(|| {
                    pairs.first().map(|p| p.workspace_name.clone()).unwrap_or_default()
                });
                let session = current_session.take().unwrap_or_default();
                lessons.push(Lesson { content, reason, workspace_name: workspace, session_id: session });
            }
            current_content = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("REASON:") {
            current_reason = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("WORKSPACE:") {
            current_workspace = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("SESSION:") {
            current_session = Some(rest.trim().to_string());
        }
    }

    // Flush final
    if let (Some(content), Some(reason)) = (current_content, current_reason) {
        let workspace = current_workspace.unwrap_or_else(|| {
            pairs.first().map(|p| p.workspace_name.clone()).unwrap_or_default()
        });
        let session = current_session.unwrap_or_default();
        lessons.push(Lesson { content, reason, workspace_name: workspace, session_id: session });
    }

    lessons
}

/// Generate lessons for a daily report from its session JSONL files.
/// Returns None if claude CLI is unavailable or no conversation pairs found.
pub fn generate_lessons(
    provider: &dyn LlmProvider,
    model: &str,
    report: &DailyReport,
    locale: &str,
) -> Option<Vec<Lesson>> {
    if !provider.is_available() {
        log_debug(&format!(
            "[daily_report] provider '{}' not available for lessons",
            provider.name()
        ));
        return None;
    }

    // Collect conversation pairs from all non-subagent sessions
    let mut all_pairs: Vec<ConversationPair> = Vec::new();

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
            all_pairs.extend(pairs);
        }
    }

    // Decision cards where the user overrode the AI's offered choices are an
    // independent evidence source — collect them even when there are no
    // conversation pairs (a session can be all decision cards, no chat).
    const MAX_DECISION_SIGNALS: usize = 40;
    let other_picks =
        crate::decision_history::collect_other_picks_for_date(&report.date, MAX_DECISION_SIGNALS);

    if all_pairs.is_empty() && other_picks.is_empty() {
        log_debug("[daily_report] no conversation pairs or decision signals found for lessons");
        return Some(vec![]);
    }

    // Collect workspace paths for deduplication against existing CLAUDE.md rules
    let workspace_paths: Vec<String> = sessions
        .iter()
        .filter(|si| !si.is_subagent)
        .map(|si| si.workspace_path.clone())
        .collect();
    let existing_rules = collect_existing_rules(&workspace_paths);

    let prompt = build_lessons_prompt(&all_pairs, locale, &existing_rules, &other_picks);

    let raw = match crate::llm_usage::complete_accounted(
        provider,
        &prompt,
        model,
        LESSONS_TIMEOUT,
        crate::llm_usage::SCENARIO_DAILY_REPORT_LESSONS,
    ) {
        Some(r) => r,
        None => return None,
    };

    if raw.is_empty() || raw.eq_ignore_ascii_case("NONE") {
        return Some(vec![]);
    }

    Some(parse_lessons(&raw, &all_pairs))
}

pub fn generate_lessons_routed(
    config: &crate::llm_provider::LlmConfig,
    report: &DailyReport,
    locale: &str,
) -> Option<Vec<Lesson>> {
    for route in crate::llm_provider::daily_report_routes(config) {
        log_debug(&format!("[daily_report] trying lessons provider '{}' model '{}'", route.provider.name(), route.model));
        if let Some(lessons) = generate_lessons(route.provider.as_ref(), &route.model, report, locale) {
            return Some(lessons);
        }
    }
    None
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

/// Scan `~/.claude/projects/` for JSONL files whose creation date matches `date`
/// (YYYY-MM-DD) in the local timezone.  Unlike the normal session scanner, this
/// has no age limit and is suitable for backfill.
pub fn scan_sessions_for_date(date: &str) -> Vec<crate::session::SessionInfo> {
    use crate::session::decode_workspace_path_with_parts;

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
        let workspace_path = crate::session::heal_workspace_path(
            &ws_path,
            decode_workspace_path_with_parts(&parts),
        );
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
                if let Some(si) = make_session_info_for_date(
                    &file_path, date, &workspace_path, &workspace_name, false,
                ) {
                    sessions.push(si);
                }
                continue;
            }

            // Sub-directory named <session-uuid>: contains subagents/agent-*.jsonl
            if !file_path.is_dir() {
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
                    &sub_path, date, &workspace_path, &workspace_name, true,
                ) {
                    sessions.push(si);
                }
            }
        }
    }

    sessions
}

/// Build a `SessionInfo` for a JSONL file only if its creation date matches `date`.
fn make_session_info_for_date(
    file_path: &std::path::Path,
    date: &str,
    workspace_path: &str,
    workspace_name: &str,
    is_subagent: bool,
) -> Option<crate::session::SessionInfo> {
    use crate::session::SessionStatus;

    let meta = file_path.metadata().ok()?;
    let sys_time = meta.created().or_else(|_| meta.modified()).ok()?;
    let created_ms = sys_time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let secs = (created_ms / 1000) as i64;
    let dt = chrono::DateTime::from_timestamp(secs, 0)?;
    let local = dt.with_timezone(&chrono::Local);
    if local.format("%Y-%m-%d").to_string() != date {
        return None;
    }

    let session_id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())?;

    let jsonl_path = file_path.to_string_lossy().to_string();

    // Extract title from JSONL content: look for ai-title line or slug
    let content = std::fs::read_to_string(file_path).unwrap_or_default();
    let mut ai_title: Option<String> = None;
    let mut slug: Option<String> = None;
    for line in content.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
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

    Some(crate::session::SessionInfo {
        id: session_id,
        workspace_path: workspace_path.to_string(),
        workspace_name: workspace_name.to_string(),
        ide_name: None,
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
        last_skill: None,
        context_percent: None,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
        rate_limit: None,
        todos: None,
        background_tasks: Vec::new(),
        task_plan: None, handoff: None, user_mark: None, title_override: None, last_read_ms: None,        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
        pending_messages: Vec::new(),
        watches: Vec::new(),
        remote_disconnect: None,
        mirror_write: None,
    })
}

// ── Report scheduler ────────────────────────────────────────────────────────

/// Start the background report scheduler thread.
/// Checks every 10 minutes for missing reports and generates them.
///
/// `running` is a shared cancellation flag. The caller flips it to false
/// (typically from their `Drop` impl) to signal the thread to exit — otherwise
/// successive backend swaps would stack up zombie scheduler threads.
pub fn start_report_scheduler(
    report_store: std::sync::Arc<std::sync::Mutex<ReportStore>>,
    locale: std::sync::Arc<std::sync::Mutex<String>>,
    llm_config: std::sync::Arc<std::sync::Mutex<crate::llm_provider::LlmConfig>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_backfill_check(&rs, &lang, &cfg);
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
                        log_debug(&format!(
                            "[report-scheduler] PANIC in backfill: {msg}"
                        ));
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

/// Whether a **past** day's report must be (re)generated during backfill.
///
/// Regenerate when there is no cached report, **or** when the cached one was
/// computed under an older metrics口径 ([`DailyMetrics::metrics_version`] <
/// [`CURRENT_METRICS_VERSION`]) — otherwise a口径 change (e.g. switching token
/// totals to cumulative-incl-cache) would never reach historical reports, which
/// are skipped on every pass once cached. `days_ago == 0` (today) is handled by
/// its own always-regenerate path and never routed through here.
pub(crate) fn past_report_needs_regen(existing: Option<&DailyReport>) -> bool {
    existing.map_or(true, |r| r.metrics.metrics_version < CURRENT_METRICS_VERSION)
}

fn run_backfill_check(
    report_store: &std::sync::Arc<std::sync::Mutex<ReportStore>>,
    locale: &str,
    llm_config: &crate::llm_provider::LlmConfig,
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

        // For today, always regenerate (new sessions keep arriving). For past
        // days, regenerate only when there's no cached report OR the cached one
        // was computed under an older metrics口径 (so a口径 change backfills
        // into history instead of stopping at today).
        if days_ago > 0 && !past_report_needs_regen(existing.as_ref()) {
            continue;
        }

        let sessions = scan_sessions_for_date(&date);
        if sessions.is_empty() {
            continue;
        }
        let session_refs: Vec<&crate::session::SessionInfo> = sessions.iter().collect();
        let tz = chrono::Local::now().format("%Z").to_string();
        let mut r = generate_report_from_sessions(&date, &tz, &session_refs);
        // Preserve the (expensive, LLM-generated) AI summary and lessons from a
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
                log_debug(&format!("[report-scheduler] save report for {date} failed: {e}"));
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

    // ── Pass 2: Generate AI summary + lessons for recent days (slow) ─────────
    // This is separated so that slow/failing AI generation never blocks
    // basic report availability.  Starts at 1 (yesterday) because today's
    // data is incomplete — AI summary would be based on partial sessions.
    for days_ago in 1..=7 {
        let date = (today - chrono::Duration::days(days_ago))
            .format("%Y-%m-%d")
            .to_string();

        let report = {
            let store = lock_store(report_store);
            store.get_report(&date).ok().flatten()
        };
        let Some(report) = report else { continue };

        // Skip if both AI summary and lessons already exist
        if report.ai_summary.is_some() && report.lessons.is_some() {
            continue;
        }

        // Check cooldown: don't retry if we failed recently
        {
            let cooldowns = AI_FAILURE_COOLDOWN.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(last_failure) = cooldowns.get(&date) {
                if last_failure.elapsed() < AI_RETRY_COOLDOWN {
                    continue;
                }
            }
        }

        let mut any_failed = false;

        if report.ai_summary.is_none() {
            log_debug(&format!("[report-scheduler] generating AI summary for {date}..."));
            if let Some(summary) = generate_ai_summary_routed(llm_config, &report, locale) {
                let store = lock_store(report_store);
                store.update_ai_summary(&date, &summary).ok();
                log_debug(&format!("[report-scheduler] AI summary for {date} done"));
            } else {
                log_debug(&format!("[report-scheduler] AI summary for {date} failed"));
                any_failed = true;
            }
        }
        if report.lessons.is_none() {
            log_debug(&format!("[report-scheduler] generating lessons for {date}..."));
            if let Some(lessons) = generate_lessons_routed(llm_config, &report, locale) {
                let store = lock_store(report_store);
                store.update_lessons(&date, &lessons).ok();
                log_debug(&format!(
                    "[report-scheduler] lessons for {date} done ({} found)",
                    lessons.len()
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
                .insert(date, std::time::Instant::now());
        }
    }

    log_debug("[report-scheduler] backfill pass finished");
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_summary_preamble_drops_codex_leaked_opener() {
        // Regression (2026-07-20): Codex opened the summary with a meta line
        // that the desktop then rendered as the hero title. The real body must
        // survive; the announcement paragraph must be gone.
        let leaked = "Generating today's daily usage summary in Chinese, based purely on the provided report data.\n\n2026年7月20日全天共运行84个AI编码助手会话，工作重心集中在四个项目。\n\n## mslug3-remake\n\n- 持续推进";
        let cleaned = strip_summary_preamble(leaked);
        assert!(
            cleaned.starts_with("2026年7月20日"),
            "real body must become the first paragraph, got: {cleaned:?}"
        );
        assert!(
            !cleaned.contains("based purely on the provided"),
            "leaked preamble must be stripped, got: {cleaned:?}"
        );
    }

    #[test]
    fn strip_summary_preamble_drops_chinese_opener() {
        let leaked = "以下是根据数据整理的今日日报摘要：\n\n2026年7月19日全天共运行47个会话。\n\n## claude-fleet\n\n- 排查问题";
        let cleaned = strip_summary_preamble(leaked);
        assert!(cleaned.starts_with("2026年7月19日"), "got: {cleaned:?}");
        assert!(!cleaned.contains("以下是"), "got: {cleaned:?}");
    }

    #[test]
    fn strip_summary_preamble_keeps_legit_opening() {
        // A genuine one-line opening that describes the day's content (no
        // announcement opener, no self-reference to "摘要/报告") must NOT be
        // dropped, even though it is followed by a blank line + a heading.
        let good = "根据今日数据，共运行84个AI编码助手会话，覆盖四个项目。\n\n## mslug3-remake\n\n- 推进建模";
        assert_eq!(strip_summary_preamble(good), good.trim());

        // Single-paragraph summary (no blank-line break) is left as-is.
        let single = "2026年7月20日只有一段总结，没有分段。";
        assert_eq!(strip_summary_preamble(single), single);
    }

    #[test]
    fn stale_past_report_is_regenerated_but_current_is_kept() {
        // Regression: after the token-accounting口径 changed, historical daily
        // reports were never re-scanned because backfill skipped any past day
        // that already had a cached report — so old last-turn-snapshot numbers
        // never got backfilled to the new cumulative口径.

        // No cached report → must generate.
        assert!(past_report_needs_regen(None), "missing report must generate");

        // Cached under the current口径 → leave it alone (no needless re-scan).
        let mut current = make_test_report("2026-07-10");
        current.metrics.metrics_version = CURRENT_METRICS_VERSION;
        assert!(
            !past_report_needs_regen(Some(&current)),
            "up-to-date report must NOT be regenerated"
        );

        // Cached under an older口径 (e.g. version 0, the pre-field default) →
        // must be regenerated so its token totals move to the new basis.
        let mut stale = make_test_report("2026-07-09");
        stale.metrics.metrics_version = 0;
        assert!(
            past_report_needs_regen(Some(&stale)),
            "stale-口径 report must be regenerated"
        );
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

    fn temp_db_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fleet_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!(
            "test_{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
    fn test_update_ai_summary() {
        let db_path = temp_db_path();
        let store = ReportStore::open_at(&db_path).unwrap();

        let report = make_test_report("2026-03-31");
        store.save_report(&report).unwrap();

        // Verify no summary initially
        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert!(loaded.ai_summary.is_none());

        store
            .update_ai_summary("2026-03-31", "Great day of coding!")
            .unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(loaded.ai_summary.as_deref(), Some("Great day of coding!"));
        assert!(loaded.ai_summary_generated_at.is_some());

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
    /// Sonnet 5: 1M input ($3) + 1M output ($15) + 1M 1h writes ($6) = $24.
    #[test]
    fn report_metrics_price_one_hour_cache_writes_at_2x() {
        let line = concat!(
            r#"{"type":"assistant","message":{"id":"msg_1","content":[],"model":"claude-sonnet-5","stop_reason":"end_turn","#,
            r#""usage":{"input_tokens":1000000,"output_tokens":1000000,"cache_creation_input_tokens":1000000,"#,
            r#""cache_read_input_tokens":0,"cache_creation":{"ephemeral_1h_input_tokens":1000000,"ephemeral_5m_input_tokens":0}}}}"#,
        );
        let m = extract_session_metrics(line);
        assert_eq!(m.cache_creation_tokens, 1_000_000);
        assert_eq!(m.cache_creation_1h_tokens, 1_000_000, "1h subset must persist");
        assert!(
            (m.cost_usd - 24.0).abs() < 1e-9,
            "expected $24.00 at the 1h rate, got ${}",
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

        let line1 = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"tool_use","name":"Edit","id":"tu_1","input":{}}],"usage":{"input_tokens":100,"output_tokens":50},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;
        let line2 = r#"{"type":"assistant","message":{"id":"msg_2","content":[{"type":"tool_use","name":"Bash","id":"tu_2","input":{}}],"usage":{"input_tokens":200,"output_tokens":80},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#;

        std::fs::write(&jsonl1_path, line1).unwrap();
        std::fs::write(&jsonl2_path, line2).unwrap();

        let s1 = crate::session::SessionInfo {
            id: "s1".to_string(),
            workspace_path: "/project-a".to_string(),
            workspace_name: "project-a".to_string(),
            ide_name: None,
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
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".to_string(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None, handoff: None, user_mark: None, title_override: None, last_read_ms: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
            remote_disconnect: None,
            mirror_write: None,
        };

        let s2 = crate::session::SessionInfo {
            id: "s2".to_string(),
            workspace_path: "/project-b".to_string(),
            workspace_name: "project-b".to_string(),
            ide_name: None,
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
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".to_string(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None, handoff: None, user_mark: None, title_override: None, last_read_ms: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
            remote_disconnect: None,
            mirror_write: None,
        };

        let sessions: Vec<&crate::session::SessionInfo> = vec![&s1, &s2];
        let report = generate_report_from_sessions("2026-03-31", "UTC", &sessions);

        assert_eq!(report.date, "2026-03-31");
        assert_eq!(report.metrics.total_sessions, 2);
        assert_eq!(report.metrics.total_subagents, 1);
        assert_eq!(report.metrics.projects.len(), 2);
        assert_eq!(report.metrics.total_output_tokens, 130); // 50 + 80
        assert_eq!(report.metrics.total_tool_calls, 2); // 1 Edit + 1 Bash
        assert_eq!(report.session_ids, vec!["s1", "s2"]);

        // Verify source breakdown
        assert_eq!(report.metrics.source_breakdown.get("claude-code"), Some(&2));

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

    #[test]
    fn test_parse_lessons_output() {
        let output = "LESSON: Never use git stash\nREASON: It can lose data\nWORKSPACE: my-proj\nSESSION: sess-42\n\nLESSON: Always write tests first\nREASON: User corrected TDD order\nWORKSPACE: my-proj\nSESSION: sess-42";
        let pairs: Vec<ConversationPair> = vec![];
        let lessons = parse_lessons(output, &pairs);
        assert_eq!(lessons.len(), 2);
        assert_eq!(lessons[0].content, "Never use git stash");
        assert_eq!(lessons[0].reason, "It can lose data");
        assert_eq!(lessons[0].workspace_name, "my-proj");
        assert_eq!(lessons[0].session_id, "sess-42");
        assert_eq!(lessons[1].content, "Always write tests first");
    }

    #[test]
    fn test_parse_lessons_none_output() {
        let pairs: Vec<ConversationPair> = vec![];
        let lessons = parse_lessons("NONE", &pairs);
        assert!(lessons.is_empty());
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

        assert!(store.get_report("2026-03-31").unwrap().unwrap().lessons.is_none());

        let lessons = vec![Lesson {
            content: "Use tests".to_string(),
            reason: "Bugs found in prod".to_string(),
            workspace_name: "proj".to_string(),
            session_id: "s1".to_string(),
        }];
        store.update_lessons("2026-03-31", &lessons).unwrap();

        let loaded = store.get_report("2026-03-31").unwrap().unwrap();
        assert_eq!(loaded.lessons.unwrap().len(), 1);
        assert!(loaded.lessons_generated_at.is_some());

        let _ = std::fs::remove_file(&db_path);
    }
}
