//! Drift check: is a relay chain still heading for the goal it was given?
//!
//! A long relay chain rarely loses state — the plan, the notes and the goal all
//! survive every hop. What it loses is direction: each hop reinterprets the
//! work a little, and twenty hops later the chain is polishing a detail or
//! chasing a narrower objective than the one the user set, with every checkbox
//! still green. The hops themselves cannot see this, because their context is
//! full of the reasons each step seemed right at the time.
//!
//! So the check is done by an outsider: a fresh LLM call that sees only the
//! chain's goal (with its recorded revisions), its plan checklist and the last
//! few handoff notes — never the transcripts — and answers one question. Only
//! a verdict other than "on track" is surfaced to the user.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::handoff::HandoffChain;
use crate::llm_provider::LlmProvider;
use crate::log_debug;

pub const SCENARIO_DRIFT_CHECK: &str = "drift_check";

/// A chain shorter than this has not had time to drift.
const MIN_SESSIONS: usize = 3;
/// Only chains that handed off recently are still being worked on.
const ACTIVE_WITHIN_MS: u64 = 3 * 24 * 3600 * 1000;
/// At most one check per chain per this interval, however fast it hops.
const RECHECK_AFTER_MS: u64 = 20 * 3600 * 1000;
/// Upper bound on LLM calls per scheduler pass.
const MAX_PER_PASS: usize = 8;
/// How many of the latest handoff notes the outsider reads, and how much of each.
const NOTES_SHOWN: usize = 3;
const NOTE_CHARS: usize = 4000;
const CHECK_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriftVerdict {
    /// Recent hops make visible progress toward the goal.
    OnTrack,
    /// Recent hops refine details, tooling or process while the goal itself
    /// gets no closer.
    Polishing,
    /// What the hops actually pursue differs from the stated goal, or the goal
    /// was rewritten narrower without a convincing reason.
    GoalShifted,
    /// The notes do not say enough to tell.
    Unclear,
}

impl DriftVerdict {
    /// Whether this verdict is something the user should look at.
    pub fn needs_attention(self) -> bool {
        matches!(self, DriftVerdict::Polishing | DriftVerdict::GoalShifted)
    }
}

/// One outsider verdict on one chain, as of a given hop.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DriftCheck {
    pub chain_id: String,
    pub workspace_path: String,
    pub workspace_name: String,
    pub plan_id: Option<String>,
    pub goal: String,
    /// Sessions on the chain when it was checked.
    pub session_count: u32,
    /// The chain's newest session — where the user would go to intervene.
    pub latest_session_id: String,
    pub verdict: DriftVerdict,
    /// One or two sentences citing what in the notes led to the verdict.
    pub evidence: String,
    /// The question the user should settle; empty when on track.
    pub question: String,
    /// Epoch ms.
    pub checked_at: u64,
}

// ── Store ────────────────────────────────────────────────────────────────────

/// Drift checks live in `fleet-reports.db` next to the daily reports that
/// surface them.
pub struct DriftStore {
    conn: Connection,
}

impl DriftStore {
    pub fn open() -> Result<Self, String> {
        let db_path = crate::session::real_home_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?
            .join(".fleet")
            .join("fleet-reports.db");
        Self::open_at(&db_path)
    }

    pub fn open_at(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path).map_err(|e| format!("sqlite open: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS drift_checks (
                 chain_id      TEXT NOT NULL,
                 session_count INTEGER NOT NULL,
                 checked_at    INTEGER NOT NULL,
                 body          TEXT NOT NULL,
                 PRIMARY KEY (chain_id, session_count)
             );
             CREATE INDEX IF NOT EXISTS drift_checks_checked_at
                 ON drift_checks (checked_at);",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;
        Ok(Self { conn })
    }

    pub fn save(&self, check: &DriftCheck) -> Result<(), String> {
        let body = serde_json::to_string(check).map_err(|e| format!("json encode: {e}"))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO drift_checks (chain_id, session_count, checked_at, body)
                 VALUES (?1, ?2, ?3, ?4)",
                params![check.chain_id, check.session_count, check.checked_at, body],
            )
            .map_err(|e| format!("save drift check: {e}"))?;
        Ok(())
    }

    /// The most recent check of every chain that has one.
    pub fn latest_per_chain(&self) -> Result<HashMap<String, DriftCheck>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT body FROM drift_checks ORDER BY checked_at")
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("query: {e}"))?;
        let mut out = HashMap::new();
        for body in rows.flatten() {
            if let Ok(c) = serde_json::from_str::<DriftCheck>(&body) {
                out.insert(c.chain_id.clone(), c);
            }
        }
        Ok(out)
    }

    /// Checks made in `[from_ms, to_ms)`, oldest first.
    pub fn list_in_range(&self, from_ms: u64, to_ms: u64) -> Result<Vec<DriftCheck>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body FROM drift_checks
                 WHERE checked_at >= ?1 AND checked_at < ?2 ORDER BY checked_at",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(params![from_ms, to_ms], |row| row.get::<_, String>(0))
            .map_err(|e| format!("query: {e}"))?;
        Ok(rows
            .flatten()
            .filter_map(|b| serde_json::from_str(&b).ok())
            .collect())
    }

    /// A single check, if recorded.
    pub fn get(&self, chain_id: &str, session_count: u32) -> Result<Option<DriftCheck>, String> {
        let body: Option<String> = self
            .conn
            .query_row(
                "SELECT body FROM drift_checks WHERE chain_id = ?1 AND session_count = ?2",
                params![chain_id, session_count],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("query: {e}"))?;
        Ok(body.and_then(|b| serde_json::from_str(&b).ok()))
    }
}

// ── Selection ────────────────────────────────────────────────────────────────

fn last_handed_at(chain: &HandoffChain) -> u64 {
    chain.links.iter().map(|l| l.handed_at).max().unwrap_or(0)
}

/// Chains worth an outsider look right now, newest activity first: they have a
/// goal, enough hops to have drifted, a handoff in the last few days, at least
/// one hop since their last check, and no check in the last ~day.
pub fn chains_due<'a>(
    chains: &'a [HandoffChain],
    latest: &HashMap<String, DriftCheck>,
    now_ms: u64,
) -> Vec<&'a HandoffChain> {
    let mut due: Vec<&HandoffChain> = chains
        .iter()
        .filter(|c| c.goal.as_deref().is_some_and(|g| !g.trim().is_empty()))
        .filter(|c| c.session_ids().len() >= MIN_SESSIONS)
        .filter(|c| now_ms.saturating_sub(last_handed_at(c)) < ACTIVE_WITHIN_MS)
        .filter(|c| match latest.get(&c.chain_id) {
            None => true,
            Some(prev) => {
                c.session_ids().len() as u32 > prev.session_count
                    && now_ms.saturating_sub(prev.checked_at) >= RECHECK_AFTER_MS
            }
        })
        .collect();
    due.sort_by_key(|c| std::cmp::Reverse(last_handed_at(c)));
    due.truncate(MAX_PER_PASS);
    due
}

// ── Prompt ───────────────────────────────────────────────────────────────────

fn clip(s: &str, limit: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(limit) {
        None => s.to_string(),
        Some((byte, _)) => format!("{}…", &s[..byte]),
    }
}

/// The plan checklist as `[x] P1 — …` lines, if the chain's plan can be found.
fn plan_checklist(chain: &HandoffChain) -> Option<String> {
    let plan_id = chain.plan_id.as_deref()?;
    let cwd = Path::new(&chain.workspace_path);
    let source = crate::prd_tasks::find_plan_source(cwd, plan_id)?;
    let content = std::fs::read_to_string(source).ok()?;
    let body = crate::prd_tasks::plan_body(&content, plan_id)?;
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("- [ ]") || l.starts_with("- [x]") || l.starts_with("- [X]"))
        .collect();
    if lines.is_empty() {
        return None;
    }
    let mut out = String::new();
    if let Some(name) = crate::prd_tasks::extract_plan_name(&body) {
        out.push_str(&format!("Plan: {name}\n"));
    }
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    Some(out)
}

pub(crate) fn build_prompt(chain: &HandoffChain, checklist: Option<&str>, locale: &str) -> String {
    let goal = chain.goal.as_deref().unwrap_or_default();
    let lang = match locale {
        "zh" => "evidence 和 question 用中文写。",
        _ => "Write evidence and question in English.",
    };

    let mut history = String::new();
    for rev in chain.goal_history.iter().skip(1) {
        history.push_str(&format!(
            "- hop {}: changed from \"{}\" to \"{}\" — reason: {}\n",
            rev.hop,
            clip(rev.from.as_deref().unwrap_or_default(), 300),
            clip(&rev.to, 300),
            rev.reason.as_deref().unwrap_or("(none recorded)")
        ));
    }
    let history = if history.is_empty() {
        "The goal has not been revised since it was set.\n".to_string()
    } else {
        format!("Goal revisions:\n{history}")
    };

    let checklist = checklist
        .map(|c| format!("PLAN CHECKLIST (current state):\n{c}\n"))
        .unwrap_or_default();

    let total = chain.session_ids().len();
    let mut notes = String::new();
    let shown: Vec<_> = chain
        .links
        .iter()
        .enumerate()
        .rev()
        .take(NOTES_SHOWN)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    for (i, link) in shown {
        notes.push_str(&format!(
            "--- Handoff note written by hop {} of {total} ---\n{}\n\n",
            i + 1,
            clip(&link.note, NOTE_CHARS)
        ));
    }

    format!(
        "You are an outside reviewer. A chain of AI agent sessions has been working on a \
         goal for {total} sessions, each handing off to the next with a note. You have \
         never seen this work and you see none of the transcripts — only the goal, the \
         plan and the latest handoff notes. That is deliberate: judge the work the way \
         its owner would on opening it cold.\n\n\
         Answer one question: is the most recent work still heading toward the GOAL?\n\n\
         Verdicts:\n\
         - on_track: the latest hops make visible progress toward what the goal says \
           done looks like.\n\
         - polishing: the latest hops refine details, tooling, rules or process — tuning \
           thresholds, hardening edge cases, writing docs or harnesses — while the core of \
           the goal gets no closer.\n\
         - goal_shifted: what the hops actually pursue differs from the stated goal, or \
           the goal was rewritten narrower without a convincing reason.\n\
         - unclear: the notes do not say enough to tell.\n\n\
         Be skeptical of a chain that reports every box ticked: a green checklist is not \
         evidence of progress toward the goal. But do not invent problems — a chain doing \
         exactly what the goal asks is on_track.\n\n\
         GOAL:\n{goal}\n\n\
         {history}\n\
         {checklist}\
         {notes}\
         Reply with ONLY a JSON object, no prose around it:\n\
         {{\"verdict\": \"on_track|polishing|goal_shifted|unclear\", \
         \"evidence\": \"<at most two sentences citing concrete things from the notes>\", \
         \"question\": \"<one question the goal's owner should settle; empty string when on_track>\"}}\n\n\
         {lang}"
    )
}

#[derive(Deserialize)]
struct RawVerdict {
    verdict: DriftVerdict,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    question: String,
}

/// Pull the verdict object out of a reply that may wrap it in prose or a code
/// fence.
pub(crate) fn parse_verdict(raw: &str) -> Option<(DriftVerdict, String, String)> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    let v: RawVerdict = serde_json::from_str(&raw[start..=end]).ok()?;
    Some((v.verdict, v.evidence.trim().to_string(), v.question.trim().to_string()))
}

// ── Running ──────────────────────────────────────────────────────────────────

fn check_chain(
    provider: &dyn LlmProvider,
    model: &str,
    chain: &HandoffChain,
    locale: &str,
    now_ms: u64,
) -> Option<DriftCheck> {
    if !provider.is_available() {
        return None;
    }
    let checklist = plan_checklist(chain);
    let prompt = build_prompt(chain, checklist.as_deref(), locale);
    let raw = crate::llm_usage::complete_accounted(
        provider,
        &prompt,
        model,
        CHECK_TIMEOUT,
        SCENARIO_DRIFT_CHECK,
    )?;
    let Some((verdict, evidence, question)) = parse_verdict(&raw) else {
        log_debug(&format!(
            "[drift_check] unparseable verdict for chain {}",
            chain.chain_id
        ));
        return None;
    };
    let ids = chain.session_ids();
    Some(DriftCheck {
        chain_id: chain.chain_id.clone(),
        workspace_path: chain.workspace_path.clone(),
        workspace_name: crate::session::workspace_name(&chain.workspace_path),
        plan_id: chain.plan_id.clone(),
        goal: chain.goal.clone().unwrap_or_default(),
        session_count: ids.len() as u32,
        latest_session_id: ids.last().cloned().unwrap_or_default(),
        verdict,
        question: if verdict == DriftVerdict::OnTrack {
            String::new()
        } else {
            question
        },
        evidence,
        checked_at: now_ms,
    })
}

fn check_chain_routed(
    config: &crate::llm_provider::LlmConfig,
    chain: &HandoffChain,
    locale: &str,
    now_ms: u64,
) -> Option<DriftCheck> {
    for route in crate::llm_provider::daily_report_routes(config) {
        if let Some(c) = check_chain(route.provider.as_ref(), &route.model, chain, locale, now_ms) {
            return Some(c);
        }
    }
    None
}

/// Check every chain that is due and store the verdicts. Returns the new
/// checks that need the user's attention.
pub fn run_due_checks(config: &crate::llm_provider::LlmConfig, locale: &str) -> Vec<DriftCheck> {
    let store = match DriftStore::open() {
        Ok(s) => s,
        Err(e) => {
            log_debug(&format!("[drift_check] open store failed: {e}"));
            return Vec::new();
        }
    };
    let latest = store.latest_per_chain().unwrap_or_default();
    let chains = crate::handoff::list_chains();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut flagged = Vec::new();
    for chain in chains_due(&chains, &latest, now_ms) {
        let Some(check) = check_chain_routed(config, chain, locale, now_ms) else {
            continue;
        };
        log_debug(&format!(
            "[drift_check] chain {} ({} sessions): {:?}",
            check.chain_id, check.session_count, check.verdict
        ));
        if let Err(e) = store.save(&check) {
            log_debug(&format!("[drift_check] save failed: {e}"));
            continue;
        }
        if check.verdict.needs_attention() {
            flagged.push(check);
        }
    }
    flagged
}

/// Checks made on local calendar day `date` (`YYYY-MM-DD`). Soft: an
/// unreadable store yields none.
pub fn checks_for_date(date: &str) -> Vec<DriftCheck> {
    let Some((from_ms, to_ms)) = crate::daily_report::local_day_bounds_ms(date) else {
        return Vec::new();
    };
    DriftStore::open()
        .and_then(|s| s.list_in_range(from_ms, to_ms))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{GoalRevision, HandoffLink};

    const NOW: u64 = 10_000_000_000_000;
    const HOUR: u64 = 3600 * 1000;

    fn chain(id: &str, goal: Option<&str>, hops: usize, last_at: u64) -> HandoffChain {
        let links = (0..hops)
            .map(|i| HandoffLink {
                from_session_id: format!("{id}-s{i}"),
                to_session_id: format!("{id}-s{}", i + 1),
                note: format!("note {i}"),
                plan_id: None,
                next_task: None,
                handed_at: last_at - (hops - 1 - i) as u64 * HOUR,
            })
            .collect();
        HandoffChain {
            chain_id: id.into(),
            workspace_path: "/tmp/repo".into(),
            plan_id: None,
            goal: goal.map(str::to_string),
            goal_history: Vec::new(),
            links,
        }
    }

    fn check(id: &str, sessions: u32, at: u64) -> DriftCheck {
        DriftCheck {
            chain_id: id.into(),
            workspace_path: String::new(),
            workspace_name: String::new(),
            plan_id: None,
            goal: String::new(),
            session_count: sessions,
            latest_session_id: String::new(),
            verdict: DriftVerdict::OnTrack,
            evidence: String::new(),
            question: String::new(),
            checked_at: at,
        }
    }

    #[test]
    fn due_requires_goal_length_and_recent_activity() {
        let chains = vec![
            chain("ok", Some("ship it"), 2, NOW - HOUR),       // 3 sessions
            chain("nogoal", None, 5, NOW - HOUR),
            chain("short", Some("ship it"), 1, NOW - HOUR),   // 2 sessions
            chain("stale", Some("ship it"), 5, NOW - 4 * 24 * HOUR),
        ];
        let due: Vec<_> = chains_due(&chains, &HashMap::new(), NOW)
            .into_iter()
            .map(|c| c.chain_id.as_str())
            .collect();
        assert_eq!(due, vec!["ok"]);
    }

    #[test]
    fn due_waits_for_a_new_hop_and_a_day_since_the_last_check() {
        let chains = vec![chain("c", Some("g"), 4, NOW - HOUR)]; // 5 sessions
        let mut latest = HashMap::new();

        latest.insert("c".to_string(), check("c", 5, NOW - 30 * HOUR));
        assert!(chains_due(&chains, &latest, NOW).is_empty(), "no hop since the check");

        latest.insert("c".to_string(), check("c", 4, NOW - 2 * HOUR));
        assert!(chains_due(&chains, &latest, NOW).is_empty(), "checked too recently");

        latest.insert("c".to_string(), check("c", 4, NOW - 30 * HOUR));
        assert_eq!(chains_due(&chains, &latest, NOW).len(), 1);
    }

    #[test]
    fn due_is_capped_and_newest_first() {
        let chains: Vec<_> = (0..12)
            .map(|i| chain(&format!("c{i}"), Some("g"), 3, NOW - (i as u64 + 1) * HOUR))
            .collect();
        let due = chains_due(&chains, &HashMap::new(), NOW);
        assert_eq!(due.len(), MAX_PER_PASS);
        assert_eq!(due[0].chain_id, "c0");
    }

    #[test]
    fn prompt_shows_goal_revisions_and_only_the_latest_notes() {
        let mut c = chain("c", Some("make paper trading run"), 5, NOW);
        c.goal_history = vec![
            GoalRevision {
                hop: 1,
                session_id: "c-s0".into(),
                from: None,
                to: "make live trading run".into(),
                reason: None,
                at: 0,
            },
            GoalRevision {
                hop: 4,
                session_id: "c-s3".into(),
                from: Some("make live trading run".into()),
                to: "make paper trading run".into(),
                reason: Some("exchange keys not ready".into()),
                at: 0,
            },
        ];
        let p = build_prompt(&c, Some("- [x] **P1** — a\n"), "en");
        assert!(p.contains("make paper trading run"));
        assert!(p.contains("exchange keys not ready"));
        assert!(p.contains("- [x] **P1** — a"));
        assert!(p.contains("note 4") && p.contains("note 2"));
        assert!(!p.contains("note 1"), "only the last {NOTES_SHOWN} notes are shown");
    }

    #[test]
    fn verdict_parses_through_fences_and_prose() {
        let raw = "Here you go:\n```json\n{\"verdict\":\"polishing\",\"evidence\":\"tuning fonts\",\"question\":\"ship first?\"}\n```";
        let (v, e, q) = parse_verdict(raw).unwrap();
        assert_eq!(v, DriftVerdict::Polishing);
        assert_eq!(e, "tuning fonts");
        assert_eq!(q, "ship first?");
        assert!(parse_verdict("{\"verdict\":\"sideways\"}").is_none());
        assert!(parse_verdict("no json").is_none());
    }

    #[test]
    fn store_round_trips_and_keeps_latest_per_chain() {
        let dir = std::env::temp_dir().join(format!(
            "drift-store-{}-{}",
            std::process::id(),
            NOW
        ));
        let db = dir.join("r.db");
        let store = DriftStore::open_at(&db).unwrap();
        store.save(&check("a", 3, 100)).unwrap();
        let mut newer = check("a", 5, 200);
        newer.verdict = DriftVerdict::GoalShifted;
        store.save(&newer).unwrap();
        store.save(&check("b", 4, 150)).unwrap();

        let latest = store.latest_per_chain().unwrap();
        assert_eq!(latest["a"].session_count, 5);
        assert_eq!(latest["a"].verdict, DriftVerdict::GoalShifted);
        assert_eq!(store.list_in_range(120, 201).unwrap().len(), 2);
        assert!(store.get("a", 3).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
