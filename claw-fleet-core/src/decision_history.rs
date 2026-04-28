//! Decision history — append-only per-session log of every AskUserQuestion
//! (`elicitation`) and ExitPlanMode (`plan-approval`) decision card the user
//! has seen, including the questions/options shown and the user's choice.
//!
//! Storage: `~/.fleet/decision-history/<session_id>.jsonl`, one record per
//! line. Records are written by the `fleet elicitation` and `fleet
//! plan-approval` CLIs at the moment a response (or terminal non-response —
//! timeout, declined, heartbeat lost) is observed, before the request file is
//! cleaned up.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};
use crate::plan_approval::{PlanApprovalRequest, PlanApprovalResponse};

// ── Outcome enums ────────────────────────────────────────────────────────────

/// Terminal outcome of an elicitation card.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
#[serde(rename_all = "kebab-case")]
pub enum PlanApprovalOutcome {
    Approved,
    ApprovedWithEdits,
    Rejected,
    HeartbeatLost,
    Timeout,
}

// ── Selected-option enrichment ───────────────────────────────────────────────

/// What the user picked for one elicitation question, enriched with the
/// matching option's label/description so the history is readable without
/// cross-referencing.
#[derive(Serialize, Deserialize, Clone, Debug)]
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
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DecisionHistoryRecord {
    Elicitation(ElicitationRecord),
    PlanApproval(PlanApprovalRecord),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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

#[derive(Serialize, Deserialize, Clone, Debug)]
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
pub fn append_record(record: &DecisionHistoryRecord) -> Result<(), String> {
    let dir = history_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create decision-history dir: {e}"))?;
    let path = history_file(record.session_id())
        .ok_or_else(|| format!("invalid session_id: {:?}", record.session_id()))?;
    let line = serde_json::to_string(record).map_err(|e| format!("serialize: {e}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("append: {e}"))
}

/// Read all records for a session, oldest-first. Missing file → empty Vec.
/// Malformed lines are skipped (logged via `eprintln!`) so a single corrupt
/// record can't take out the whole session view.
pub fn list_session_records(session_id: &str) -> Vec<DecisionHistoryRecord> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};
    use std::sync::Mutex;

    // real_home_dir() reads $FLEET_HOME, so tests must serialize and override it.
    static FLEET_HOME_LOCK: Mutex<()> = Mutex::new(());

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
        let _g = FLEET_HOME_LOCK.lock().unwrap();
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
    fn malformed_line_is_skipped() {
        let _g = FLEET_HOME_LOCK.lock().unwrap();
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
}
