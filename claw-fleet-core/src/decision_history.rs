//! Decision history — append-only per-session log of every AskUserQuestion
//! (`elicitation`) and ExitPlanMode (`plan-approval`) decision card the user
//! has seen, including the questions/options shown and the user's choice.
//!
//! Storage: `~/.fleet/decision-history/<session_id>.jsonl`, one record per
//! line. Records are written by the `fleet elicitation` and `fleet
//! plan-approval` CLIs at the moment a response (or terminal non-response —
//! timeout, declined, heartbeat lost) is observed, before the request file is
//! cleaned up.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};
use crate::mcp_ipc::{FleetAskQuestion, FleetAskRequest};
use crate::plan_approval::{PlanApprovalRequest, PlanApprovalResponse};

// ── Outcome enums ────────────────────────────────────────────────────────────

/// Terminal outcome of an elicitation card.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum ElicitationOutcome {
    /// User picked an option (or typed via "Other"). `answers` is populated.
    Answered,
    /// User explicitly closed the card.
    Declined,
    /// Desktop consumer disappeared mid-flight; CLI fell back to native UI.
    HeartbeatLost,
    /// 600s elapsed without any response.
    Timeout,
}

/// Terminal outcome of a plan-approval card.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum PlanApprovalOutcome {
    Approved,
    ApprovedWithEdits,
    Rejected,
    HeartbeatLost,
    Timeout,
}

/// Terminal outcome of a fleet__ask card.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum FleetAskOutcome {
    /// User submitted answers via the Decision Panel. `answers` is populated.
    Answered,
    /// User clicked Cancel.
    Cancelled,
    /// Desktop consumer disappeared mid-flight.
    HeartbeatLost,
    /// Configured wait_seconds elapsed without any response.
    Timeout,
}

// ── Selected-option enrichment ───────────────────────────────────────────────

/// What the user picked for one elicitation question, enriched with the
/// matching option's label/description so the history is readable without
/// cross-referencing.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SelectedOption {
    /// Option label as shown in the card. Falls back to the raw answer string
    /// when the user typed via "Other" (no matching option).
    pub label: String,
    /// Option description shown as helper text. `None` when the user typed
    /// via "Other".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Was this answer typed via the "Other" escape hatch?
    #[serde(default, skip_serializing_if = "is_false")]
    pub other: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ── Record envelope ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DecisionHistoryRecord {
    Elicitation(ElicitationRecord),
    PlanApproval(PlanApprovalRecord),
    UserPrompt(UserPromptRecord),
    FleetAsk(FleetAskRecord),
}

/// A real user-typed prompt extracted from the session JSONL.
///
/// Filter rules applied during extraction:
/// - `type == "user"` with `message.role == "user"`
/// - `isSidechain` is false (subagent task descriptions are not the user)
/// - `isCompactSummary` is false (those summaries are written by Claude Code)
/// - At least one `text` content block whose text does NOT begin with
///   `<ide_opened_file>` or `<ide_selection>` (IDE auto-injected context).
///
/// `id` is the jsonl entry's uuid, which is stable across re-scans and used
/// for de-duplication when syncing into `decision_history.jsonl`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(rename = "UserPromptHistoryRecord"))]
#[serde(rename_all = "camelCase")]
pub struct UserPromptRecord {
    pub id: String,
    pub session_id: String,
    /// Concatenated user-typed text (one block per `\n\n`). Injected blocks
    /// that match the filter list are stripped before joining.
    pub text: String,
    /// True when the user pasted at least one image alongside their prompt.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_image: bool,
    /// jsonl entry timestamp (ISO-8601 UTC).
    pub sent_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(rename = "ElicitationHistoryRecord"))]
#[serde(rename_all = "camelCase")]
pub struct ElicitationRecord {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    /// When the request was originally raised.
    pub requested_at: String,
    /// When the terminal outcome was recorded.
    pub resolved_at: String,
    pub outcome: ElicitationOutcome,
    pub questions: Vec<ElicitationQuestion>,
    /// `question text → selected option`. Empty unless `outcome = answered`.
    #[serde(default)]
    pub answers: HashMap<String, SelectedOption>,
}

/// Persisted shape of a resolved `fleet__ask` card. Mirrors the v2 MCP-IPC
/// `FleetAskRequest` + `FleetAskResponse` so the history view can reconstruct
/// what the user saw and what they submitted.
///
/// `answers` is the same flat map the response file carries: a question's
/// text → user's option / "Other" string (possibly with `@path` mention
/// suffixes from attachments), and each form-field's `name` → value. The
/// namespace overlap is benign in practice because question text is prose
/// and field names are identifiers.
///
/// The card may have shown sandboxed HTML at render time; the original HTML
/// content is preserved on each question so the record stays self-contained,
/// but the history viewer must NOT re-render it as an iframe — display only
/// an "[HTML preview was shown]" marker.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(rename = "FleetAskHistoryRecord"))]
#[serde(rename_all = "camelCase")]
pub struct FleetAskRecord {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    pub requested_at: String,
    pub resolved_at: String,
    pub outcome: FleetAskOutcome,
    pub questions: Vec<FleetAskQuestion>,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(rename = "PlanApprovalHistoryRecord"))]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalRecord {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    pub requested_at: String,
    pub resolved_at: String,
    pub outcome: PlanApprovalOutcome,
    pub plan_content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_file_path: Option<String>,
    /// Present when outcome = approved-with-edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_plan: Option<String>,
    /// Present when outcome = rejected and the user supplied feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

impl DecisionHistoryRecord {
    pub fn session_id(&self) -> &str {
        match self {
            DecisionHistoryRecord::Elicitation(r) => &r.session_id,
            DecisionHistoryRecord::PlanApproval(r) => &r.session_id,
            DecisionHistoryRecord::UserPrompt(r) => &r.session_id,
            DecisionHistoryRecord::FleetAsk(r) => &r.session_id,
        }
    }

    /// Stable per-session record id used for dedup on read-side merging.
    pub fn id(&self) -> &str {
        match self {
            DecisionHistoryRecord::Elicitation(r) => &r.id,
            DecisionHistoryRecord::PlanApproval(r) => &r.id,
            DecisionHistoryRecord::UserPrompt(r) => &r.id,
            DecisionHistoryRecord::FleetAsk(r) => &r.id,
        }
    }
}

// ── Builder helpers ──────────────────────────────────────────────────────────

/// Build an elicitation record given the original request and the user's raw
/// answer map (`question text → option label`). The `answers` map is enriched
/// with each matching option's description; unmatched answers are flagged as
/// `other = true`.
pub fn build_elicitation_record(
    req: &ElicitationRequest,
    outcome: ElicitationOutcome,
    raw_answers: &HashMap<String, String>,
    resolved_at: String,
) -> ElicitationRecord {
    let mut enriched: HashMap<String, SelectedOption> = HashMap::new();
    if matches!(outcome, ElicitationOutcome::Answered) {
        for q in &req.questions {
            let Some(answer) = raw_answers.get(&q.question) else {
                continue;
            };
            // Multi-select answers are joined with ", " by the desktop side;
            // try to match each piece against the question's option list.
            let pieces: Vec<&str> = answer.split(',').map(|p| p.trim()).collect();
            let matched: Vec<&ElicitationOption> = pieces
                .iter()
                .filter_map(|p| q.options.iter().find(|o| o.label == *p))
                .collect();

            let selected = if matched.len() == pieces.len() && !matched.is_empty() {
                let labels = matched
                    .iter()
                    .map(|o| o.label.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let descriptions = matched
                    .iter()
                    .map(|o| o.description.clone())
                    .collect::<Vec<_>>()
                    .join(" / ");
                SelectedOption {
                    label: labels,
                    description: Some(descriptions),
                    other: false,
                }
            } else {
                SelectedOption {
                    label: answer.clone(),
                    description: None,
                    other: true,
                }
            };
            enriched.insert(q.question.clone(), selected);
        }
    }

    ElicitationRecord {
        id: req.id.clone(),
        session_id: req.session_id.clone(),
        workspace_name: req.workspace_name.clone(),
        ai_title: req.ai_title.clone(),
        requested_at: req.timestamp.clone(),
        resolved_at,
        outcome,
        questions: req.questions.clone(),
        answers: enriched,
    }
}

/// Build a plan-approval record. `resp` is `None` for timeout / heartbeat-lost;
/// otherwise it carries the user's decision and (on approve) any edited plan.
pub fn build_plan_approval_record(
    req: &PlanApprovalRequest,
    outcome: PlanApprovalOutcome,
    resp: Option<&PlanApprovalResponse>,
    resolved_at: String,
) -> PlanApprovalRecord {
    let edited_plan = resp.and_then(|r| r.edited_plan.clone());
    let feedback = resp.and_then(|r| r.feedback.clone());
    PlanApprovalRecord {
        id: req.id.clone(),
        session_id: req.session_id.clone(),
        workspace_name: req.workspace_name.clone(),
        ai_title: req.ai_title.clone(),
        requested_at: req.timestamp.clone(),
        resolved_at,
        outcome,
        plan_content: req.plan_content.clone(),
        plan_file_path: req.plan_file_path.clone(),
        edited_plan,
        feedback,
    }
}

/// Build a fleet__ask record. `answers` is the flat map from the response
/// file (empty for non-Answered outcomes). The original request's questions
/// are cloned so the history view stays self-contained even if the schema
/// changes later.
pub fn build_fleet_ask_record(
    req: &FleetAskRequest,
    outcome: FleetAskOutcome,
    answers: BTreeMap<String, String>,
    resolved_at: String,
) -> FleetAskRecord {
    let answers = if matches!(outcome, FleetAskOutcome::Answered) {
        answers
    } else {
        BTreeMap::new()
    };
    FleetAskRecord {
        id: req.id.clone(),
        session_id: req.session_id.clone(),
        workspace_name: req.workspace_name.clone(),
        ai_title: req.ai_title.clone(),
        requested_at: req.timestamp.clone(),
        resolved_at,
        outcome,
        questions: req.questions.clone(),
        answers,
    }
}

// ── Daily decision-card stats ────────────────────────────────────────────────

/// Per-type aggregate for one day's decision cards. `elicitation` and
/// `fleet-ask` are option-based so `recommendedHit` / `withRecommendation` /
/// `otherPick` are meaningful; `plan-approval` only populates the outcome
/// counters (approve counts as `answered`, reject as `declined`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DecisionTypeStats {
    /// Total cards of this type whose `requestedAt` falls on the day.
    pub triggered: u32,
    /// Terminal outcome = answered (fleet-ask) / approved (plan-approval).
    pub answered: u32,
    /// Declined / cancelled / rejected.
    pub declined: u32,
    /// Desktop consumer disappeared mid-flight.
    pub heartbeat_lost: u32,
    /// Wait window elapsed with no response.
    pub timeout: u32,
    /// Answered cards whose first question presented an explicit
    /// `(Recommended)` option — the denominator for the hit rate.
    pub with_recommendation: u32,
    /// Answered cards where the user picked that recommended option.
    pub recommended_hit: u32,
    /// Answered cards where the user's first-question pick was NOT one of the
    /// offered options (the "Other" free-text escape hatch).
    pub other_pick: u32,
    /// Sum of answer latency (resolvedAt − requestedAt) over answered cards.
    pub latency_secs_sum: f64,
    /// Count of answered cards contributing to `latencySecsSum` (the divisor
    /// for the average). May be < `answered` if a timestamp failed to parse.
    pub latency_count: u32,
}

/// A day's decision-card analytics, keyed by card type
/// (`elicitation` | `fleet-ask` | `plan-approval`). Embedded in `DailyMetrics`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DecisionCardStats {
    #[serde(default)]
    pub by_type: BTreeMap<String, DecisionTypeStats>,
}

/// Parse an RFC-3339 timestamp and return its local calendar date (YYYY-MM-DD).
fn local_date_of(ts: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    Some(
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string(),
    )
}

fn add_latency(s: &mut DecisionTypeStats, requested_at: &str, resolved_at: &str) {
    if let (Ok(a), Ok(b)) = (
        chrono::DateTime::parse_from_rfc3339(requested_at),
        chrono::DateTime::parse_from_rfc3339(resolved_at),
    ) {
        let secs = (b - a).num_milliseconds() as f64 / 1000.0;
        if secs >= 0.0 {
            s.latency_secs_sum += secs;
            s.latency_count += 1;
        }
    }
}

/// Label suffix that marks the recommended option in a card.
fn is_recommended_label(label: &str) -> bool {
    label.trim_end().ends_with("(Recommended)")
}

fn accumulate_first_q_elicitation(s: &mut DecisionTypeStats, r: &ElicitationRecord) {
    let Some(q) = r.questions.first() else {
        return;
    };
    let rec_label = q
        .options
        .iter()
        .map(|o| o.label.as_str())
        .find(|l| is_recommended_label(l));
    let Some(sel) = r.answers.get(&q.question) else {
        return;
    };
    if sel.other {
        s.other_pick += 1;
    }
    if let Some(rl) = rec_label {
        s.with_recommendation += 1;
        if !sel.other && sel.label == rl {
            s.recommended_hit += 1;
        }
    }
}

fn accumulate_first_q_fleet_ask(s: &mut DecisionTypeStats, r: &FleetAskRecord) {
    let Some(q) = r.questions.first() else {
        return;
    };
    // Form-only / html-only cards have no options; skip pick classification.
    if q.options.is_empty() {
        return;
    }
    let Some(ans) = r.answers.get(&q.question) else {
        return;
    };
    // The fleet-ask answer map stores the picked option's label verbatim;
    // anything not matching a listed option came through the "Other" input.
    let matched_option = q.options.iter().any(|o| o.label == *ans);
    if !matched_option {
        s.other_pick += 1;
    }
    if let Some(rl) = q
        .options
        .iter()
        .map(|o| o.label.as_str())
        .find(|l| is_recommended_label(l))
    {
        s.with_recommendation += 1;
        if ans == rl {
            s.recommended_hit += 1;
        }
    }
}

/// Fold one record into `stats` if its `requestedAt` local date matches `date`.
/// `user-prompt` records are ignored. Public within the crate for unit tests.
fn accumulate_record(stats: &mut DecisionCardStats, rec: &DecisionHistoryRecord, date: &str) {
    let (type_key, requested_at, resolved_at) = match rec {
        DecisionHistoryRecord::Elicitation(r) => {
            ("elicitation", &r.requested_at, &r.resolved_at)
        }
        DecisionHistoryRecord::FleetAsk(r) => ("fleet-ask", &r.requested_at, &r.resolved_at),
        DecisionHistoryRecord::PlanApproval(r) => {
            ("plan-approval", &r.requested_at, &r.resolved_at)
        }
        DecisionHistoryRecord::UserPrompt(_) => return,
    };
    if local_date_of(requested_at).as_deref() != Some(date) {
        return;
    }
    let s = stats.by_type.entry(type_key.to_string()).or_default();
    s.triggered += 1;

    match rec {
        DecisionHistoryRecord::Elicitation(r) => match r.outcome {
            ElicitationOutcome::Answered => {
                s.answered += 1;
                add_latency(s, requested_at, resolved_at);
                accumulate_first_q_elicitation(s, r);
            }
            ElicitationOutcome::Declined => s.declined += 1,
            ElicitationOutcome::HeartbeatLost => s.heartbeat_lost += 1,
            ElicitationOutcome::Timeout => s.timeout += 1,
        },
        DecisionHistoryRecord::FleetAsk(r) => match r.outcome {
            FleetAskOutcome::Answered => {
                s.answered += 1;
                add_latency(s, requested_at, resolved_at);
                accumulate_first_q_fleet_ask(s, r);
            }
            FleetAskOutcome::Cancelled => s.declined += 1,
            FleetAskOutcome::HeartbeatLost => s.heartbeat_lost += 1,
            FleetAskOutcome::Timeout => s.timeout += 1,
        },
        DecisionHistoryRecord::PlanApproval(r) => match r.outcome {
            PlanApprovalOutcome::Approved | PlanApprovalOutcome::ApprovedWithEdits => {
                s.answered += 1;
                add_latency(s, requested_at, resolved_at);
            }
            PlanApprovalOutcome::Rejected => s.declined += 1,
            PlanApprovalOutcome::HeartbeatLost => s.heartbeat_lost += 1,
            PlanApprovalOutcome::Timeout => s.timeout += 1,
        },
        DecisionHistoryRecord::UserPrompt(_) => {}
    }
}

/// Visit every decision-history record that could belong to `date`, calling
/// `f` on each parsed record. Callers apply their own per-record date filter.
///
/// Files whose mtime is strictly before the start of `date` are skipped: since
/// records are only ever appended and `resolvedAt >= requestedAt`, such a file
/// cannot hold a card requested on `date`. This keeps the scan cheap even with
/// thousands of per-session history files.
fn for_each_record_on_date(date: &str, mut f: impl FnMut(&DecisionHistoryRecord)) {
    use chrono::TimeZone;

    let Some(dir) = history_dir() else {
        return;
    };

    // Compute the start-of-day SystemTime for mtime pruning.
    let start_sys: Option<std::time::SystemTime> = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).single())
        .and_then(|dt| {
            let secs = dt.timestamp();
            (secs >= 0).then(|| {
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64)
            })
        });

    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(start) = start_sys {
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime < start {
                        continue;
                    }
                }
            }
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(rec) = serde_json::from_str::<DecisionHistoryRecord>(line) {
                f(&rec);
            }
        }
    }
}

/// Scan `~/.fleet/decision-history/*.jsonl` and aggregate every decision card
/// whose `requestedAt` falls on `date` (local calendar day).
pub fn compute_stats_for_date(date: &str) -> DecisionCardStats {
    let mut stats = DecisionCardStats::default();
    for_each_record_on_date(date, |rec| accumulate_record(&mut stats, rec, date));
    stats
}

/// A single decision card where the user rejected the AI's offered choices —
/// either by typing a free-text answer via "Other" (elicitation / fleet-ask)
/// or by rejecting a proposed plan. High-signal evidence that the AI misjudged
/// how to frame the decision. Fed into the daily lessons generator.
#[derive(Clone, Debug, PartialEq)]
pub struct OtherPickContext {
    /// "elicitation" | "fleet-ask" | "plan-approval".
    pub card_type: String,
    pub workspace_name: String,
    pub session_id: String,
    /// The question / decision prompt the AI raised.
    pub question: String,
    /// The options the AI offered, each rendered as `label — description`;
    /// the recommended option (if any) keeps its `(Recommended)` marker.
    /// Empty for plan-approval rejections.
    pub options: Vec<String>,
    /// What the user actually chose or typed instead (or their rejection
    /// feedback for a plan). Empty when the user gave no free text.
    pub user_choice: String,
}

/// Collect the day's "Other"-answered elicitation/fleet-ask questions and
/// rejected plan-approvals as `OtherPickContext` evidence, oldest files first.
/// Bounded to `max` contexts so the lessons prompt stays within budget.
pub fn collect_other_picks_for_date(date: &str, max: usize) -> Vec<OtherPickContext> {
    let mut out: Vec<OtherPickContext> = Vec::new();
    for_each_record_on_date(date, |rec| {
        if out.len() >= max {
            return;
        }
        match rec {
            DecisionHistoryRecord::Elicitation(r) => {
                if r.outcome != ElicitationOutcome::Answered {
                    return;
                }
                if local_date_of(&r.requested_at).as_deref() != Some(date) {
                    return;
                }
                for q in &r.questions {
                    let Some(sel) = r.answers.get(&q.question) else {
                        continue;
                    };
                    if !sel.other {
                        continue;
                    }
                    out.push(OtherPickContext {
                        card_type: "elicitation".into(),
                        workspace_name: r.workspace_name.clone(),
                        session_id: r.session_id.clone(),
                        question: q.question.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| format!("{} — {}", o.label, o.description))
                            .collect(),
                        user_choice: sel.label.clone(),
                    });
                    if out.len() >= max {
                        break;
                    }
                }
            }
            DecisionHistoryRecord::FleetAsk(r) => {
                if r.outcome != FleetAskOutcome::Answered {
                    return;
                }
                if local_date_of(&r.requested_at).as_deref() != Some(date) {
                    return;
                }
                for q in &r.questions {
                    if q.options.is_empty() {
                        continue;
                    }
                    let Some(ans) = r.answers.get(&q.question) else {
                        continue;
                    };
                    if q.options.iter().any(|o| o.label == *ans) {
                        continue; // picked a listed option, not "Other"
                    }
                    out.push(OtherPickContext {
                        card_type: "fleet-ask".into(),
                        workspace_name: r.workspace_name.clone(),
                        session_id: r.session_id.clone(),
                        question: q.question.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| format!("{} — {}", o.label, o.description))
                            .collect(),
                        user_choice: ans.clone(),
                    });
                    if out.len() >= max {
                        break;
                    }
                }
            }
            DecisionHistoryRecord::PlanApproval(r) => {
                if r.outcome != PlanApprovalOutcome::Rejected {
                    return;
                }
                if local_date_of(&r.requested_at).as_deref() != Some(date) {
                    return;
                }
                let excerpt: String = r.plan_content.chars().take(300).collect();
                out.push(OtherPickContext {
                    card_type: "plan-approval".into(),
                    workspace_name: r.workspace_name.clone(),
                    session_id: r.session_id.clone(),
                    question: format!("Proposed plan (rejected):\n{excerpt}"),
                    options: Vec::new(),
                    user_choice: r.feedback.clone().unwrap_or_default(),
                });
            }
            DecisionHistoryRecord::UserPrompt(_) => {}
        }
    });
    out
}

// ── Storage ──────────────────────────────────────────────────────────────────

fn history_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("decision-history"))
}

fn history_file(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || session_id.contains('/') || session_id.contains('\\') {
        return None;
    }
    history_dir().map(|d| d.join(format!("{session_id}.jsonl")))
}

/// Append a record to the per-session JSONL file.
///
/// The record and its trailing newline go out in a **single** `write_all`. Two
/// writes (payload, then `\n`) let a concurrent appender — another thread, or
/// the `fleet` CLI in another process — slip its bytes in between, producing a
/// line like `{…}{…}\n` that `read_persisted_records` then discards as
/// malformed. One `O_APPEND` write keeps each record intact.
pub fn append_record(record: &DecisionHistoryRecord) -> Result<(), String> {
    let dir = history_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create decision-history dir: {e}"))?;
    let path = history_file(record.session_id())
        .ok_or_else(|| format!("invalid session_id: {:?}", record.session_id()))?;
    let mut line = serde_json::to_string(record).map_err(|e| format!("serialize: {e}"))?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| format!("append: {e}"))
}

/// Read all records for a session, oldest-first. Missing file → empty Vec.
/// Malformed lines are skipped (logged via `eprintln!`) so a single corrupt
/// record can't take out the whole session view.
fn read_persisted_records(session_id: &str) -> Vec<DecisionHistoryRecord> {
    let Some(path) = history_file(session_id) else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str::<DecisionHistoryRecord>(l) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!(
                    "decision_history: skipping malformed line in {}: {e}",
                    path.display()
                );
                None
            }
        })
        .collect()
}

/// Public legacy entry point: returns persisted records only, no jsonl sync.
/// Prefer `list_session_records_with_jsonl` from caller code that has a path.
pub fn list_session_records(session_id: &str) -> Vec<DecisionHistoryRecord> {
    let mut records = read_persisted_records(session_id);
    records.sort_by(|a, b| record_sort_ts(a).cmp(record_sort_ts(b)));
    records
}

/// Returns persisted records merged with user prompts extracted from the
/// session's jsonl, sorted oldest-first by timestamp. The jsonl-derived
/// `UserPrompt` records are also persisted (de-duped by entry uuid) so
/// future reads have them available even if the jsonl is rotated.
pub fn list_session_records_with_jsonl(
    session_id: &str,
    jsonl_path: Option<&Path>,
) -> Vec<DecisionHistoryRecord> {
    if let Some(path) = jsonl_path {
        if let Err(e) = sync_user_prompts_from_jsonl(session_id, path) {
            eprintln!("decision_history: jsonl sync failed for {session_id}: {e}");
        }
    }
    let mut records = read_persisted_records(session_id);
    records.sort_by(|a, b| record_sort_ts(a).cmp(record_sort_ts(b)));
    records
}

/// Used to order a heterogeneous record list on a single timeline.
fn record_sort_ts(r: &DecisionHistoryRecord) -> &str {
    match r {
        DecisionHistoryRecord::Elicitation(e) => &e.requested_at,
        DecisionHistoryRecord::PlanApproval(p) => &p.requested_at,
        DecisionHistoryRecord::UserPrompt(u) => &u.sent_at,
        DecisionHistoryRecord::FleetAsk(f) => &f.requested_at,
    }
}

// ── Session JSONL → UserPrompt extraction ───────────────────────────────────

/// Guards the read-then-append inside [`sync_user_prompts_from_jsonl`] so
/// concurrent callers in this process can't each observe the same prompt as
/// missing and append it twice. Cross-process appenders are still possible, but
/// `append_record`'s single-write contract keeps their lines intact.
static SYNC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Prefixes that mark a user-role text block as auto-injected (not actually
/// typed by the user). When a content block's stripped text starts with one
/// of these, that block is dropped from the prompt.
const INJECTED_TEXT_PREFIXES: &[&str] = &["<ide_opened_file>", "<ide_selection>"];

/// Scan `jsonl_path` for real user prompts and append any not-yet-seen ones
/// to `~/.fleet/decision-history/<session_id>.jsonl` as `UserPrompt` records.
/// Existing record uuids are loaded once and used for de-dup.
pub fn sync_user_prompts_from_jsonl(
    session_id: &str,
    jsonl_path: &Path,
) -> Result<(), String> {
    // Serialise the read-then-append below. `list_session_decisions` used to be
    // a synchronous Tauri command, so the main thread was the de-facto lock;
    // now it runs on a threadpool and two cards mounting for the same session
    // would otherwise both see a prompt as missing and append it twice.
    // Poison-tolerant: a panicking appender must not wedge the history sync.
    let _guard = SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Incremental scan: on the first sync of a session we read the whole
    // transcript, but each later sync (every SessionDetail open, every decision
    // poll) resumes from the byte offset where the previous scan stopped and
    // reads only the lines appended since. A 50 MB transcript was otherwise
    // re-read and re-parsed in full on every open (~35 ms, under the global
    // lock). The offset only ever advances past *complete* lines, so it can
    // never skip an unprocessed prompt; if the file is shorter than the cached
    // offset (rotated / a different session reusing the id) the scan resets to
    // the start. The `existing`-id de-dup below stays as a correctness backstop
    // independent of the offset.
    let start_offset = cached_sync_offset(session_id, jsonl_path);
    let (content, new_offset) = match read_appended_lines(jsonl_path, start_offset) {
        Ok(pair) => pair,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("read {}: {e}", jsonl_path.display())),
    };

    let existing: HashSet<String> = read_persisted_records(session_id)
        .iter()
        .map(|r| r.id().to_string())
        .collect();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(rec) = parse_user_prompt_line(trimmed, session_id) else {
            continue;
        };
        if existing.contains(&rec.id) {
            continue;
        }
        // Best-effort append; failures are logged but do not abort the scan
        // (a single I/O error shouldn't lose every later record).
        if let Err(e) = append_record(&DecisionHistoryRecord::UserPrompt(rec)) {
            eprintln!("decision_history: append user prompt failed: {e}");
        }
    }
    store_sync_offset(session_id, jsonl_path, new_offset);
    Ok(())
}

/// Per-session byte offset into the transcript where the last
/// [`sync_user_prompts_from_jsonl`] scan stopped, so the next scan reads only
/// what was appended since. Keyed by `session_id`; the stored path guards
/// against a stale offset if the same id ever maps to a different file.
/// In-memory only — after a process restart the first sync re-reads in full,
/// which the id de-dup makes harmless.
static SYNC_OFFSETS: LazyLock<Mutex<HashMap<String, (PathBuf, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The cached offset to resume from, or 0 when this session hasn't been scanned
/// in this process or the cached entry was for a different path.
fn cached_sync_offset(session_id: &str, jsonl_path: &Path) -> u64 {
    let map = SYNC_OFFSETS.lock().unwrap_or_else(|e| e.into_inner());
    match map.get(session_id) {
        Some((path, off)) if path == jsonl_path => *off,
        _ => 0,
    }
}

fn store_sync_offset(session_id: &str, jsonl_path: &Path, offset: u64) {
    let mut map = SYNC_OFFSETS.lock().unwrap_or_else(|e| e.into_inner());
    map.insert(session_id.to_string(), (jsonl_path.to_path_buf(), offset));
}

/// Read the transcript from `offset` to EOF, returning **all** appended text
/// (for parsing) together with the offset to resume from next time.
///
/// The returned text includes a trailing line with no final newline — matching
/// the old `read_to_string().lines()`, which processed a last unterminated line
/// too, so a finished transcript's closing prompt is never dropped. The resume
/// offset, however, only advances **past the last newline**: an unterminated
/// trailing line (the CLI mid-write, or a genuinely final line) is therefore
/// re-read on the next scan, where the id de-dup makes reprocessing a no-op.
/// This keeps the offset from ever skipping a line that later gains content.
///
/// If `offset` is past the current end (file truncated or replaced) the read
/// restarts from 0. No appended bytes yields an empty string and an unchanged
/// offset.
fn read_appended_lines(path: &Path, offset: u64) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let start = if offset <= size { offset } else { 0 };
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let resume = match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => start + (i + 1) as u64,
        None => start,
    };
    let text = String::from_utf8_lossy(&buf).into_owned();
    Ok((text, resume))
}

/// Parse a single jsonl line into a `UserPromptRecord`, applying the filter
/// rules. Returns `None` for lines that do not represent user-typed input.
fn parse_user_prompt_line(line: &str, session_id: &str) -> Option<UserPromptRecord> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "user" {
        return None;
    }
    if v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    if v.get("isCompactSummary").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    if v.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    let msg = v.get("message")?;
    if msg.get("role")?.as_str()? != "user" {
        return None;
    }
    let content = msg.get("content")?;
    let mut texts: Vec<String> = Vec::new();
    let mut has_image = false;
    // Keep one text block unless it's purely IDE-injected context.
    let mut push_text = |text: &str| {
        let stripped = text.trim_start();
        if stripped.is_empty() {
            return;
        }
        if INJECTED_TEXT_PREFIXES
            .iter()
            .any(|p| stripped.starts_with(p))
        {
            return;
        }
        texts.push(text.to_string());
    };
    // The session's first typed prompt is stored as a bare string; later user
    // turns (tool_results, pasted images) use the block-array form.
    if let Some(text) = content.as_str() {
        push_text(text);
    } else if let Some(blocks) = content.as_array() {
        for block in blocks {
            let kind = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match kind {
                "text" => {
                    let text = block.get("text").and_then(|x| x.as_str()).unwrap_or("");
                    push_text(text);
                }
                "image" => {
                    has_image = true;
                }
                // tool_result and others: not user-typed, drop.
                _ => {}
            }
        }
    } else {
        return None;
    }
    drop(push_text);
    if texts.is_empty() && !has_image {
        return None;
    }
    let id = v.get("uuid").and_then(|x| x.as_str())?.to_string();
    let sent_at = v
        .get("timestamp")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(UserPromptRecord {
        id,
        session_id: session_id.to_string(),
        text: texts.join("\n\n"),
        has_image,
        sent_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};

    fn tmp_jsonl(name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir()
            .join(format!("dh_incr_{}_{}.jsonl", name, std::process::id()));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn incremental_read_from_zero_returns_all_lines() {
        let p = tmp_jsonl("all", b"line1\nline2\nline3\n");
        let (text, off) = read_appended_lines(&p, 0).unwrap();
        assert_eq!(text, "line1\nline2\nline3\n");
        assert_eq!(off, 18);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn incremental_read_resumes_and_sees_only_appended() {
        let p = tmp_jsonl("resume", b"a\nb\n");
        let (_t1, off1) = read_appended_lines(&p, 0).unwrap();
        assert_eq!(off1, 4);
        // Append more, resume from the previous offset.
        {
            let mut f = OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(b"c\nd\n").unwrap();
        }
        let (t2, off2) = read_appended_lines(&p, off1).unwrap();
        assert_eq!(t2, "c\nd\n");
        assert_eq!(off2, 8);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn incremental_read_returns_final_unterminated_line_but_offset_stops_before_it() {
        // No trailing newline: the last line is still returned for parsing
        // (a finished transcript's closing prompt must not be dropped), but the
        // resume offset stops before it so the next scan re-reads it.
        let p = tmp_jsonl("partial", b"whole\npart");
        let (text, off) = read_appended_lines(&p, 0).unwrap();
        assert_eq!(text, "whole\npart");
        assert_eq!(off, 6, "resume offset stops before the unterminated 'part'");
        // Finish the line; next read from `off` picks up just the completion.
        {
            let mut f = OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(b"ial\n").unwrap();
        }
        let (t2, off2) = read_appended_lines(&p, off).unwrap();
        assert_eq!(t2, "partial\n");
        assert_eq!(off2, 14);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn incremental_read_resets_when_file_shorter_than_offset() {
        let p = tmp_jsonl("trunc", b"x\ny\n");
        // Cached offset points past a now-shorter file → restart from 0.
        let (text, off) = read_appended_lines(&p, 9999).unwrap();
        assert_eq!(text, "x\ny\n");
        assert_eq!(off, 4);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn incremental_read_no_new_bytes_is_empty() {
        let p = tmp_jsonl("noop", b"a\nb\n");
        let (text, off) = read_appended_lines(&p, 4).unwrap();
        assert_eq!(text, "");
        assert_eq!(off, 4);
        std::fs::remove_file(p).ok();
    }

    // real_home_dir() reads $FLEET_HOME, so tests must serialize and override it.
    // Uses the crate-wide `crate::session::fleet_home_lock` so tests in
    // different modules don't race on the global env. The shared lock is
    // poison-tolerant: panics inside the critical section don't cascade.

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: tests serialize via FLEET_HOME_LOCK
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }

    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                if let Some(p) = &self.prev {
                    std::env::set_var("FLEET_HOME", p);
                } else {
                    std::env::remove_var("FLEET_HOME");
                }
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "decision-history-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn sample_request(session_id: &str, id: &str) -> ElicitationRequest {
        ElicitationRequest {
            parked: false,
            id: id.into(),
            session_id: session_id.into(),
            workspace_name: "claude-fleet".into(),
            ai_title: Some("test session".into()),
            timestamp: "2026-04-28T00:00:00Z".into(),
            questions: vec![ElicitationQuestion {
                question: "Pick one?".into(),
                header: "Pick".into(),
                multi_select: false,
                options: vec![
                    ElicitationOption {
                        label: "A".into(),
                        description: "the first".into(),
                        preview: None,
                    },
                    ElicitationOption {
                        label: "B".into(),
                        description: "the second".into(),
                        preview: None,
                    },
                ],
            }],
        }
    }

    #[test]
    fn answered_record_enriches_with_description() {
        let req = sample_request("s1", "req1");
        let mut answers = HashMap::new();
        answers.insert("Pick one?".into(), "A".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T00:00:01Z".into(),
        );
        let sel = rec.answers.get("Pick one?").unwrap();
        assert_eq!(sel.label, "A");
        assert_eq!(sel.description.as_deref(), Some("the first"));
        assert!(!sel.other);
    }

    #[test]
    fn other_answer_is_flagged() {
        let req = sample_request("s1", "req2");
        let mut answers = HashMap::new();
        answers.insert("Pick one?".into(), "C — typed by user".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T00:00:01Z".into(),
        );
        let sel = rec.answers.get("Pick one?").unwrap();
        assert_eq!(sel.label, "C — typed by user");
        assert!(sel.description.is_none());
        assert!(sel.other);
    }

    #[test]
    fn declined_record_has_empty_answers() {
        let req = sample_request("s1", "req3");
        let answers = HashMap::new();
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Declined,
            &answers,
            "2026-04-28T00:00:01Z".into(),
        );
        assert!(rec.answers.is_empty());
        assert_eq!(rec.outcome, ElicitationOutcome::Declined);
    }

    #[test]
    fn append_then_list_roundtrips() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("roundtrip");
        let _home = FleetHomeOverride::new(&tmp);

        let req = sample_request("session-xyz", "req-1");
        let mut answers = HashMap::new();
        answers.insert("Pick one?".into(), "B".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T00:00:01Z".into(),
        );
        append_record(&DecisionHistoryRecord::Elicitation(rec)).unwrap();

        let req2 = sample_request("session-xyz", "req-2");
        let rec2 = build_elicitation_record(
            &req2,
            ElicitationOutcome::Timeout,
            &HashMap::new(),
            "2026-04-28T00:00:02Z".into(),
        );
        append_record(&DecisionHistoryRecord::Elicitation(rec2)).unwrap();

        let records = list_session_records("session-xyz");
        assert_eq!(records.len(), 2);
        match &records[0] {
            DecisionHistoryRecord::Elicitation(r) => {
                assert_eq!(r.id, "req-1");
                assert_eq!(r.outcome, ElicitationOutcome::Answered);
                assert_eq!(r.answers.get("Pick one?").unwrap().label, "B");
            }
            _ => panic!("expected elicitation"),
        }
        match &records[1] {
            DecisionHistoryRecord::Elicitation(r) => {
                assert_eq!(r.outcome, ElicitationOutcome::Timeout);
            }
            _ => panic!("expected elicitation"),
        }
    }

    #[test]
    fn invalid_session_id_rejected() {
        assert!(history_file("").is_none());
        assert!(history_file("a/b").is_none());
        assert!(history_file("a\\b").is_none());
        assert!(history_file("ok-id").is_some());
    }

    #[test]
    fn parse_user_prompt_keeps_real_text() {
        let line = r#"{"type":"user","isSidechain":false,"uuid":"u1","timestamp":"2026-04-28T00:00:00Z","message":{"role":"user","content":[{"type":"text","text":"hi boss"}]}}"#;
        let rec = parse_user_prompt_line(line, "ssn").unwrap();
        assert_eq!(rec.id, "u1");
        assert_eq!(rec.text, "hi boss");
        assert!(!rec.has_image);
    }

    #[test]
    fn parse_user_prompt_drops_ide_injection() {
        let line = r#"{"type":"user","uuid":"u2","timestamp":"t","message":{"role":"user","content":[{"type":"text","text":"<ide_selection>file foo.rs</ide_selection>"}]}}"#;
        assert!(parse_user_prompt_line(line, "ssn").is_none());
    }

    #[test]
    fn parse_user_prompt_drops_compact_summary() {
        let line = r#"{"type":"user","isCompactSummary":true,"uuid":"u3","timestamp":"t","message":{"role":"user","content":[{"type":"text","text":"summary..."}]}}"#;
        assert!(parse_user_prompt_line(line, "ssn").is_none());
    }

    #[test]
    fn parse_user_prompt_drops_sidechain() {
        let line = r#"{"type":"user","isSidechain":true,"uuid":"u4","timestamp":"t","message":{"role":"user","content":[{"type":"text","text":"task for subagent"}]}}"#;
        assert!(parse_user_prompt_line(line, "ssn").is_none());
    }

    #[test]
    fn parse_user_prompt_drops_tool_result_only() {
        let line = r#"{"type":"user","uuid":"u5","timestamp":"t","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":[]}]}}"#;
        assert!(parse_user_prompt_line(line, "ssn").is_none());
    }

    #[test]
    fn parse_user_prompt_keeps_text_with_image() {
        let line = r#"{"type":"user","uuid":"u6","timestamp":"t","message":{"role":"user","content":[{"type":"image","source":{}},{"type":"text","text":"看这个截图"}]}}"#;
        let rec = parse_user_prompt_line(line, "ssn").unwrap();
        assert!(rec.has_image);
        assert_eq!(rec.text, "看这个截图");
    }

    #[test]
    fn parse_user_prompt_keeps_text_alongside_injected_blocks() {
        // Real text + injected ide_opened_file block: the prompt survives,
        // and only the injected block is stripped out of the joined text.
        let line = r#"{"type":"user","uuid":"u7","timestamp":"t","message":{"role":"user","content":[{"type":"text","text":"<ide_opened_file>...injected..."},{"type":"text","text":"actual user words"}]}}"#;
        let rec = parse_user_prompt_line(line, "ssn").unwrap();
        assert_eq!(rec.text, "actual user words");
    }

    #[test]
    fn parse_user_prompt_keeps_string_content() {
        // The session's very first typed prompt is stored by Claude Code with
        // `message.content` as a bare string, not a block array. That opening
        // question must still surface in the decision timeline.
        let line = r#"{"type":"user","uuid":"u8","timestamp":"2026-07-15T00:00:00Z","message":{"role":"user","content":"现在应用对与codex，能支持决策卡么？"}}"#;
        let rec = parse_user_prompt_line(line, "ssn").unwrap();
        assert_eq!(rec.id, "u8");
        assert_eq!(rec.text, "现在应用对与codex，能支持决策卡么？");
        assert!(!rec.has_image);
    }

    #[test]
    fn parse_user_prompt_string_content_drops_ide_injection() {
        // A string-form prompt that is purely IDE-injected context is still
        // filtered out, matching the block-array behaviour.
        let line = r#"{"type":"user","uuid":"u9","timestamp":"t","message":{"role":"user","content":"<ide_selection>foo.rs</ide_selection>"}}"#;
        assert!(parse_user_prompt_line(line, "ssn").is_none());
    }

    #[test]
    fn sync_user_prompts_appends_and_dedups() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("syncprompts");
        let _home = FleetHomeOverride::new(&tmp);

        // Write a tiny fake session jsonl with two real prompts and one
        // ide_selection injection that must be skipped.
        let jsonl = tmp.join("session.jsonl");
        let body = [
            r#"{"type":"user","uuid":"u1","timestamp":"2026-04-28T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"first"}]}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2026-04-28T00:00:02Z","message":{"role":"user","content":[{"type":"text","text":"<ide_selection>noise"}]}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2026-04-28T00:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x"}]}}"#,
            r#"{"type":"user","uuid":"u4","timestamp":"2026-04-28T00:00:04Z","message":{"role":"user","content":[{"type":"text","text":"second"}]}}"#,
        ]
        .join("\n");
        fs::write(&jsonl, body).unwrap();

        sync_user_prompts_from_jsonl("ssn", &jsonl).unwrap();
        let recs = read_persisted_records("ssn");
        assert_eq!(recs.len(), 2);
        let ids: Vec<&str> = recs.iter().map(|r| r.id()).collect();
        assert!(ids.contains(&"u1"));
        assert!(ids.contains(&"u4"));

        // Second sync must not duplicate.
        sync_user_prompts_from_jsonl("ssn", &jsonl).unwrap();
        let recs2 = read_persisted_records("ssn");
        assert_eq!(recs2.len(), 2);
    }

    #[test]
    fn list_with_jsonl_merges_sorted() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("merge");
        let _home = FleetHomeOverride::new(&tmp);

        // Persist a decision card with timestamp BETWEEN two user prompts.
        let req = sample_request("ssn", "card-1");
        let mut answers = HashMap::new();
        answers.insert("Pick one?".into(), "A".into());
        let mut rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T00:00:03Z".into(),
        );
        rec.requested_at = "2026-04-28T00:00:02Z".into();
        append_record(&DecisionHistoryRecord::Elicitation(rec)).unwrap();

        // jsonl with two user prompts surrounding the card.
        let jsonl = tmp.join("session.jsonl");
        let body = [
            r#"{"type":"user","uuid":"u-before","timestamp":"2026-04-28T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"before"}]}}"#,
            r#"{"type":"user","uuid":"u-after","timestamp":"2026-04-28T00:00:05Z","message":{"role":"user","content":[{"type":"text","text":"after"}]}}"#,
        ]
        .join("\n");
        fs::write(&jsonl, body).unwrap();

        let merged = list_session_records_with_jsonl("ssn", Some(&jsonl));
        assert_eq!(merged.len(), 3);
        // Sorted ascending by timestamp.
        assert_eq!(merged[0].id(), "u-before");
        assert_eq!(merged[1].id(), "card-1");
        assert_eq!(merged[2].id(), "u-after");
    }

    #[test]
    fn malformed_line_is_skipped() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("malformed");
        let _home = FleetHomeOverride::new(&tmp);

        let req = sample_request("ssn", "req-1");
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Declined,
            &HashMap::new(),
            "2026-04-28T00:00:01Z".into(),
        );
        append_record(&DecisionHistoryRecord::Elicitation(rec)).unwrap();

        // Inject a malformed line.
        let path = history_file("ssn").unwrap();
        let mut existing = fs::read_to_string(&path).unwrap();
        existing.push_str("not-json\n");
        fs::write(&path, existing).unwrap();

        let records = list_session_records("ssn");
        assert_eq!(records.len(), 1);
    }

    // ── FleetAsk record fixtures + tests ─────────────────────────────────

    fn sample_fleet_ask_request(session_id: &str, id: &str) -> FleetAskRequest {
        use crate::mcp_ipc::{FleetAskOption, FormFieldKind};
        FleetAskRequest {
            parked: false,
            id: id.into(),
            session_id: session_id.into(),
            workspace_name: "claude-fleet".into(),
            ai_title: Some("v2 test".into()),
            timestamp: "2026-05-28T00:00:00Z".into(),
            questions: vec![FleetAskQuestion {
                question: "Pick or fill?".into(),
                header: "Mix".into(),
                multi_select: false,
                options: vec![FleetAskOption {
                    label: "A".into(),
                    description: "the first".into(),
                    preview: None,
                }],
                html: Some("<p>preview</p>".into()),
                form_fields: vec![crate::mcp_ipc::FleetAskFormField {
                    name: "note".into(),
                    kind: FormFieldKind::Text,
                    label: "Note".into(),
                    placeholder: None,
                    options: vec![],
                    required: false,
                    default: None,
                    min: None,
                    max: None,
                    step: None,
                }],
                images: vec![],
            }],
        }
    }

    #[test]
    fn fleet_ask_record_round_trip_preserves_html_and_form_fields() {
        let req = sample_fleet_ask_request("ssn", "card-7");
        let mut answers = BTreeMap::new();
        answers.insert("Pick or fill?".into(), "A".into());
        answers.insert("note".into(), "hello world".into());
        let rec = build_fleet_ask_record(
            &req,
            FleetAskOutcome::Answered,
            answers,
            "2026-05-28T00:00:05Z".into(),
        );
        let wrapped = DecisionHistoryRecord::FleetAsk(rec);
        let line = serde_json::to_string(&wrapped).unwrap();
        let back: DecisionHistoryRecord = serde_json::from_str(&line).unwrap();
        match back {
            DecisionHistoryRecord::FleetAsk(r) => {
                assert_eq!(r.id, "card-7");
                assert_eq!(r.outcome, FleetAskOutcome::Answered);
                assert_eq!(r.answers.get("note"), Some(&"hello world".to_string()));
                assert_eq!(r.questions[0].html.as_deref(), Some("<p>preview</p>"));
                assert_eq!(r.questions[0].form_fields.len(), 1);
            }
            other => panic!("expected FleetAsk variant, got {other:?}"),
        }
    }

    #[test]
    fn fleet_ask_record_drops_answers_on_non_answered_outcome() {
        let req = sample_fleet_ask_request("ssn", "card-8");
        let mut answers = BTreeMap::new();
        answers.insert("Pick or fill?".into(), "A".into());
        let rec = build_fleet_ask_record(
            &req,
            FleetAskOutcome::Cancelled,
            answers.clone(),
            "2026-05-28T00:00:06Z".into(),
        );
        assert!(rec.answers.is_empty(), "cancelled should not retain answers");

        let rec_hbl = build_fleet_ask_record(
            &req,
            FleetAskOutcome::HeartbeatLost,
            answers.clone(),
            "2026-05-28T00:00:07Z".into(),
        );
        assert!(rec_hbl.answers.is_empty());

        let rec_to = build_fleet_ask_record(
            &req,
            FleetAskOutcome::Timeout,
            answers,
            "2026-05-28T00:00:08Z".into(),
        );
        assert!(rec_to.answers.is_empty());
    }

    #[test]
    fn fleet_ask_record_persists_and_lists() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("fleet-ask-persist");
        let _home = FleetHomeOverride::new(&tmp);

        let req = sample_fleet_ask_request("ssn-v2", "card-9");
        let mut answers = BTreeMap::new();
        answers.insert("Pick or fill?".into(), "A".into());
        let rec = build_fleet_ask_record(
            &req,
            FleetAskOutcome::Answered,
            answers,
            "2026-05-28T00:00:09Z".into(),
        );
        append_record(&DecisionHistoryRecord::FleetAsk(rec)).unwrap();

        let listed = list_session_records("ssn-v2");
        assert_eq!(listed.len(), 1);
        match &listed[0] {
            DecisionHistoryRecord::FleetAsk(r) => assert_eq!(r.id, "card-9"),
            other => panic!("expected FleetAsk, got {other:?}"),
        }
    }

    #[test]
    fn fleet_ask_record_mixes_with_other_kinds_on_timeline() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("fleet-ask-mixed");
        let _home = FleetHomeOverride::new(&tmp);

        // v1 elicitation first, then a v2 fleet-ask later — both should
        // come back oldest-first from list_session_records().
        let elic_req = sample_request("ssn-mix", "elic-1");
        let elic_rec = build_elicitation_record(
            &elic_req,
            ElicitationOutcome::Declined,
            &HashMap::new(),
            "2026-05-28T00:00:01Z".into(),
        );
        append_record(&DecisionHistoryRecord::Elicitation(elic_rec)).unwrap();

        let fa_req = FleetAskRequest {
            parked: false,
            id: "fa-1".into(),
            session_id: "ssn-mix".into(),
            workspace_name: "claude-fleet".into(),
            ai_title: None,
            timestamp: "2026-05-28T00:00:02Z".into(),
            questions: vec![],
        };
        let fa_rec = build_fleet_ask_record(
            &fa_req,
            FleetAskOutcome::Cancelled,
            BTreeMap::new(),
            "2026-05-28T00:00:03Z".into(),
        );
        append_record(&DecisionHistoryRecord::FleetAsk(fa_rec)).unwrap();

        let listed = list_session_records("ssn-mix");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id(), "elic-1");
        assert_eq!(listed[1].id(), "fa-1");
    }

    // ── Decision-card stats ──────────────────────────────────────────────

    /// Build an elicitation request whose first option is explicitly marked
    /// `(Recommended)`.
    fn recommended_request(session_id: &str, id: &str) -> ElicitationRequest {
        ElicitationRequest {
            parked: false,
            id: id.into(),
            session_id: session_id.into(),
            workspace_name: "claude-fleet".into(),
            ai_title: None,
            timestamp: "2026-04-28T02:00:00Z".into(),
            questions: vec![ElicitationQuestion {
                question: "Which approach?".into(),
                header: "Approach".into(),
                multi_select: false,
                options: vec![
                    ElicitationOption {
                        label: "Do it inline (Recommended)".into(),
                        description: "fast".into(),
                        preview: None,
                    },
                    ElicitationOption {
                        label: "Refactor first".into(),
                        description: "clean".into(),
                        preview: None,
                    },
                ],
            }],
        }
    }

    /// Accumulate a record using the day derived from its own requestedAt so
    /// the local-timezone date filter always matches, regardless of the host
    /// timezone the test runs in.
    fn accumulate_self(stats: &mut DecisionCardStats, rec: &DecisionHistoryRecord) {
        let ts = match rec {
            DecisionHistoryRecord::Elicitation(r) => &r.requested_at,
            DecisionHistoryRecord::FleetAsk(r) => &r.requested_at,
            DecisionHistoryRecord::PlanApproval(r) => &r.requested_at,
            DecisionHistoryRecord::UserPrompt(u) => &u.sent_at,
        };
        let date = local_date_of(ts).expect("parse date");
        accumulate_record(stats, rec, &date);
    }

    #[test]
    fn stats_count_recommended_hit() {
        let req = recommended_request("s", "c1");
        let mut answers = HashMap::new();
        answers.insert("Which approach?".into(), "Do it inline (Recommended)".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T02:00:05Z".into(),
        );
        let mut stats = DecisionCardStats::default();
        accumulate_self(&mut stats, &DecisionHistoryRecord::Elicitation(rec));
        let s = &stats.by_type["elicitation"];
        assert_eq!(s.triggered, 1);
        assert_eq!(s.answered, 1);
        assert_eq!(s.with_recommendation, 1);
        assert_eq!(s.recommended_hit, 1);
        assert_eq!(s.other_pick, 0);
        assert_eq!(s.latency_count, 1);
        assert!((s.latency_secs_sum - 5.0).abs() < 0.001);
    }

    #[test]
    fn stats_count_other_pick_over_recommended() {
        let req = recommended_request("s", "c2");
        let mut answers = HashMap::new();
        // User typed something not in the option list → "Other".
        answers.insert("Which approach?".into(), "Just delete the whole module".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T02:00:03Z".into(),
        );
        let mut stats = DecisionCardStats::default();
        accumulate_self(&mut stats, &DecisionHistoryRecord::Elicitation(rec));
        let s = &stats.by_type["elicitation"];
        assert_eq!(s.answered, 1);
        assert_eq!(s.with_recommendation, 1, "card offered a recommended option");
        assert_eq!(s.recommended_hit, 0);
        assert_eq!(s.other_pick, 1);
    }

    #[test]
    fn stats_no_recommendation_denominator_zero() {
        // sample_request has two plain options, neither marked (Recommended).
        let req = sample_request("s", "c3");
        let mut answers = HashMap::new();
        answers.insert("Pick one?".into(), "A".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T00:00:02Z".into(),
        );
        let mut stats = DecisionCardStats::default();
        accumulate_self(&mut stats, &DecisionHistoryRecord::Elicitation(rec));
        let s = &stats.by_type["elicitation"];
        assert_eq!(s.answered, 1);
        assert_eq!(s.with_recommendation, 0);
        assert_eq!(s.recommended_hit, 0);
        assert_eq!(s.other_pick, 0);
    }

    #[test]
    fn stats_count_non_answered_outcomes() {
        let mut stats = DecisionCardStats::default();
        for (id, outcome) in [
            ("t1", ElicitationOutcome::Timeout),
            ("t2", ElicitationOutcome::Declined),
            ("t3", ElicitationOutcome::HeartbeatLost),
        ] {
            let req = sample_request("s", id);
            let rec = build_elicitation_record(
                &req,
                outcome,
                &HashMap::new(),
                "2026-04-28T00:00:02Z".into(),
            );
            accumulate_self(&mut stats, &DecisionHistoryRecord::Elicitation(rec));
        }
        let s = &stats.by_type["elicitation"];
        assert_eq!(s.triggered, 3);
        assert_eq!(s.answered, 0);
        assert_eq!(s.timeout, 1);
        assert_eq!(s.declined, 1);
        assert_eq!(s.heartbeat_lost, 1);
        assert_eq!(s.latency_count, 0);
    }

    #[test]
    fn stats_fleet_ask_matched_vs_other() {
        // Matched option → not other; recommended present → hit.
        let mut req = sample_fleet_ask_request("s", "fa1");
        req.questions[0].options = vec![
            crate::mcp_ipc::FleetAskOption {
                label: "Commit now (Recommended)".into(),
                description: "ship".into(),
                preview: None,
            },
            crate::mcp_ipc::FleetAskOption {
                label: "Hold".into(),
                description: "wait".into(),
                preview: None,
            },
        ];
        let q = req.questions[0].question.clone();

        let mut hit_ans = BTreeMap::new();
        hit_ans.insert(q.clone(), "Commit now (Recommended)".into());
        let hit = build_fleet_ask_record(
            &req,
            FleetAskOutcome::Answered,
            hit_ans,
            "2026-05-28T00:00:04Z".into(),
        );

        let mut other_ans = BTreeMap::new();
        other_ans.insert(q, "actually revert everything".into());
        let other = build_fleet_ask_record(
            &req,
            FleetAskOutcome::Answered,
            other_ans,
            "2026-05-28T00:00:04Z".into(),
        );

        let mut stats = DecisionCardStats::default();
        accumulate_self(&mut stats, &DecisionHistoryRecord::FleetAsk(hit));
        accumulate_self(&mut stats, &DecisionHistoryRecord::FleetAsk(other));
        let s = &stats.by_type["fleet-ask"];
        assert_eq!(s.triggered, 2);
        assert_eq!(s.answered, 2);
        assert_eq!(s.with_recommendation, 2);
        assert_eq!(s.recommended_hit, 1);
        assert_eq!(s.other_pick, 1);
    }

    #[test]
    fn stats_plan_approval_outcomes() {
        let mk = |id: &str, outcome: PlanApprovalOutcome| {
            let req = PlanApprovalRequest {
                parked: false,
                id: id.into(),
                session_id: "s".into(),
                workspace_name: "claude-fleet".into(),
                ai_title: None,
                timestamp: "2026-06-01T00:00:00Z".into(),
                plan_content: "do stuff".into(),
                plan_file_path: None,
            };
            DecisionHistoryRecord::PlanApproval(build_plan_approval_record(
                &req,
                outcome,
                None,
                "2026-06-01T00:00:10Z".into(),
            ))
        };
        let mut stats = DecisionCardStats::default();
        accumulate_self(&mut stats, &mk("p1", PlanApprovalOutcome::Approved));
        accumulate_self(&mut stats, &mk("p2", PlanApprovalOutcome::Rejected));
        let s = &stats.by_type["plan-approval"];
        assert_eq!(s.triggered, 2);
        assert_eq!(s.answered, 1);
        assert_eq!(s.declined, 1);
        assert_eq!(s.with_recommendation, 0);
        assert_eq!(s.latency_count, 1);
    }

    #[test]
    fn collect_other_picks_only_grabs_other_answers() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("collect-other");
        let _home = FleetHomeOverride::new(&tmp);

        // Card 1: user typed a free-text answer via "Other" → should be collected.
        let req_other = recommended_request("sess-collect", "other-1");
        let mut a1 = HashMap::new();
        a1.insert("Which approach?".into(), "just rewrite it from scratch".into());
        let rec_other = build_elicitation_record(
            &req_other,
            ElicitationOutcome::Answered,
            &a1,
            "2026-04-28T02:00:05Z".into(),
        );
        append_record(&DecisionHistoryRecord::Elicitation(rec_other)).unwrap();

        // Card 2: user picked a listed option → must NOT be collected.
        let req_picked = recommended_request("sess-collect", "picked-1");
        let mut a2 = HashMap::new();
        a2.insert("Which approach?".into(), "Refactor first".into());
        let rec_picked = build_elicitation_record(
            &req_picked,
            ElicitationOutcome::Answered,
            &a2,
            "2026-04-28T02:00:06Z".into(),
        );
        append_record(&DecisionHistoryRecord::Elicitation(rec_picked)).unwrap();

        let picks = collect_other_picks_for_date("2026-04-28", 40);
        assert_eq!(picks.len(), 1, "only the Other-answered card should be collected");
        let ctx = &picks[0];
        assert_eq!(ctx.card_type, "elicitation");
        assert_eq!(ctx.question, "Which approach?");
        assert_eq!(ctx.user_choice, "just rewrite it from scratch");
        assert_eq!(ctx.options.len(), 2);
        assert!(ctx.options[0].contains("(Recommended)"));

        // A different day yields nothing.
        assert!(collect_other_picks_for_date("2020-01-01", 40).is_empty());
    }

    #[test]
    fn collect_other_picks_grabs_rejected_plan() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("collect-reject");
        let _home = FleetHomeOverride::new(&tmp);

        let req = PlanApprovalRequest {
            parked: false,
            id: "plan-rej".into(),
            session_id: "sess-plan".into(),
            workspace_name: "claude-fleet".into(),
            ai_title: None,
            timestamp: "2026-04-28T03:00:00Z".into(),
            plan_content: "Step 1: delete everything\nStep 2: rewrite".into(),
            plan_file_path: None,
        };
        let resp = PlanApprovalResponse {
            id: "plan-rej".into(),
            decision: "reject".into(),
            edited_plan: None,
            feedback: Some("don't delete the tests".into()),
        };
        let rec = build_plan_approval_record(
            &req,
            PlanApprovalOutcome::Rejected,
            Some(&resp),
            "2026-04-28T03:00:20Z".into(),
        );
        append_record(&DecisionHistoryRecord::PlanApproval(rec)).unwrap();

        let picks = collect_other_picks_for_date("2026-04-28", 40);
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].card_type, "plan-approval");
        assert_eq!(picks[0].user_choice, "don't delete the tests");
        assert!(picks[0].question.contains("delete everything"));
    }

    #[test]
    fn stats_date_filter_excludes_other_days() {
        let req = recommended_request("s", "c-other-day");
        let mut answers = HashMap::new();
        answers.insert("Which approach?".into(), "Refactor first".into());
        let rec = build_elicitation_record(
            &req,
            ElicitationOutcome::Answered,
            &answers,
            "2026-04-28T02:00:05Z".into(),
        );
        let mut stats = DecisionCardStats::default();
        // Ask for a different day than the record's requestedAt.
        accumulate_record(&mut stats, &DecisionHistoryRecord::Elicitation(rec), "2020-01-01");
        assert!(stats.by_type.is_empty(), "record on another day must be excluded");
    }

    /// `list_session_decisions` used to be a synchronous Tauri command, so the
    /// main thread serialised every call and the read-then-append inside
    /// `sync_user_prompts_from_jsonl` could never interleave. Now that the
    /// command runs on a threadpool, two concurrent calls for the same session
    /// must still not append the same user prompt twice.
    #[test]
    fn concurrent_sync_does_not_duplicate_user_prompts() {
        let _g = crate::session::fleet_home_lock();
        let tmp = temp_dir("concurrent-sync");
        let _home = FleetHomeOverride::new(&tmp);

        let session = "sess-concurrent";
        let jsonl = tmp.join("transcript.jsonl");
        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&format!(
                r#"{{"type":"user","uuid":"u{i}","timestamp":"2026-04-28T00:00:0{i}Z","message":{{"role":"user","content":[{{"type":"text","text":"hello {i}"}}]}}}}"#
            ));
            content.push('\n');
        }
        fs::write(&jsonl, content).unwrap();

        const THREADS: usize = 8;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let b = barrier.clone();
            let p = jsonl.clone();
            handles.push(std::thread::spawn(move || {
                // All threads enter the read-then-append window together.
                b.wait();
                let _ = sync_user_prompts_from_jsonl("sess-concurrent", &p);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let records = read_persisted_records(session);
        let ids: Vec<String> = records.iter().map(|r| r.id().to_string()).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "concurrent sync appended duplicate user-prompt records: {ids:?}"
        );
        assert_eq!(unique.len(), 5, "each of the 5 prompts recorded exactly once");
    }
}
