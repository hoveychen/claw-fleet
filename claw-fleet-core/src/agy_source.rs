//! Antigravity CLI (`agy`) agent source (Beta).
//!
//! `agy` is Google's closed-source Go CLI that replaced Gemini CLI for personal
//! (AI Pro / Ultra) subscriptions. What Fleet reads and drives:
//!
//! - **Session list** from `~/.gemini/antigravity-cli/conversation_summaries.db`
//!   (SQLite, written by agy in WAL mode). Read-only, never `immutable=1` (that
//!   reads torn pages while agy writes), and a 0-byte file is skipped without
//!   opening it: a reader's lock on a fresh rollback-journal file blocks agy's
//!   first write.
//! - **Message bodies and usage** from `conversations/<id>.db`, decoded by
//!   [`crate::agy_conv`] (see its docs for where every field number comes from
//!   and the caveat that no real non-empty conversation has been decoded yet).
//!   Conversations without readable steps fall back to agy's prompt history
//!   (`history.jsonl`) plus Fleet's own run records.
//! - **Launch / resume** through headless `agy -p … --output-format stream-json`
//!   (`--conversation <id>` to resume). agy mints the conversation id itself and
//!   prints it on the `init` event; there is no flag to pre-assign one.
//! - **Fleet run records** (`~/.fleet/agy-runs/<run_id>.{json,out,err}`): the
//!   stdout/stderr of every Fleet-started turn is kept. A launch that fails
//!   before agy creates a conversation — the eligibility check does exactly that,
//!   answering with a `result` whose `conversation_id` is empty — still gets a
//!   session row (`agy-run-<run_id>`) that carries the raw error.
//!
//! Out of scope: auto-resume and rate-limit handling (agy exposes no quota
//! surface Fleet can read).

use std::collections::HashMap;
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent_source::{AgentSource, ResumeSpec, SpawnSpec, WatchStrategy};
use crate::session::{SessionInfo, SessionStatus, TurnEnd};
use crate::session_launch::SpawnSessionResponse;

/// Config / `agent_source` / api name of this source.
pub const SOURCE_NAME: &str = "agy";
/// URI scheme of an agy session's `jsonl_path` (there is no jsonl file).
pub const URI_PREFIX: &str = "agy://";
/// Explicit model prefix that routes any id to agy (`agy:claude-opus-5-5-high`).
pub const MODEL_PREFIX: &str = "agy:";
/// Session-id prefix of a Fleet launch that never got a conversation id.
pub const RUN_ID_PREFIX: &str = "agy-run-";

/// How long a launch waits for agy to name its conversation (or fail) before
/// returning with the run's own id. The eligibility failure answers in ~5s.
const CONVERSATION_ID_TIMEOUT: Duration = Duration::from_secs(30);
/// Run records older than this are pruned on the next launch.
const RUN_RECORD_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Prompt text kept in a run record (for the detail view), in chars.
const RUN_PROMPT_CAP: usize = 4000;

// ── paths & availability ─────────────────────────────────────────────────────

/// `~/.gemini/antigravity-cli` — agy's data directory.
pub fn agy_home() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".gemini").join("antigravity-cli"))
}

/// `~/.fleet/agy-runs` — Fleet's own record of the turns it started.
fn runs_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("agy-runs"))
}

/// Locate the `agy` binary. Well-known install paths are checked with a stat
/// first (a GUI-launched app has a minimal PATH, and a `which` subprocess on
/// every 5s poll is waste); `which` is the fallback, cached for a minute.
pub fn find_agy_binary() -> Option<PathBuf> {
    static CACHE: Mutex<Option<(Instant, Option<PathBuf>)>> = Mutex::new(None);
    if let Ok(guard) = CACHE.lock() {
        if let Some((at, hit)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return hit.clone();
            }
        }
    }
    let found = probe_agy_binary();
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((Instant::now(), found.clone()));
    }
    found
}

fn probe_agy_binary() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin/agy"),
        PathBuf::from("/usr/local/bin/agy"),
    ];
    if let Some(home) = crate::session::real_home_dir() {
        candidates.push(home.join(".local").join("bin").join("agy"));
    }
    if let Some(hit) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(hit);
    }
    crate::process_util::which("agy").map(PathBuf::from)
}

/// Whether this source can be offered: the binary is installed, or agy has
/// already left conversations on disk to list.
pub fn is_available() -> bool {
    find_agy_binary().is_some()
        || agy_home().is_some_and(|h| h.join("conversation_summaries.db").is_file())
}

// ── model routing ────────────────────────────────────────────────────────────

/// Whether a model spec names agy: the explicit `agy:` prefix, or a bare
/// `gemini-*` id (Gemini models are only reachable through agy). agy also serves
/// `claude-*` and `gpt-oss-*` ids; those keep routing to Claude Code / Codex
/// unless written with the prefix.
pub fn is_agy_model(spec: &str) -> bool {
    let m = spec.trim();
    m.starts_with(MODEL_PREFIX) || m.to_ascii_lowercase().starts_with("gemini-")
}

/// The value agy's `--model` takes: the spec without Fleet's `agy:` prefix.
/// Blank → `None` (agy picks its own default).
pub fn model_arg(spec: Option<&str>) -> Option<String> {
    let m = spec?.trim();
    let m = m.strip_prefix(MODEL_PREFIX).unwrap_or(m).trim();
    (!m.is_empty()).then(|| m.to_string())
}

// ── conversation_summaries.db ────────────────────────────────────────────────

/// One row of agy's conversation index, as phase 1 uses it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SummaryRow {
    pub conversation_id: String,
    pub title: String,
    pub preview: String,
    pub step_count: i64,
    /// `last_modified_time`, ms since epoch; `None` for agy's zero time
    /// (`0001-01-01 …`), which is what a conversation that never ran a step has.
    pub last_modified_ms: Option<u64>,
    pub workspace_path: Option<String>,
    pub status: String,
    pub parent_conversation_id: String,
    pub killed: bool,
}

/// Read every row of `conversation_summaries`. A missing or 0-byte file is an
/// empty list, not an error (see the module docs for why 0 bytes is skipped).
pub fn read_summaries(db: &Path) -> Result<Vec<SummaryRow>, String> {
    match std::fs::metadata(db) {
        Ok(m) if m.len() == 0 => return Ok(Vec::new()),
        Ok(_) => {}
        Err(_) => return Ok(Vec::new()),
    }
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open {}: {e}", db.display()))?;
    let _ = conn.busy_timeout(Duration::from_millis(250));
    let mut stmt = conn
        .prepare(
            "SELECT conversation_id, title, preview, step_count, \
             CAST(last_modified_time AS TEXT), workspace_uris, status, \
             parent_conversation_id, killed \
             FROM conversation_summaries",
        )
        .map_err(|e| format!("query {}: {e}", db.display()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SummaryRow {
                conversation_id: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                preview: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                step_count: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                last_modified_ms: r
                    .get::<_, Option<String>>(4)?
                    .as_deref()
                    .and_then(parse_agy_time),
                workspace_path: r
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .and_then(first_workspace_path),
                status: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                parent_conversation_id: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                killed: r.get::<_, Option<i64>>(8).ok().flatten().unwrap_or(0) != 0,
            })
        })
        .map_err(|e| format!("read {}: {e}", db.display()))?;
    Ok(rows
        .filter_map(Result::ok)
        .filter(|r| !r.conversation_id.is_empty())
        .collect())
}

/// Parse agy's `datetime` text (`2026-11-19 21:14:05.123+00:00`). Go's zero
/// time and anything before 2000 read as "unknown".
pub fn parse_agy_time(text: &str) -> Option<u64> {
    let t = text.trim();
    let parsed = chrono::DateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f%:z")
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(t))
        .ok()?;
    let ms = parsed.timestamp_millis();
    // 2000-01-01T00:00:00Z
    (ms >= 946_684_800_000).then_some(ms as u64)
}

/// The first `file://` entry of agy's `workspace_uris` JSON array, as a path.
pub fn first_workspace_path(uris: &str) -> Option<String> {
    let list: Vec<String> = serde_json::from_str(uris).ok()?;
    list.iter().find_map(|u| {
        let rest = u.strip_prefix("file://")?;
        let path = percent_decode(rest);
        (!path.is_empty()).then_some(path)
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mtime_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn created_ms(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let t = meta.created().or_else(|_| meta.modified()).ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── conversation databases ───────────────────────────────────────────────────

/// `(len, mtime_ms)` of a file, `(0, 0)` when absent.
fn stat_pair(path: &Path) -> (u64, u64) {
    std::fs::metadata(path)
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            (m.len(), mtime)
        })
        .unwrap_or((0, 0))
}

fn wal_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// Change fingerprint of a conversation database: its own and its WAL's stat.
/// agy's writes land in the WAL first, so the main file alone would miss them.
fn db_fingerprint(db: &Path) -> [(u64, u64); 2] {
    [stat_pair(db), stat_pair(&wal_path(db))]
}

/// Decoded conversations keyed by database path, reused while the database's
/// fingerprint is unchanged — the scan polls every 5s and must not re-decode
/// every conversation on each beat.
type ConvCache = HashMap<PathBuf, ([(u64, u64); 2], std::sync::Arc<crate::agy_conv::Conversation>)>;
static CONV_CACHE: Mutex<Option<ConvCache>> = Mutex::new(None);

/// Read (or reuse) one conversation's decoded steps and usage. A decode error
/// is logged and reads as an empty conversation.
pub fn conversation(db: &Path, conv: &str) -> std::sync::Arc<crate::agy_conv::Conversation> {
    let fp = db_fingerprint(db);
    if let Ok(guard) = CONV_CACHE.lock() {
        if let Some((cached_fp, c)) = guard.as_ref().and_then(|m| m.get(db)) {
            if *cached_fp == fp {
                return c.clone();
            }
        }
    }
    let decoded = crate::agy_conv::read_conversation(db, conv).unwrap_or_else(|e| {
        crate::log_debug(&format!("agy: decode {}: {e}", db.display()));
        crate::agy_conv::Conversation::default()
    });
    if decoded.steps_skipped > 0 || decoded.generations.drifted > 0 {
        crate::log_debug(&format!(
            "agy: {conv}: {} step(s) in an unsupported shape, {} usage row(s) failed the self-check",
            decoded.steps_skipped, decoded.generations.drifted
        ));
    }
    let arc = std::sync::Arc::new(decoded);
    if let Ok(mut guard) = CONV_CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(db.to_path_buf(), (fp, arc.clone()));
    }
    arc
}

/// `conversations/<id>.db` under agy's home.
fn conversation_db(conv: &str) -> Option<PathBuf> {
    agy_home().map(|h| h.join("conversations").join(format!("{conv}.db")))
}

// ── history.jsonl ────────────────────────────────────────────────────────────

/// One prompt from agy's `history.jsonl` (what the user typed, by
/// conversation). Real lines look like
/// `{"display":"hi","timestamp":1795…,"workspace":"/p","conversationId":"…"}`;
/// slash commands carry `"type":"slash_command"` and are not prompts.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub conversation_id: String,
    pub display: String,
    pub timestamp_ms: u64,
}

/// History lines kept from the tail of the file.
const HISTORY_TAIL_BYTES: u64 = 4 * 1024 * 1024;

pub fn read_history(path: &Path) -> Vec<HistoryEntry> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(HISTORY_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        // A tail read starts mid-line; that partial line fails to parse.
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("type").and_then(Value::as_str).is_none_or(|t| t != "slash_command"))
        .filter_map(|v| {
            Some(HistoryEntry {
                conversation_id: v.get("conversationId")?.as_str()?.trim().to_string(),
                display: v.get("display")?.as_str()?.to_string(),
                timestamp_ms: v.get("timestamp").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .filter(|e| !e.conversation_id.is_empty() && !e.display.trim().is_empty())
        .collect()
}

fn history_path() -> Option<PathBuf> {
    agy_home().map(|h| h.join("history.jsonl"))
}

// ── Fleet run records ────────────────────────────────────────────────────────

/// The metadata half of a run record (`<run_id>.json`). The stdout sink
/// (`<run_id>.out`, agy's stream-json) and stderr (`<run_id>.err`) sit beside it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunMeta {
    pub run_id: String,
    /// `"new"` or `"resume"`.
    pub kind: String,
    pub workspace: String,
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub started_ms: u64,
    pub pid: Option<u32>,
    /// The conversation being resumed, or the one agy named for a new turn.
    pub conversation_id: Option<String>,
    pub ended_ms: Option<u64>,
    pub exit_code: Option<i32>,
    /// The Fleet entrypoint the turn was started under (`claw-fleet-newsession`,
    /// …). Copied onto the row so clients' Fleet-owned filters (the task page,
    /// the phone's only list) include agy sessions Fleet started. `None` for a
    /// resume of a conversation the user opened in agy by hand.
    #[serde(default)]
    pub entrypoint: Option<String>,
}

/// What agy's stream-json said about a turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunOutcome {
    /// From `init` (top level) or a non-empty `result.conversation_id`.
    pub conversation_id: Option<String>,
    /// `result.status` (`"ERROR"`, …) — `None` while the turn has not ended.
    pub status: Option<String>,
    pub error: Option<String>,
    pub response: Option<String>,
}

impl RunOutcome {
    pub fn failed(&self) -> bool {
        self.error.as_deref().is_some_and(|e| !e.is_empty())
            || self
                .status
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("error"))
    }
}

/// Fold agy's stream-json stdout into a [`RunOutcome`]. Unknown events and
/// non-JSON lines are ignored, so a format addition does not break the fold.
pub fn parse_run_output(text: &str) -> RunOutcome {
    let mut out = RunOutcome::default();
    let non_empty = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let event = v
            .get("event")
            .or_else(|| v.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if out.conversation_id.is_none() {
            out.conversation_id = non_empty(v.get("conversation_id"));
        }
        if event == "result" {
            let r = v.get("result").unwrap_or(&v);
            if let Some(id) = non_empty(r.get("conversation_id")) {
                out.conversation_id = Some(id);
            }
            out.status = non_empty(r.get("status")).or(Some(String::new()));
            out.error = non_empty(r.get("error"));
            out.response = non_empty(r.get("response"));
        }
    }
    out
}

/// One run record, loaded.
#[derive(Clone, Debug, Default)]
pub struct RunRecord {
    pub meta: RunMeta,
    pub outcome: RunOutcome,
    /// Last non-empty stderr line — the error of a turn that died without a
    /// `result` event.
    pub stderr_tail: Option<String>,
}

impl RunRecord {
    /// The conversation this run belongs to, when agy named one.
    pub fn conversation_id(&self) -> Option<&str> {
        self.outcome
            .conversation_id
            .as_deref()
            .or(self.meta.conversation_id.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// The session key this run is listed under.
    pub fn session_key(&self) -> String {
        match self.conversation_id() {
            Some(id) => id.to_string(),
            None => format!("{RUN_ID_PREFIX}{}", self.meta.run_id),
        }
    }

    /// Raw error text of a failed turn, verbatim from agy.
    pub fn error_text(&self) -> Option<String> {
        if let Some(e) = self.outcome.error.as_deref().filter(|e| !e.is_empty()) {
            return Some(e.to_string());
        }
        let ended_badly = self.outcome.failed()
            || (self.outcome.status.is_none()
                && self.meta.ended_ms.is_some()
                && self.meta.exit_code != Some(0));
        if ended_badly {
            return Some(self.stderr_tail.clone().unwrap_or_else(|| {
                format!("agy exited with code {:?}", self.meta.exit_code)
            }));
        }
        None
    }

    fn ended(&self) -> bool {
        self.outcome.status.is_some() || self.meta.ended_ms.is_some()
    }
}

fn last_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(|l| l.strip_prefix("error: ").unwrap_or(l).to_string())
}

/// Load every run record under `dir`, oldest first.
pub fn load_runs(dir: &Path) -> Vec<RunRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut runs: Vec<RunRecord> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let meta: RunMeta = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            let out = std::fs::read_to_string(p.with_extension("out")).unwrap_or_default();
            let err = std::fs::read_to_string(p.with_extension("err")).unwrap_or_default();
            Some(RunRecord {
                meta,
                outcome: parse_run_output(&out),
                stderr_tail: last_non_empty_line(&err),
            })
        })
        .collect();
    runs.sort_by_key(|r| r.meta.started_ms);
    runs
}

fn write_meta(dir: &Path, meta: &RunMeta) -> Result<(), String> {
    let path = dir.join(format!("{}.json", meta.run_id));
    let tmp = dir.join(format!("{}.json.tmp", meta.run_id));
    let body = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))
}

/// Serialises read-modify-write of run metadata between the launch thread and
/// the reaper thread of the same process.
static META_LOCK: Mutex<()> = Mutex::new(());

fn update_meta(dir: &Path, run_id: &str, edit: impl FnOnce(&mut RunMeta)) {
    let _g = META_LOCK.lock();
    let path = dir.join(format!("{run_id}.json"));
    let Some(mut meta) = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<RunMeta>(&b).ok())
    else {
        return;
    };
    edit(&mut meta);
    if let Err(e) = write_meta(dir, &meta) {
        crate::log_debug(&format!("agy: update run meta {run_id}: {e}"));
    }
}

fn prune_runs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = SystemTime::now()
        .checked_sub(RUN_RECORD_TTL)
        .unwrap_or(UNIX_EPOCH);
    for p in entries.filter_map(Result::ok).map(|e| e.path()) {
        let old = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < cutoff);
        if old {
            let _ = std::fs::remove_file(&p);
        }
    }
}

// ── scan ─────────────────────────────────────────────────────────────────────

/// Build the session list from agy's index plus Fleet's run records.
///
/// `conversations_dir` supplies a fallback activity time (the per-conversation
/// `.db` mtime) because agy leaves `last_modified_time` at Go's zero time until a
/// step lands. `is_alive` is injectable so tests decide which pids are running.
pub fn build_sessions(
    summaries: &[SummaryRow],
    conversations_dir: Option<&Path>,
    runs: &[RunRecord],
    is_alive: impl Fn(u32) -> bool,
) -> Vec<SessionInfo> {
    let mut by_key: HashMap<String, Vec<&RunRecord>> = HashMap::new();
    for r in runs {
        by_key.entry(r.session_key()).or_default().push(r);
    }

    let mut sessions: Vec<SessionInfo> = Vec::new();
    for row in summaries {
        let db = conversations_dir.map(|d| d.join(format!("{}.db", row.conversation_id)));
        let file_mtime = db.as_deref().and_then(|p| {
            let wal = p.with_extension("db-wal");
            mtime_ms(p).max(mtime_ms(&wal))
        });
        let mut s = SessionInfo {
            id: row.conversation_id.clone(),
            workspace_path: row.workspace_path.clone().unwrap_or_default(),
            ai_title: Some(row.title.trim().to_string()).filter(|t| !t.is_empty()),
            last_message_preview: Some(row.preview.trim().to_string()).filter(|t| !t.is_empty()),
            last_activity_ms: row.last_modified_ms.or(file_mtime).unwrap_or(0),
            created_at_ms: db.as_deref().and_then(created_ms).unwrap_or(0),
            parent_session_id: Some(row.parent_conversation_id.clone())
                .filter(|p| !p.is_empty()),
            status: if row.status.contains("RUNNING") && !row.killed {
                SessionStatus::Active
            } else {
                SessionStatus::Idle
            },
            ..Default::default()
        };
        if let Some(db) = db.as_deref() {
            overlay_conversation(&mut s, &conversation(db, &row.conversation_id));
        }
        if let Some(rs) = by_key.remove(&row.conversation_id) {
            overlay_runs(&mut s, &rs, &is_alive);
        }
        sessions.push(s);
    }

    // Runs whose conversation is not (yet) in the index: a launch agy refused
    // before creating a conversation, or one whose row has not been flushed.
    for (key, rs) in by_key {
        let first = rs[0];
        let mut s = SessionInfo {
            id: key,
            workspace_path: first.meta.workspace.clone(),
            created_at_ms: first.meta.started_ms,
            last_activity_ms: first.meta.started_ms,
            ..Default::default()
        };
        overlay_runs(&mut s, &rs, &is_alive);
        sessions.push(s);
    }

    for s in &mut sessions {
        s.agent_source = SOURCE_NAME.to_string();
        s.jsonl_path = format!("{URI_PREFIX}{}", s.id);
        if s.workspace_name.is_empty() && !s.workspace_path.is_empty() {
            s.workspace_name = crate::session::workspace_name(&s.workspace_path);
        }
        if s.created_at_ms == 0 {
            s.created_at_ms = s.last_activity_ms;
        }
    }
    sessions.sort_by(|a, b| b.last_activity_ms.cmp(&a.last_activity_ms));
    sessions
}

/// Apply what the decoded conversation knows: token totals, cost, the model of
/// the latest generation, and the last decoded message as the preview.
fn overlay_conversation(s: &mut SessionInfo, conv: &crate::agy_conv::Conversation) {
    let (input, output) = conv.generations.totals();
    s.total_input_tokens = input;
    s.total_output_tokens = output;
    s.reasoning_output_tokens = conv.generations.rows.iter().map(|g| g.thinking_tokens).sum();
    s.total_cost_usd = conv
        .generations
        .rows
        .iter()
        .filter_map(generation_cost_usd)
        .sum();
    s.agent_total_cost_usd = s.total_cost_usd;
    if let Some(m) = conv.generations.last_model() {
        s.model = Some(m.to_string());
    }
    if let Some(ms) = conv.generations.rows.iter().map(|g| g.timestamp_ms).max() {
        s.last_activity_ms = s.last_activity_ms.max(ms);
    }
    if let Some(preview) = conv.messages.iter().rev().find_map(message_preview) {
        s.last_message_preview = Some(preview);
    }
}

/// Plain text of a decoded message (user string content or assistant text
/// blocks), for the list preview.
fn message_preview(m: &Value) -> Option<String> {
    let content = m.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty()).then(|| t.chars().take(300).collect())
}

/// USD cost of one generation, `None` when its model has no known price
/// (unpriced is not free — callers must not fold it in as $0 silently).
pub fn generation_cost_usd(g: &crate::agy_conv::Generation) -> Option<f64> {
    let model = g.model.as_deref()?;
    crate::model_cost::agy_generation_cost_usd(
        model,
        g.timestamp_ms,
        g.input_tokens,
        g.output_tokens,
        g.cache_write_tokens,
        g.cache_read_tokens,
    )
}

/// Apply what Fleet's own runs know to a session row: liveness, model, and the
/// latest turn's outcome — its raw error verbatim on failure.
fn overlay_runs(s: &mut SessionInfo, runs: &[&RunRecord], is_alive: &impl Fn(u32) -> bool) {
    let Some(last) = runs.iter().max_by_key(|r| r.meta.started_ms) else {
        return;
    };
    s.fleet_spawned = true;
    if s.entrypoint.is_none() {
        s.entrypoint = runs
            .iter()
            .min_by_key(|r| r.meta.started_ms)
            .and_then(|r| r.meta.entrypoint.clone());
    }
    if s.model.is_none() {
        s.model = runs.iter().rev().find_map(|r| r.meta.model.clone());
    }
    if s.effort.is_none() {
        s.effort = runs.iter().rev().find_map(|r| r.meta.effort.clone());
    }
    let activity = last.meta.ended_ms.unwrap_or(last.meta.started_ms);
    s.last_activity_ms = s.last_activity_ms.max(activity);
    if s.workspace_path.is_empty() {
        s.workspace_path = last.meta.workspace.clone();
    }
    if s.ai_title.is_none() {
        s.ai_title = Some(title_from_prompt(&last.meta.prompt)).filter(|t| !t.is_empty());
    }

    let running = !last.ended() && last.meta.pid.is_some_and(is_alive);
    if running {
        s.pid = last.meta.pid;
        s.pid_precise = true;
        s.proc_alive = true;
        s.status = SessionStatus::Active;
        return;
    }
    if let Some(err) = last.error_text() {
        s.status = SessionStatus::Idle;
        s.last_turn_end = Some(TurnEnd::Failed);
        s.last_message_preview = Some(format!("agy: {err}"));
    } else if last.ended() {
        s.last_turn_end = Some(TurnEnd::Clean);
        if let Some(resp) = last.outcome.response.as_deref() {
            s.last_message_preview = Some(resp.chars().take(300).collect());
        }
        if !matches!(s.status, SessionStatus::Active) {
            s.status = SessionStatus::WaitingInput;
        }
    } else {
        // Not ended and the pid is gone: the turn died without a word.
        s.status = SessionStatus::Idle;
        s.last_turn_end = Some(TurnEnd::MidTurn);
    }
}

fn title_from_prompt(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().take(80).collect()
}

// ── detail messages ──────────────────────────────────────────────────────────

fn iso_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn run_result_message(r: &RunRecord) -> Option<Value> {
    let text = match r.error_text() {
        Some(err) => format!("agy error: {err}"),
        None => r.outcome.response.clone()?,
    };
    let ts = iso_ms(r.meta.ended_ms.unwrap_or(r.meta.started_ms));
    Some(json!({
        "type": "assistant",
        "uuid": format!("agy-run:{}:result", r.meta.run_id),
        "timestamp": ts,
        "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] },
    }))
}

/// The detail transcript of an agy session, Claude-shaped so every client
/// renders it as-is.
///
/// - With decoded steps (`conv`), they are the transcript; Fleet's run records
///   only add the raw error of a turn agy failed (those never become steps).
/// - Without steps — a conversation agy refused, or one whose database is
///   empty or in a shape the decoder skips — the transcript is rebuilt from
///   agy's prompt history plus Fleet's runs (prompt and raw outcome), so the
///   user still sees what was asked and what came back.
pub fn detail_messages(
    key: &str,
    conv: Option<&crate::agy_conv::Conversation>,
    history: &[HistoryEntry],
    runs: &[RunRecord],
) -> Vec<Value> {
    let runs: Vec<&RunRecord> = runs.iter().filter(|r| r.session_key() == key).collect();
    if let Some(c) = conv.filter(|c| !c.messages.is_empty()) {
        let mut out = c.messages.clone();
        out.extend(
            runs.iter()
                .filter(|r| r.error_text().is_some())
                .filter_map(|r| run_result_message(r)),
        );
        return out;
    }

    // (timestamp, message) pairs, merged by time.
    let mut items: Vec<(u64, Value)> = Vec::new();
    let prompts: Vec<&HistoryEntry> = history.iter().filter(|h| h.conversation_id == key).collect();
    for h in &prompts {
        items.push((
            h.timestamp_ms,
            json!({
                "type": "user",
                "uuid": format!("agy:{key}:history:{}", h.timestamp_ms),
                "timestamp": iso_ms(h.timestamp_ms),
                "message": { "role": "user", "content": h.display },
            }),
        ));
    }
    for r in &runs {
        // agy may log a Fleet-sent prompt to its history too; show it once.
        let in_history = prompts.iter().any(|h| {
            h.display.trim() == r.meta.prompt.trim()
                && h.timestamp_ms.abs_diff(r.meta.started_ms) < 120_000
        });
        if !r.meta.prompt.is_empty() && !in_history {
            items.push((
                r.meta.started_ms,
                json!({
                    "type": "user",
                    "uuid": format!("agy-run:{}:prompt", r.meta.run_id),
                    "timestamp": iso_ms(r.meta.started_ms),
                    "message": { "role": "user", "content": r.meta.prompt },
                }),
            ));
        }
        if let Some(m) = run_result_message(r) {
            items.push((r.meta.ended_ms.unwrap_or(r.meta.started_ms) + 1, m));
        }
    }
    items.sort_by_key(|(t, _)| *t);
    let mut out: Vec<Value> = items.into_iter().map(|(_, m)| m).collect();
    if conv.is_some_and(|c| c.steps_skipped > 0) {
        out.insert(
            0,
            json!({
                "type": "assistant",
                "uuid": format!("agy:{key}:unsupported"),
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "text",
                        "text": "这个 agy 会话的步骤存储格式 Fleet 还读不懂，下面只列出提示词和 Fleet 发起轮次的结果。\n\n\
                                 This agy conversation stores its steps in a format Fleet cannot \
                                 decode yet; only the prompts and the outcome of Fleet-started turns \
                                 are shown.",
                    }],
                },
            }),
        );
    }
    out
}

/// Change fingerprint of everything the detail transcript is built from: the
/// conversation database (+WAL), agy's prompt history and Fleet's run records
/// for the session. Never 0 — 0 is the "start from the beginning" offset
/// clients seed poll-type sources with.
fn detail_fingerprint(key: &str, db: Option<&Path>, history: Option<&Path>, runs: &[RunRecord]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(db) = db {
        db_fingerprint(db).hash(&mut h);
    }
    if let Some(p) = history {
        stat_pair(p).hash(&mut h);
    }
    for r in runs.iter().filter(|r| r.session_key() == key) {
        r.meta.run_id.hash(&mut h);
        r.meta.ended_ms.hash(&mut h);
        r.outcome.status.hash(&mut h);
        r.outcome.conversation_id.hash(&mut h);
    }
    h.finish() | 1
}

/// Tail window re-sent when the fingerprint changes; clients dedup by uuid.
const TAIL_WINDOW: usize = 60;

// ── launch ───────────────────────────────────────────────────────────────────

/// Build agy's headless argv. The prompt is `-p`'s value (Go's flag parser
/// takes the next argument verbatim, so a prompt starting with `-` is safe).
/// `--dangerously-skip-permissions` is deliberately NOT passed: phase 1 has no
/// `fleet guard` hook on agy, so skipping agy's own permission layer would
/// leave its tool calls with no audit at all.
pub fn build_args(
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    conversation: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
    ];
    if let Some(m) = model_arg(model) {
        args.push("--model".to_string());
        args.push(m);
    }
    if let Some(e) = effort.map(str::trim).filter(|e| !e.is_empty()) {
        args.push("--effort".to_string());
        args.push(e.to_string());
    }
    if let Some(c) = conversation.map(str::trim).filter(|c| !c.is_empty()) {
        args.push("--conversation".to_string());
        args.push(c.to_string());
    }
    args
}

struct Launch<'a> {
    kind: &'static str,
    workspace: &'a str,
    prompt: &'a str,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    conversation: Option<&'a str>,
    entrypoint: Option<&'a str>,
}

/// Start one headless agy turn. Returns the pid and the session key the turn
/// is listed under, plus the reaper's join point via `on_exit`.
fn launch(
    l: Launch<'_>,
    on_exit: Box<dyn FnOnce(bool) + Send>,
) -> Result<SpawnSessionResponse, String> {
    let prompt = l.prompt.trim();
    if prompt.is_empty() {
        return Err("prompt is required".to_string());
    }
    let workspace = crate::session_launch::normalize_workspace_path(l.workspace)?;
    if !Path::new(&workspace).is_dir() {
        return Err(format!("Workspace directory not found: {workspace}"));
    }
    let agy = find_agy_binary()
        .ok_or_else(|| "Antigravity CLI (`agy`) not found on this machine".to_string())?;
    let dir = runs_dir().ok_or_else(|| "no fleet dir".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    prune_runs(&dir);

    let run_id = uuid::Uuid::new_v4().to_string();
    let mut meta = RunMeta {
        run_id: run_id.clone(),
        kind: l.kind.to_string(),
        workspace: workspace.clone(),
        prompt: prompt.chars().take(RUN_PROMPT_CAP).collect(),
        model: l.model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_string),
        effort: l.effort.map(str::trim).filter(|e| !e.is_empty()).map(str::to_string),
        started_ms: now_ms(),
        pid: None,
        conversation_id: l.conversation.map(str::to_string),
        ended_ms: None,
        exit_code: None,
        entrypoint: l.entrypoint.map(str::to_string),
    };
    write_meta(&dir, &meta)?;
    let out_path = dir.join(format!("{run_id}.out"));
    let err_path = dir.join(format!("{run_id}.err"));
    let stdout = std::fs::File::create(&out_path)
        .map_err(|e| format!("create {}: {e}", out_path.display()))?;
    let stderr = std::fs::File::create(&err_path)
        .map_err(|e| format!("create {}: {e}", err_path.display()))?;

    let args = build_args(prompt, l.model, l.effort, l.conversation);
    crate::log_debug(&format!(
        "agy launch: {} {} (cwd={workspace}, run={run_id}, model={:?}, conversation={:?})",
        agy.display(),
        l.kind,
        l.model,
        l.conversation
    ));
    let mut cmd = crate::process_util::command(&agy);
    cmd.args(&args)
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        // Files, not pipes: the turn must survive the Fleet app quitting.
        .stdout(std::process::Stdio::from(stdout))
        .stderr(std::process::Stdio::from(stderr));
    crate::process_util::detach_process_group(&mut cmd);
    apply_launch_env(&mut cmd, &run_id, l.conversation);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("spawn agy failed: {e}");
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&err_path) {
                let _ = writeln!(f, "{msg}");
            }
            meta.ended_ms = Some(now_ms());
            meta.exit_code = Some(-1);
            let _ = write_meta(&dir, &meta);
            return Err(msg);
        }
    };
    let pid = child.id();
    update_meta(&dir, &run_id, |m| m.pid = Some(pid));

    let reaper_dir = dir.clone();
    let reaper_run = run_id.clone();
    let reaper_out = out_path.clone();
    std::thread::spawn(move || {
        let status = child.wait();
        let code = status.as_ref().ok().and_then(|s| s.code());
        let outcome = std::fs::read_to_string(&reaper_out)
            .map(|t| parse_run_output(&t))
            .unwrap_or_default();
        update_meta(&reaper_dir, &reaper_run, |m| {
            m.ended_ms = Some(now_ms());
            m.exit_code = code.or(Some(-1));
            if m.conversation_id.is_none() {
                m.conversation_id = outcome.conversation_id.clone();
            }
        });
        on_exit(code == Some(0) && !outcome.failed());
    });

    // Wait for agy to name the conversation, or to end the turn without one.
    let deadline = Instant::now() + CONVERSATION_ID_TIMEOUT;
    let mut key = l.conversation.map(str::to_string);
    while key.is_none() && Instant::now() < deadline {
        let outcome = read_outcome(&out_path);
        if let Some(id) = outcome.conversation_id {
            update_meta(&dir, &run_id, |m| m.conversation_id = Some(id.clone()));
            key = Some(id);
            break;
        }
        if outcome.status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let session_id = key.unwrap_or_else(|| format!("{RUN_ID_PREFIX}{run_id}"));
    crate::launch_spec::record(&session_id, l.model, l.effort);
    Ok(SpawnSessionResponse {
        pid,
        session_id: Some(session_id),
    })
}

/// Identity env for a Fleet-launched agy process. Inherited session ids are
/// cleared first (the spawner may itself be a Fleet session, and its id would
/// otherwise win in [`crate::codex_launch::resolve_fleet_session_id_from_env`]
/// inside agy's MCP children). The run id is always set — the conversation id
/// of a new turn does not exist yet — and a resume also carries the
/// conversation as `FLEET_SESSION_ID`.
fn apply_launch_env(cmd: &mut std::process::Command, run_id: &str, conversation: Option<&str>) {
    cmd.env_remove("FLEET_SESSION_ID");
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env(crate::agy_hooks::RUN_ENV, run_id);
    cmd.env("FLEET_AGENT_SOURCE", SOURCE_NAME);
    if let Some(c) = conversation.map(str::trim).filter(|c| !c.is_empty()) {
        cmd.env("FLEET_SESSION_ID", c);
    }
}

/// Every Fleet run record on this machine, oldest first.
pub fn runs() -> Option<Vec<RunRecord>> {
    runs_dir().map(|d| load_runs(&d))
}

/// The session a Fleet-launched agy process belongs to, from the run id Fleet
/// put in its env: the conversation once agy has named it, the run's own key
/// before that. `None` for an unknown run.
pub fn session_id_for_run(run_id: &str) -> Option<String> {
    if run_id.is_empty() || run_id.contains(['/', '\\']) || run_id.contains("..") {
        return None;
    }
    let dir = runs_dir()?;
    let meta: RunMeta =
        serde_json::from_slice(&std::fs::read(dir.join(format!("{run_id}.json"))).ok()?).ok()?;
    let out = std::fs::read_to_string(dir.join(format!("{run_id}.out"))).unwrap_or_default();
    let record = RunRecord {
        meta,
        outcome: parse_run_output(&out),
        stderr_tail: None,
    };
    Some(record.session_key())
}

/// Close out run records whose process is gone but whose end was never
/// written. The reaper thread lives in the process that launched the turn, so a
/// turn started by a short-lived `fleet` CLI loses its reaper the moment the CLI
/// exits. The end time is the output file's mtime (the last thing agy wrote);
/// the exit code is inferred from the stream — 0 for a clean `result`, 1 for a
/// failed one, -1 when agy died without one — since the real code is
/// unrecoverable once nobody waited on the process.
fn settle_orphan_runs(dir: &Path, runs: &mut [RunRecord], is_alive: &impl Fn(u32) -> bool) {
    for r in runs.iter_mut() {
        let Some(pid) = r.meta.pid else { continue };
        if r.meta.ended_ms.is_some() || is_alive(pid) {
            continue;
        }
        let ended = mtime_ms(&dir.join(format!("{}.out", r.meta.run_id))).unwrap_or_else(now_ms);
        let code = match &r.outcome {
            o if o.status.is_none() => -1,
            o if o.failed() => 1,
            _ => 0,
        };
        let conv = r.outcome.conversation_id.clone();
        update_meta(dir, &r.meta.run_id, |m| {
            m.ended_ms.get_or_insert(ended);
            m.exit_code.get_or_insert(code);
            if m.conversation_id.is_none() {
                m.conversation_id = conv.clone();
            }
        });
        r.meta.ended_ms = Some(ended);
        r.meta.exit_code = Some(code);
        if r.meta.conversation_id.is_none() {
            r.meta.conversation_id = conv;
        }
    }
}

fn read_outcome(path: &Path) -> RunOutcome {
    let Ok(f) = std::fs::File::open(path) else {
        return RunOutcome::default();
    };
    // Only complete lines: a half-written trailing event is ignored.
    let mut text = String::new();
    for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
        text.push_str(&line);
        text.push('\n');
    }
    parse_run_output(&text)
}

// ── AgentSource ──────────────────────────────────────────────────────────────

pub struct AgySource;

impl AgySource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AgySource {
    fn default() -> Self {
        Self::new()
    }
}

fn key_of(path: &str) -> &str {
    path.strip_prefix(URI_PREFIX).unwrap_or(path)
}

impl AgentSource for AgySource {
    fn name(&self) -> &'static str {
        SOURCE_NAME
    }

    fn uri_prefix(&self) -> &'static str {
        URI_PREFIX
    }

    fn is_available(&self) -> bool {
        is_available()
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        let home = agy_home();
        let summaries = match home.as_ref() {
            Some(h) => read_summaries(&h.join("conversation_summaries.db")).unwrap_or_else(|e| {
                crate::log_debug(&format!("agy scan: {e}"));
                Vec::new()
            }),
            None => Vec::new(),
        };
        let runs = match runs_dir() {
            Some(d) => {
                let mut runs = load_runs(&d);
                settle_orphan_runs(&d, &mut runs, &crate::session::is_process_alive);
                runs
            }
            None => Vec::new(),
        };
        let conversations = home.map(|h| h.join("conversations"));
        build_sessions(
            &summaries,
            conversations.as_deref(),
            &runs,
            crate::session::is_process_alive,
        )
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let key = key_of(path);
        let runs = runs_dir().map(|d| load_runs(&d)).unwrap_or_default();
        let conv = (!key.starts_with(RUN_ID_PREFIX))
            .then(|| conversation_db(key))
            .flatten()
            .map(|db| conversation(&db, key));
        let history = history_path().map(|p| read_history(&p)).unwrap_or_default();
        Ok(detail_messages(key, conv.as_deref(), &history, &runs))
    }

    /// The transcript is rebuilt from several stores, so the offset is a
    /// change fingerprint rather than a byte position: unchanged → nothing;
    /// changed → the last [`TAIL_WINDOW`] messages, which clients dedup by
    /// their stable uuids (the Codex follow model).
    fn tail_incremental(&self, path: &str, offset: u64) -> Result<(Vec<Value>, u64), String> {
        let key = key_of(path);
        let runs = runs_dir().map(|d| load_runs(&d)).unwrap_or_default();
        let db = (!key.starts_with(RUN_ID_PREFIX)).then(|| conversation_db(key)).flatten();
        let fp = detail_fingerprint(key, db.as_deref(), history_path().as_deref(), &runs);
        if fp == offset {
            return Ok((Vec::new(), fp));
        }
        let all = self.get_messages(path)?;
        let start = all.len().saturating_sub(TAIL_WINDOW);
        Ok((all[start..].to_vec(), fp))
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Poll(Duration::from_secs(5))
    }

    fn resolve_file_path(&self, _path: &str) -> Option<PathBuf> {
        None
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        crate::session::kill_pid_impl(pid)
    }

    fn spawn(&self, spec: &SpawnSpec) -> Result<SpawnSessionResponse, String> {
        launch(
            Launch {
                kind: "new",
                workspace: &spec.workspace_path,
                prompt: &spec.prompt,
                model: spec.model.as_deref(),
                effort: spec.effort.as_deref(),
                conversation: None,
                entrypoint: Some(spec.entrypoint.as_str()).filter(|e| !e.is_empty()),
            },
            Box::new(|_| {}),
        )
    }

    fn resume(&self, spec: &ResumeSpec, on_exit: Box<dyn FnOnce(bool) + Send>) -> Result<(), String> {
        let prompt = if spec.prompt.trim().is_empty() {
            "continue"
        } else {
            spec.prompt.as_str()
        };
        // A launch agy refused never got a conversation; "resuming" it is a
        // retry of the same prompt as a fresh conversation.
        let conversation = Some(spec.session_id.as_str()).filter(|id| !id.starts_with(RUN_ID_PREFIX));
        launch(
            Launch {
                kind: if conversation.is_some() { "resume" } else { "new" },
                workspace: &spec.workspace_path,
                prompt,
                model: spec.model.as_deref(),
                effort: spec.effort.as_deref(),
                conversation,
                // A retried refusal is a new Fleet-started conversation; a
                // resume keeps whatever the conversation already was.
                entrypoint: conversation
                    .is_none()
                    .then_some(crate::session_launch::NEW_SESSION_ENTRYPOINT),
            },
            on_exit,
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema agy 1.5.12 creates (copied from a real
    /// `conversation_summaries.db`; FTS tables omitted).
    const SCHEMA: &str = r#"CREATE TABLE `conversation_summaries` (`conversation_id` text,`title` text NOT NULL DEFAULT "",`preview` text NOT NULL DEFAULT "",`step_count` integer NOT NULL DEFAULT 0,`last_modified_time` datetime NOT NULL,`workspace_uris` text NOT NULL,`status` text NOT NULL DEFAULT "",`source` text NOT NULL DEFAULT "",`project_id` text NOT NULL DEFAULT "",`agent_name` text NOT NULL DEFAULT "",`parent_conversation_id` text NOT NULL DEFAULT "",`nesting_depth` integer NOT NULL DEFAULT 0,`battle_id` text NOT NULL DEFAULT "",`winning_conversation_id` text NOT NULL DEFAULT "",`not_fully_idle` numeric NOT NULL DEFAULT false,`killed` numeric NOT NULL DEFAULT false,`last_user_input_time` datetime NOT NULL,`last_user_input_step_index` integer NOT NULL DEFAULT -1,`app_data_dir` text NOT NULL DEFAULT "",`raw_summary` blob,`group_id` text NOT NULL DEFAULT "",PRIMARY KEY (`conversation_id`));"#;

    fn fixture_db(dir: &Path, rows: &[(&str, &str, &str, &str)]) -> PathBuf {
        let path = dir.join("conversation_summaries.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        for (id, title, modified, uris) in rows {
            conn.execute(
                "INSERT INTO conversation_summaries (conversation_id, title, last_modified_time, \
                 workspace_uris, status, last_user_input_time, app_data_dir) \
                 VALUES (?1, ?2, ?3, ?4, 'CASCADE_RUN_STATUS_IDLE', '0001-01-01 00:00:00+00:00', 'antigravity-cli')",
                rusqlite::params![id, title, modified, uris],
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn reads_summaries_from_an_agy_shaped_database() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fixture_db(
            tmp.path(),
            &[
                ("c-1", "", "0001-01-01 00:00:00+00:00", r#"["file:///tmp/my%20proj"]"#),
                ("c-2", "Fix the build", "2026-11-19 21:14:05.5+00:00", r#"["file:///tmp/other"]"#),
            ],
        );
        let mut rows = read_summaries(&db).unwrap();
        rows.sort_by(|a, b| a.conversation_id.cmp(&b.conversation_id));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].last_modified_ms, None, "Go zero time is unknown");
        assert_eq!(rows[0].workspace_path.as_deref(), Some("/tmp/my proj"));
        assert_eq!(rows[1].title, "Fix the build");
        assert_eq!(rows[1].last_modified_ms, Some(1_795_122_845_500));
        assert_eq!(rows[1].status, "CASCADE_RUN_STATUS_IDLE");
    }

    fn orphan_meta(run_id: &str, pid: u32) -> RunMeta {
        RunMeta {
            run_id: run_id.to_string(),
            kind: "new".to_string(),
            workspace: "/w".to_string(),
            prompt: "hi".to_string(),
            started_ms: 1,
            pid: Some(pid),
            ..Default::default()
        }
    }

    #[test]
    fn orphan_runs_get_an_end_and_an_inferred_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let cases = [
            ("ok", 11, r#"{"type":"init","conversation_id":"c-ok"}
{"type":"result","status":"SUCCESS","conversation_id":"c-ok","response":"done"}"#),
            ("bad", 12, r#"{"type":"result","status":"ERROR","conversation_id":"","error":"not eligible"}"#),
            ("died", 13, r#"{"type":"init","conversation_id":"c-died"}"#),
            ("alive", 14, ""),
        ];
        for (id, pid, out) in cases {
            write_meta(dir, &orphan_meta(id, pid)).unwrap();
            std::fs::write(dir.join(format!("{id}.out")), out).unwrap();
        }
        let mut runs = load_runs(dir);
        settle_orphan_runs(dir, &mut runs, &|pid| pid == 14);

        let reloaded = load_runs(dir);
        let by = |id: &str| reloaded.iter().find(|r| r.meta.run_id == id).unwrap().meta.clone();
        assert_eq!(by("ok").exit_code, Some(0));
        assert_eq!(by("ok").conversation_id.as_deref(), Some("c-ok"));
        assert_eq!(by("bad").exit_code, Some(1));
        assert_eq!(by("died").exit_code, Some(-1));
        assert_eq!(by("died").conversation_id.as_deref(), Some("c-died"));
        for id in ["ok", "bad", "died"] {
            assert!(by(id).ended_ms.is_some(), "{id} must be closed out on disk");
        }
        assert_eq!(by("alive").ended_ms, None, "a live pid is left to its reaper");
        assert!(runs.iter().all(|r| r.meta.run_id == "alive" || r.meta.ended_ms.is_some()));
    }

    #[test]
    fn settled_runs_keep_a_reaper_written_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let mut meta = orphan_meta("r", 21);
        meta.ended_ms = Some(5);
        meta.exit_code = Some(3);
        write_meta(dir, &meta).unwrap();
        let mut runs = load_runs(dir);
        settle_orphan_runs(dir, &mut runs, &|_| false);
        assert_eq!(load_runs(dir)[0].meta.exit_code, Some(3));
    }

    #[test]
    fn launch_env_carries_the_run_and_drops_inherited_ids() {
        let probe = "printf '%s|%s|%s' \"${FLEET_AGY_RUN:-E}\" \"${FLEET_SESSION_ID:-E}\" \"${CLAUDE_CODE_SESSION_ID:-E}\"";
        let run = |conv: Option<&str>| {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.arg("-c").arg(probe);
            cmd.env("FLEET_SESSION_ID", "leaked").env("CLAUDE_CODE_SESSION_ID", "leaked");
            apply_launch_env(&mut cmd, "run-1", conv);
            String::from_utf8(cmd.output().unwrap().stdout).unwrap()
        };
        assert_eq!(run(None), "run-1|E|E");
        assert_eq!(run(Some("conv-9")), "run-1|conv-9|E");
    }

    #[test]
    fn missing_and_empty_databases_are_empty_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.db");
        assert!(read_summaries(&missing).unwrap().is_empty());
        let empty = tmp.path().join("empty.db");
        std::fs::write(&empty, b"").unwrap();
        assert!(read_summaries(&empty).unwrap().is_empty());
        assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0, "must not be initialised");
    }

    #[test]
    fn scan_lists_conversations_with_file_mtime_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fixture_db(
            tmp.path(),
            &[("c-1", "", "0001-01-01 00:00:00+00:00", r#"["file:///tmp/ws"]"#)],
        );
        let conv = tmp.path().join("conversations");
        std::fs::create_dir_all(&conv).unwrap();
        std::fs::write(conv.join("c-1.db"), b"x").unwrap();
        let rows = read_summaries(&db).unwrap();
        let sessions = build_sessions(&rows, Some(&conv), &[], |_| false);
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.id, "c-1");
        assert_eq!(s.agent_source, "agy");
        assert_eq!(s.jsonl_path, "agy://c-1");
        assert_eq!(s.workspace_path, "/tmp/ws");
        assert_eq!(s.workspace_name, "ws");
        assert!(s.last_activity_ms > 0, "falls back to the conversation file mtime");
        assert!(matches!(s.status, SessionStatus::Idle));
    }

    /// Verbatim stdout of `agy -p … --output-format stream-json` on an
    /// account outside the supported regions (agy 1.5.12, 2026-11-19).
    const INELIGIBLE: &str = r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","response":"","error":"Eligibility check failed: Your current account is not eligible for Antigravity, because it is not currently available in your location.","duration_seconds":0,"num_turns":0,"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},"finished_reason":"error"}}"#;

    fn run(id: &str, started: u64, out: &str) -> RunRecord {
        RunRecord {
            meta: RunMeta {
                run_id: id.into(),
                kind: "new".into(),
                workspace: "/tmp/ws".into(),
                prompt: "say hi".into(),
                model: Some("gemini-4-neon-high".into()),
                started_ms: started,
                pid: Some(4242),
                ended_ms: Some(started + 5000),
                exit_code: Some(1),
                ..Default::default()
            },
            outcome: parse_run_output(out),
            stderr_tail: None,
        }
    }

    #[test]
    fn a_refused_launch_is_listed_with_the_raw_error() {
        let mut r = run("r1", 1_000, INELIGIBLE);
        r.meta.entrypoint = Some(crate::session_launch::NEW_SESSION_ENTRYPOINT.into());
        assert_eq!(r.conversation_id(), None);
        let sessions = build_sessions(&[], None, &[r.clone()], |_| false);
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.id, "agy-run-r1");
        assert_eq!(s.agent_source, "agy");
        assert_eq!(s.last_turn_end, Some(TurnEnd::Failed));
        let preview = s.last_message_preview.as_deref().unwrap();
        assert!(preview.contains("Eligibility check failed"), "{preview}");
        assert_eq!(s.model.as_deref(), Some("gemini-4-neon-high"));
        assert_eq!(s.ai_title.as_deref(), Some("say hi"));
        // Without the Fleet entrypoint the task page (and the phone) hide it.
        assert_eq!(s.entrypoint.as_deref(), Some("claw-fleet-newsession"));

        let msgs = detail_messages("agy-run-r1", None, &[], &[r]);
        let text = serde_json::to_string(&msgs).unwrap();
        assert!(!text.contains("暂不支持"), "the phase-1 placeholder is gone");
        assert_eq!(msgs.len(), 2, "prompt + raw outcome: {text}");
        assert_eq!(msgs[0]["message"]["content"], "say hi");
        assert!(text.contains("Eligibility check failed"), "raw error in detail");
    }

    fn conv_fixture(dir: &Path, id: &str) -> PathBuf {
        use crate::agy_conv::fixture::*;
        let conv = dir.join("conversations");
        std::fs::create_dir_all(&conv).unwrap();
        let db = conv.join(format!("{id}.db"));
        conversation_db(
            &db,
            &[
                (0, 3, step(3, 1_795_000_000_000, 19, &sf(1, "fix the build"), &[])),
                (1, 3, step(3, 1_795_000_005_000, 20, &sf(1, "Done, it builds."), &[])),
            ],
            &[generation(
                None,
                Some("gemini-4-neon-high"),
                1_000_000,
                0,
                1_000_000,
                1_000_000,
                0,
                "resp-1",
                Some(1_795_000_004_000),
            )],
        );
        db
    }

    #[test]
    fn scan_overlays_tokens_cost_model_and_preview_from_the_conversation_db() {
        let tmp = tempfile::tempdir().unwrap();
        conv_fixture(tmp.path(), "c-tok");
        let row = SummaryRow {
            conversation_id: "c-tok".into(),
            workspace_path: Some("/tmp/ws".into()),
            ..Default::default()
        };
        let sessions = build_sessions(&[row], Some(&tmp.path().join("conversations")), &[], |_| false);
        let s = &sessions[0];
        assert_eq!(s.total_input_tokens, 1_000_000);
        assert_eq!(s.total_output_tokens, 1_000_000);
        assert_eq!(s.model.as_deref(), Some("gemini-4-neon-high"));
        assert!((s.total_cost_usd - 18.0).abs() < 1e-9, "neon $3 + $15: {}", s.total_cost_usd);
        assert_eq!(s.last_message_preview.as_deref(), Some("Done, it builds."));
        assert!(s.last_activity_ms >= 1_795_000_004_000);
    }

    #[test]
    fn detail_prefers_decoded_steps_and_keeps_fleet_run_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let db = conv_fixture(tmp.path(), "c-steps");
        let conv = conversation(&db, "c-steps");
        let mut failed = run("r9", 1_795_000_010_000, INELIGIBLE);
        failed.meta.conversation_id = Some("c-steps".into());
        let msgs = detail_messages("c-steps", Some(&conv), &[], &[failed]);
        assert_eq!(msgs.len(), 3, "{msgs:#?}");
        assert_eq!(msgs[0]["uuid"], "agy:c-steps:step:0");
        assert_eq!(msgs[1]["message"]["content"][0]["text"], "Done, it builds.");
        assert!(msgs[2]["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Eligibility"));
    }

    #[test]
    fn without_steps_the_detail_merges_history_and_runs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let hist = tmp.path().join("history.jsonl");
        std::fs::write(
            &hist,
            concat!(
                r#"{"display":"/model","timestamp":1,"type":"slash_command"}"#, "\n",
                r#"{"display":"say hi","timestamp":1500,"workspace":"/tmp/ws","conversationId":"c-h"}"#, "\n",
                r#"{"display":"older","timestamp":500,"workspace":"/tmp/ws","conversationId":"c-h"}"#, "\n",
                r#"{"display":"other conv","timestamp":600,"conversationId":"c-x"}"#, "\n",
                r#"{"display":"no conv","timestamp":700}"#, "\n",
            ),
        )
        .unwrap();
        let history = read_history(&hist);
        assert_eq!(history.len(), 3, "slash commands and conversation-less lines dropped");
        let mut r = run("r10", 1_000, INELIGIBLE);
        r.meta.conversation_id = Some("c-h".into());
        let msgs = detail_messages("c-h", None, &history, &[r]);
        let texts: Vec<String> = msgs
            .iter()
            .map(|m| m["message"]["content"].to_string())
            .collect();
        assert_eq!(msgs.len(), 3, "{texts:?}");
        assert!(texts[0].contains("older"));
        assert!(texts[1].contains("say hi"), "the run prompt duplicates history and shows once");
        assert!(texts[2].contains("Eligibility"));
    }

    #[test]
    fn tail_fingerprint_changes_with_the_wal_and_is_never_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let db = conv_fixture(tmp.path(), "c-fp");
        let a = detail_fingerprint("c-fp", Some(&db), None, &[]);
        assert_ne!(a, 0);
        assert_eq!(a, detail_fingerprint("c-fp", Some(&db), None, &[]));
        std::fs::write(wal_path(&db), b"grown").unwrap();
        assert_ne!(a, detail_fingerprint("c-fp", Some(&db), None, &[]));
        let r = run("r11", 1, "");
        assert_ne!(
            detail_fingerprint("agy-run-r11", None, None, &[]),
            detail_fingerprint("agy-run-r11", None, None, &[r])
        );
    }

    #[test]
    fn init_names_the_conversation_and_joins_its_index_row() {
        let out = concat!(
            r#"{"event":"init","conversation_id":"c-9","init":{"model":"gemini-4-neon-high"}}"#,
            "\n",
            r#"{"event":"step_update","conversation_id":"c-9","step_index":0,"text_delta":"hi"}"#,
            "\n",
        );
        let mut r = run("r2", 2_000, out);
        r.meta.ended_ms = None;
        r.meta.exit_code = None;
        assert_eq!(r.session_key(), "c-9");
        let row = SummaryRow {
            conversation_id: "c-9".into(),
            workspace_path: Some("/tmp/ws".into()),
            ..Default::default()
        };
        let sessions = build_sessions(&[row], None, &[r], |pid| pid == 4242);
        assert_eq!(sessions.len(), 1, "the run merges into the conversation row");
        assert!(sessions[0].proc_alive);
        assert_eq!(sessions[0].pid, Some(4242));
        assert!(matches!(sessions[0].status, SessionStatus::Active));
    }

    #[test]
    fn a_turn_that_died_without_a_result_uses_stderr() {
        let mut r = run("r3", 3_000, "");
        r.stderr_tail = Some("keyring: access denied".into());
        assert_eq!(r.error_text().as_deref(), Some("keyring: access denied"));
    }

    #[test]
    fn model_specs_and_argv() {
        assert!(is_agy_model("agy:claude-opus-5-5-high"));
        assert!(is_agy_model("gemini-4-neon-high"));
        assert!(!is_agy_model("claude-opus-5-5-high"));
        assert!(!is_agy_model("gpt-oss-120b-medium"));
        assert_eq!(model_arg(Some("agy:gpt-oss-120b-medium")).as_deref(), Some("gpt-oss-120b-medium"));
        assert_eq!(model_arg(Some("  ")), None);
        let args = build_args("-x hi", Some("agy:gemini-3.8-flash-low"), Some("high"), Some("c-1"));
        assert_eq!(
            args,
            vec![
                "-p", "-x hi", "--output-format", "stream-json", "--model",
                "gemini-3.8-flash-low", "--effort", "high", "--conversation", "c-1"
            ]
        );
        assert!(!args.iter().any(|a| a.contains("dangerously")));
    }

    #[test]
    fn run_records_round_trip_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = RunMeta {
            run_id: "r4".into(),
            kind: "new".into(),
            workspace: "/tmp/ws".into(),
            prompt: "p".into(),
            started_ms: 10,
            ..Default::default()
        };
        write_meta(tmp.path(), &meta).unwrap();
        std::fs::write(tmp.path().join("r4.out"), INELIGIBLE).unwrap();
        std::fs::write(tmp.path().join("r4.err"), "error: boom\n\n").unwrap();
        update_meta(tmp.path(), "r4", |m| m.exit_code = Some(1));
        let runs = load_runs(tmp.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].meta.exit_code, Some(1));
        // Records written before `entrypoint` existed still load.
        let legacy = r#"{"runId":"r5","kind":"new","workspace":"/w","prompt":"p","startedMs":1}"#;
        let m: RunMeta = serde_json::from_str(legacy).unwrap();
        assert_eq!(m.entrypoint, None);
        assert_eq!(runs[0].stderr_tail.as_deref(), Some("boom"));
        assert!(runs[0].outcome.failed());
    }
}
