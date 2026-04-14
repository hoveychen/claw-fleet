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

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DailyMetrics {
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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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
#[serde(rename_all = "camelCase")]
pub struct DailyReportStats {
    pub date: String,
    pub total_tokens: u64,
    pub total_sessions: u32,
    pub total_tool_calls: u32,
    pub total_projects: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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
    /// Last assistant turn's "context window size" estimate
    /// (input + cache_creation + cache_read at the final turn). Kept for
    /// backward compatibility with the existing "input tokens" display.
    pub input_tokens: u64,
    /// Summed output tokens across all unique assistant turns.
    pub output_tokens: u64,
    /// Summed cache-creation tokens (for billing).
    pub cache_creation_tokens: u64,
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

        // Migrations: add lessons columns if they don't exist yet
        let _ = conn.execute_batch(
            "ALTER TABLE daily_reports ADD COLUMN lessons TEXT;
             ALTER TABLE daily_reports ADD COLUMN lessons_generated_at INTEGER;",
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
    let mut last_input: u64 = 0;
    let mut sum_cache_create: u64 = 0;
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

        total_output += output_tokens;
        sum_cache_create += cache_create;
        sum_cache_read += cache_read;
        sum_web_search += web_search;

        // Back-compat "input_tokens" field = last turn's full context window.
        let total_input = input + cache_create + cache_read;
        if total_input > 0 {
            last_input = total_input;
        }

        // Per-turn cost uses this turn's own model (falls back to the
        // most recently seen model if this line omits it).
        let turn_model = msg.get("model").and_then(|m| m.as_str());
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
        input_tokens: last_input,
        output_tokens: total_output,
        cache_creation_tokens: sum_cache_create,
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
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                });
            entry.input_tokens += sd.metrics.input_tokens;
            entry.output_tokens += sd.metrics.output_tokens;
            entry.cache_creation_tokens += sd.metrics.cache_creation_tokens;
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
        },
        ai_summary: None,
        ai_summary_generated_at: None,
        session_ids,
        lessons: None,
        lessons_generated_at: None,
    }
}

// ── AI summary generation ────────────────────────────────────────────────────

const AI_SUMMARY_TIMEOUT: Duration = Duration::from_secs(120);

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
         {lang_instruction}\n\
         \n\
         ---\n\
         {sections}",
    )
}

/// Generate AI summary for a daily report using `claude -p --model sonnet`.
/// Blocks for up to 120 seconds. Call from a background thread.
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
    provider.complete(&prompt, model, AI_SUMMARY_TIMEOUT)
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
    if let Some(home) = crate::session::real_home_dir() {
        let global = home.join(".claude").join("CLAUDE.md");
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

fn build_lessons_prompt(pairs: &[ConversationPair], locale: &str, existing_rules: &str) -> String {
    let lang_instruction = match locale {
        "zh" => "请用中文撰写输出。",
        _ => "Write the output in English.",
    };

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
        "Below are conversation turns between an AI coding assistant and a user. \
         Each turn shows what the AI said, followed by the user's reply.\n\n\
         Your task: identify cases where the user corrected the AI, rejected an approach, \
         pointed out a mistake, or repeated a requirement the AI ignored.\n\n\
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

    if all_pairs.is_empty() {
        log_debug("[daily_report] no conversation pairs found for lessons");
        return Some(vec![]);
    }

    // Collect workspace paths for deduplication against existing CLAUDE.md rules
    let workspace_paths: Vec<String> = sessions
        .iter()
        .filter(|si| !si.is_subagent)
        .map(|si| si.workspace_path.clone())
        .collect();
    let existing_rules = collect_existing_rules(&workspace_paths);

    let prompt = build_lessons_prompt(&all_pairs, locale, &existing_rules);

    let raw = match provider.complete(&prompt, model, LESSONS_TIMEOUT) {
        Some(r) => r,
        None => return None,
    };

    if raw.is_empty() || raw.eq_ignore_ascii_case("NONE") {
        return Some(vec![]);
    }

    Some(parse_lessons(&raw, &all_pairs))
}

/// Append a single lesson to `~/.claude/CLAUDE.md`.
pub fn append_lesson_to_claude_md(lesson: &Lesson) -> Result<(), String> {
    let path = crate::session::real_home_dir()
        .ok_or_else(|| "cannot determine home dir".to_string())?
        .join(".claude")
        .join("CLAUDE.md");

    let entry = format!(
        "\n\n# Lesson (from {}, session {})\n{}\n\n**Why:** {}\n",
        lesson.workspace_name, lesson.session_id, lesson.content, lesson.reason
    );

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open CLAUDE.md: {e}"))?;

    use std::io::Write;
    file.write_all(entry.as_bytes())
        .map_err(|e| format!("write CLAUDE.md: {e}"))?;

    Ok(())
}

// ── Session scanning for a specific date ────────────────────────────────────

/// Scan `~/.claude/projects/` for JSONL files whose creation date matches `date`
/// (YYYY-MM-DD) in the local timezone.  Unlike the normal session scanner, this
/// has no age limit and is suitable for backfill.
pub fn scan_sessions_for_date(date: &str) -> Vec<crate::session::SessionInfo> {
    use crate::session::decode_workspace_path_with_parts;

    let home = match crate::session::real_home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let projects_dir = home.join(".claude").join("projects");
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
        let workspace_path = decode_workspace_path_with_parts(&parts);
        let workspace_name = workspace_path
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(&workspace_path)
            .to_string();

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
        is_subagent,
        parent_session_id: None,
        agent_type: None,
        agent_description: None,
        slug,
        ai_title,
        status: SessionStatus::Idle,
        token_speed: 0.0,
        total_output_tokens: 0,
        last_message_preview: None,
        last_activity_ms: 0,
        created_at_ms: created_ms,
        jsonl_path,
        model: None,
        thinking_level: None,
        pid: None,
        pid_precise: false,
        last_skill: None,
        context_percent: None,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
    })
}

// ── Report scheduler ────────────────────────────────────────────────────────

/// Start the background report scheduler thread.
/// Checks every 30 minutes for missing reports and generates them.
pub fn start_report_scheduler(
    report_store: std::sync::Arc<std::sync::Mutex<ReportStore>>,
    locale: std::sync::Arc<std::sync::Mutex<String>>,
    llm_config: std::sync::Arc<std::sync::Mutex<crate::llm_provider::LlmConfig>>,
) {
    std::thread::Builder::new()
        .name("report-scheduler".into())
        .spawn(move || {
            // Short initial delay to let the app start, then generate immediately
            std::thread::sleep(Duration::from_secs(10));

            loop {
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
                std::thread::sleep(Duration::from_secs(10 * 60));
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

        // For today, always regenerate (new sessions keep arriving).
        // For past days, skip if report already exists.
        if days_ago > 0 && existing.is_some() {
            continue;
        }

        let sessions = scan_sessions_for_date(&date);
        if sessions.is_empty() {
            continue;
        }
        let session_refs: Vec<&crate::session::SessionInfo> = sessions.iter().collect();
        let tz = chrono::Local::now().format("%Z").to_string();
        let r = generate_report_from_sessions(&date, &tz, &session_refs);
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

        let provider = crate::llm_provider::resolve_provider(&llm_config.provider);
        let Some(provider) = provider else {
            log_debug(&format!(
                "[report-scheduler] unknown provider '{}', skipping AI",
                llm_config.provider
            ));
            break;
        };

        if report.ai_summary.is_none() {
            log_debug(&format!("[report-scheduler] generating AI summary for {date}..."));
            if let Some(summary) = generate_ai_summary(provider.as_ref(), &llm_config.standard_model, &report, locale) {
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
            if let Some(lessons) = generate_lessons(provider.as_ref(), &llm_config.standard_model, &report, locale) {
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
    fn make_test_report(date: &str) -> DailyReport {
        DailyReport {
            date: date.to_string(),
            timezone: "UTC".to_string(),
            generated_at: 1000000,
            metrics: DailyMetrics {
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
            },
            ai_summary: None,
            ai_summary_generated_at: None,
            session_ids: vec!["sess-1".to_string(), "sess-2".to_string()],
            lessons: None,
            lessons_generated_at: None,
        }
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

    #[test]
    fn test_extract_multiple_messages() {
        let lines = [
            r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"tool_use","name":"Bash","id":"tu_1","input":{}}],"usage":{"input_tokens":100,"output_tokens":30},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#,
            r#"{"type":"assistant","message":{"id":"msg_2","content":[{"type":"tool_use","name":"Edit","id":"tu_2","input":{}},{"type":"tool_use","name":"Bash","id":"tu_3","input":{}}],"usage":{"input_tokens":200,"output_tokens":60,"cache_creation_input_tokens":20},"model":"claude-sonnet-4-20250514","stop_reason":"end_turn"}}"#,
        ];
        let content = lines.join("\n");
        let m = extract_session_metrics(&content);

        // input_tokens: last message's 200 + 20 = 220
        assert_eq!(m.input_tokens, 220);
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
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: Some("fix-bug".to_string()),
            ai_title: None,
            status: crate::session::SessionStatus::Idle,
            token_speed: 0.0,
            total_output_tokens: 50,
            last_message_preview: Some("Fixed the bug".to_string()),
            last_activity_ms: 0,
            created_at_ms: 1743400000000, // some timestamp
            jsonl_path: jsonl1_path.to_string_lossy().to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".to_string(),
            last_outcome: None,
        };

        let s2 = crate::session::SessionInfo {
            id: "s2".to_string(),
            workspace_path: "/project-b".to_string(),
            workspace_name: "project-b".to_string(),
            ide_name: None,
            is_subagent: true,
            parent_session_id: Some("s1".to_string()),
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: Some("Add feature".to_string()),
            status: crate::session::SessionStatus::Idle,
            token_speed: 0.0,
            total_output_tokens: 80,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 1743400000000,
            jsonl_path: jsonl2_path.to_string_lossy().to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".to_string(),
            last_outcome: None,
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
