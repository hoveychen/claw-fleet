//! Bridge from dsh's two answerable downlink frames to Fleet Decision Cards.
//!
//! `dsh web` blocks a turn on two things a human has to settle: an
//! `approval/requested` (a tool call the session's policy will not run
//! unattended) and a `question/requested` (the agent called `ask_user_question`).
//! Both arrive on `events.mux` as `server-request` envelopes and are answered by
//! POSTing a `client-response` to `/api/respond` that echoes the frame's
//! `rpcId` — see [`crate::dsh_client`].
//!
//! ## No new card channel
//!
//! Fleet already carries two card shapes whose semantics match these exactly,
//! each with its whole pipeline built (Backend trait → `fleet serve` HTTP →
//! `RemoteBackend` → desktop watcher → mobile relay → decision history):
//!
//! * approval → [`crate::permission_prompt_ipc`] — one tool, allow or deny.
//! * question → [`crate::elicitation`] — several questions, options, free text.
//!
//! So this module writes into those two channels rather than inventing a third.
//! The card id *is* the frame's `rpcId`, which makes the whole thing idempotent
//! for free: dsh's mux replays every still-pending requested frame with its
//! original `rpcId` whenever the socket reopens, and a replay lands on the file
//! that already exists instead of raising a second card.
//!
//! ## Everything runs on one worker thread
//!
//! [`DecisionBridge::offer`] only hands the frame to a channel. All the work —
//! writing the card, polling for the user's answer, POSTing it back — happens on
//! a dedicated thread, because the socket pump lives inside a tokio runtime and
//! [`crate::dsh_client`] is `reqwest::blocking` (whose internal runtime panics
//! if it is dropped on an async worker). One thread also means the pending table
//! needs no lock: it is that thread's local state.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use serde_json::{json, Value};

use crate::dsh_client::DshClient;
use crate::dsh_events::DshFrame;
use crate::elicitation::{
    ElicitationOption, ElicitationQuestion, ElicitationRequest, ElicitationResponse,
};
use crate::permission_prompt_ipc::{
    PermissionPromptDecision, PermissionPromptRequest, PermissionPromptResponse,
};

/// How often the worker re-checks the answer files of everything it is holding.
/// Also the ceiling on how long a frame waits before the worker notices it,
/// since the same `recv_timeout` carries both.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Message shown to dsh when Fleet withdraws a question the user declined.
const DECLINED_MESSAGE: &str = "the user declined to answer in Fleet";

/// Message shown to dsh when the bridge goes away with questions still open.
const ABANDONED_MESSAGE: &str = "Fleet stopped watching this dsh server";

// ── The question domain ─────────────────────────────────────────────────────

/// One choice offered by `ask_user_question`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshQuestionOption {
    pub label: String,
    pub description: Option<String>,
}

/// One question out of a `question/requested` frame.
///
/// Mirrors dsh's `AskUserQuestionItem`. `id` is the caller's, and the answer
/// must echo it: the server validates the answer array positionally *and* by id
/// before it will accept it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshQuestion {
    pub id: String,
    pub question: String,
    /// Supporting detail dsh keeps out of the option labels.
    pub detail: Option<String>,
    pub header: Option<String>,
    /// Absent in dsh means "no menu, answer in free text"; empty here.
    pub options: Vec<DshQuestionOption>,
    pub multi_select: bool,
}

impl DshQuestion {
    /// Decode one `questions[]` entry. `id` and `question` are the only fields
    /// dsh guarantees, so an entry missing either is not a question we can
    /// answer and is dropped.
    pub fn from_value(v: &Value) -> Option<Self> {
        let id = v.get("id").and_then(Value::as_str)?.to_string();
        let question = v.get("question").and_then(Value::as_str)?.to_string();
        let text = |key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let options = v
            .get("options")
            .and_then(Value::as_array)
            .map(|opts| {
                opts.iter()
                    .filter_map(|o| {
                        Some(DshQuestionOption {
                            label: o.get("label").and_then(Value::as_str)?.to_string(),
                            description: o
                                .get("description")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id,
            question,
            detail: text("detail"),
            header: text("header"),
            options,
            multi_select: v.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

// ── dsh question → Fleet card ───────────────────────────────────────────────

/// Longest header a Decision Card chip renders without truncating.
const HEADER_MAX: usize = 12;

/// Render one dsh question as one card question.
///
/// `detail` becomes a second paragraph of the prompt: the card has no separate
/// slot for it, and dropping it would hide the context the agent attached
/// precisely because it did not fit an option label.
fn card_question(q: &DshQuestion) -> ElicitationQuestion {
    let question = match q.detail.as_deref() {
        Some(detail) => format!("{}\n\n{}", q.question, detail),
        None => q.question.clone(),
    };
    let header = q
        .header
        .as_deref()
        .filter(|h| !h.is_empty())
        .map(|h| h.chars().take(HEADER_MAX).collect::<String>())
        .unwrap_or_else(|| "dsh".to_string());
    ElicitationQuestion {
        question,
        header,
        options: q
            .options
            .iter()
            .map(|o| ElicitationOption {
                label: o.label.clone(),
                description: o.description.clone().unwrap_or_default(),
                preview: None,
            })
            .collect(),
        multi_select: q.multi_select,
    }
}

// ── Fleet answer → dsh response payload ─────────────────────────────────────

/// Turn one card answer string back into dsh's `{id, selected, custom?}`.
///
/// The card hands answers back as one flat string per question: selected labels
/// joined with `", "`, or whatever the user typed into the free-text box, and it
/// may carry appended decorations (attachment mentions, the single→multi
/// override note). So this recognises labels rather than assuming the string is
/// one — and anything it cannot recognise degrades to `custom`, which keeps the
/// user's own words rather than dropping them.
///
/// The output obeys dsh's `matchesQuestions` validation, which refuses the whole
/// answer if any of it is off: no duplicate labels, no label that is not one of
/// this question's options, no empty `custom`, and — for a single-select
/// question — at most one selection and never a selection alongside a `custom`.
fn decode_answer(answer: &str, q: &DshQuestion) -> Value {
    let trimmed = answer.trim();
    let mut selected: Vec<String> = Vec::new();
    let mut leftover: Vec<&str> = Vec::new();

    // Whole-string match first: a label that itself contains ", " would be torn
    // apart by the split below, and single-select answers are exactly this case.
    if q.options.iter().any(|o| o.label == trimmed) {
        selected.push(trimmed.to_string());
    } else {
        for segment in trimmed.split(", ") {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            if q.options.iter().any(|o| o.label == segment) {
                if !selected.iter().any(|s| s == segment) {
                    selected.push(segment.to_string());
                }
            } else {
                leftover.push(segment);
            }
        }
    }

    let mut custom = leftover.join(", ");
    if !q.multi_select {
        // A single-select question accepts a selection or free text, never both.
        if !selected.is_empty() {
            custom.clear();
            selected.truncate(1);
        }
    }

    let mut item = json!({ "id": q.id, "selected": selected });
    if !custom.trim().is_empty() {
        item["custom"] = json!(custom);
    }
    item
}

/// Build the `result.value` of a question answer.
fn question_response_value(session_id: &str, answers: Vec<Value>) -> Value {
    json!({
        "sessionId": session_id,
        "answer": { "answers": answers },
    })
}

/// Build the `result.value` of an approval answer.
///
/// dsh's outcome vocabulary has four members but only two are answerable —
/// `cancelled` and `unavailable` are outcomes the *server* reaches on its own.
/// The card's optional deny reason has nowhere to go: the payload schema is
/// closed, so an unrecognised field would fail validation and be refused.
fn approval_response_value(session_id: &str, approval_id: &str, allow: bool) -> Value {
    json!({
        "sessionId": session_id,
        "approvalId": approval_id,
        "outcome": if allow { "allowed-once" } else { "rejected" },
    })
}

// ── The pending table ───────────────────────────────────────────────────────

/// One card the bridge has raised and is waiting on.
enum Pending {
    Approval {
        session_id: String,
        approval_id: String,
    },
    Question {
        session_id: String,
        questions: Vec<DshQuestion>,
        /// The card's question texts, in the same order — the keys the answer
        /// map comes back under (which is the *rendered* text, not the dsh one,
        /// because `detail` is folded into it).
        texts: Vec<String>,
    },
}

impl Pending {
    /// Drop this card without answering it.
    fn cleanup(&self, id: &str) {
        match self {
            Self::Approval { .. } => crate::permission_prompt_ipc::cleanup(id),
            Self::Question { .. } => crate::elicitation::cleanup(id),
        }
    }
}

// ── The bridge ──────────────────────────────────────────────────────────────

/// Hands answerable frames to a worker thread and forgets about them.
///
/// Dropping it closes the channel, which is how the worker learns to withdraw
/// everything still open and exit.
pub struct DecisionBridge {
    tx: Sender<DshFrame>,
}

impl DecisionBridge {
    /// Start the worker against the `dsh web` instance on `port`.
    pub fn start(port: u16) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("dsh-decisions".into())
            .spawn(move || worker(port, rx));
        if let Err(e) = spawned {
            crate::log_debug(&format!("dsh decisions: cannot spawn worker: {e}"));
        }
        Self { tx }
    }

    /// Hand one frame over. Never blocks; a dead worker silently drops it,
    /// which is the same outcome as never having raised the card.
    pub fn offer(&self, frame: DshFrame) {
        let _ = self.tx.send(frame);
    }
}

/// Own every pending card until it is answered, resolved elsewhere, or the
/// channel closes.
fn worker(port: u16, rx: Receiver<DshFrame>) {
    let client = match DshClient::new(port) {
        Ok(c) => c,
        Err(e) => {
            crate::log_debug(&format!("dsh decisions: no client: {e}"));
            return;
        }
    };
    let mut pending: HashMap<String, Pending> = HashMap::new();

    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(frame) => handle_frame(&client, &mut pending, frame),
            Err(RecvTimeoutError::Timeout) => collect_answers(&client, &mut pending),
            // The watcher was dropped: this server is gone or has moved to a new
            // port, so nothing here can ever be answered. Withdraw the questions
            // (an unanswered `ask_user_question` would hang its turn forever)
            // and take the cards down.
            Err(RecvTimeoutError::Disconnected) => {
                for (id, entry) in pending.drain() {
                    if let Pending::Question { .. } = entry {
                        let _ = client.respond_cancelled(&id, ABANDONED_MESSAGE);
                    }
                    entry.cleanup(&id);
                }
                return;
            }
        }
    }
}

fn handle_frame(client: &DshClient, pending: &mut HashMap<String, Pending>, frame: DshFrame) {
    match frame {
        DshFrame::ApprovalRequested {
            rpc_id,
            session_id,
            approval_id,
            tool_name,
            call_id,
            reason,
        } => {
            // A mux reconnect replays this frame verbatim; the card is already up.
            if pending.contains_key(&rpc_id) {
                return;
            }
            let (workspace_name, ai_title) = session_meta(client, &session_id);
            let request = PermissionPromptRequest {
                id: rpc_id.clone(),
                session_id: session_id.clone(),
                workspace_name,
                ai_title,
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_name,
                tool_input: json!({
                    "approvalId": approval_id,
                    "reason": reason,
                    "callId": call_id,
                    "agent": "dsh",
                }),
                tool_use_id: call_id,
            };
            match crate::permission_prompt_ipc::write_request(&request) {
                Ok(()) => {
                    pending.insert(
                        rpc_id,
                        Pending::Approval {
                            session_id,
                            approval_id,
                        },
                    );
                }
                Err(e) => crate::log_debug(&format!("dsh decisions: approval card: {e}")),
            }
        }
        DshFrame::QuestionRequested {
            rpc_id,
            session_id,
            questions,
        } => {
            if pending.contains_key(&rpc_id) {
                return;
            }
            if questions.is_empty() {
                // Nothing to render, and an empty answer array would fail the
                // server's length check anyway.
                return;
            }
            let (workspace_name, ai_title) = session_meta(client, &session_id);
            let cards: Vec<ElicitationQuestion> = questions.iter().map(card_question).collect();
            let texts = cards.iter().map(|c| c.question.clone()).collect();
            let request = ElicitationRequest {
                id: rpc_id.clone(),
                session_id: session_id.clone(),
                workspace_name,
                ai_title,
                questions: cards,
                timestamp: chrono::Utc::now().to_rfc3339(),
                parked: false,
            };
            match crate::elicitation::write_request(&request) {
                Ok(()) => {
                    pending.insert(
                        rpc_id,
                        Pending::Question {
                            session_id,
                            questions,
                            texts,
                        },
                    );
                }
                Err(e) => crate::log_debug(&format!("dsh decisions: question card: {e}")),
            }
        }
        // Someone else settled it — dsh's own web UI, a cancelled turn, or the
        // fail-closed default. The card is stale, so take it down rather than
        // leaving a button that would come back `not-pending`.
        DshFrame::ApprovalResolved { approval_id, .. } => {
            let stale: Vec<String> = pending
                .iter()
                .filter(|(_, p)| {
                    matches!(p, Pending::Approval { approval_id: a, .. } if *a == approval_id)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                if let Some(entry) = pending.remove(&id) {
                    entry.cleanup(&id);
                }
            }
        }
        DshFrame::QuestionResolved { question_rpc_id } => {
            if let Some(entry) = pending.remove(&question_rpc_id) {
                entry.cleanup(&question_rpc_id);
            }
        }
        // Phase frames belong to `dsh_events`; the pump never routes them here.
        DshFrame::Event { .. } | DshFrame::Status { .. } | DshFrame::Ignored => {}
    }
}

/// Answer everything the user has settled since the last tick.
fn collect_answers(client: &DshClient, pending: &mut HashMap<String, Pending>) {
    let answered: Vec<String> = pending
        .iter()
        .filter(|(id, entry)| match entry {
            Pending::Approval { .. } => {
                crate::permission_prompt_ipc::try_read_response(id).is_some()
            }
            Pending::Question { .. } => crate::elicitation::try_read_response(id).is_some(),
        })
        .map(|(id, _)| id.clone())
        .collect();

    for id in answered {
        let Some(entry) = pending.remove(&id) else {
            continue;
        };
        let outcome = match &entry {
            Pending::Approval {
                session_id,
                approval_id,
            } => crate::permission_prompt_ipc::try_read_response(&id).map(|r| {
                send_approval(client, &id, session_id, approval_id, &r)
            }),
            Pending::Question {
                session_id,
                questions,
                texts,
            } => crate::elicitation::try_read_response(&id)
                .map(|r| send_question(client, &id, session_id, questions, texts, &r)),
        };
        if let Some(Err(e)) = outcome {
            crate::log_debug(&format!("dsh decisions: respond {id}: {e}"));
        }
        entry.cleanup(&id);
    }
}

fn send_approval(
    client: &DshClient,
    rpc_id: &str,
    session_id: &str,
    approval_id: &str,
    resp: &PermissionPromptResponse,
) -> Result<(), String> {
    let allow = matches!(resp.decision, PermissionPromptDecision::Allow);
    client
        .respond(rpc_id, approval_response_value(session_id, approval_id, allow))
        .map_err(Into::into)
}

fn send_question(
    client: &DshClient,
    rpc_id: &str,
    session_id: &str,
    questions: &[DshQuestion],
    texts: &[String],
    resp: &ElicitationResponse,
) -> Result<(), String> {
    if resp.declined {
        return client
            .respond_cancelled(rpc_id, DECLINED_MESSAGE)
            .map_err(Into::into);
    }
    // Positional: the server checks answer[i].id against questions[i].id, so the
    // array has to cover every question in its original order even when the user
    // left one blank.
    let answers = questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let answer = texts
                .get(i)
                .and_then(|t| resp.answers.get(t))
                .map(String::as_str)
                .unwrap_or("");
            decode_answer(answer, q)
        })
        .collect();
    client
        .respond(rpc_id, question_response_value(session_id, answers))
        .map_err(Into::into)
}

/// Look up the workspace name and title Fleet shows on the card.
///
/// One `session.list` per decision — decisions are rare and the call is a
/// loopback round trip, so caching it would only risk showing a stale title.
/// A lookup that fails leaves the card with the fields the panel already
/// tolerates being empty.
fn session_meta(client: &DshClient, session_id: &str) -> (String, Option<String>) {
    let Ok(listed) = client.call("session.list", json!({})) else {
        return (String::new(), None);
    };
    listed
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|i| i.get("sessionId").and_then(Value::as_str) == Some(session_id))
                .and_then(crate::dsh_source::session_info_from_list_item)
        })
        .map(|info| (info.workspace_name, info.ai_title))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(multi_select: bool) -> DshQuestion {
        DshQuestion {
            id: "q1".into(),
            question: "Which one?".into(),
            detail: None,
            header: Some("Choice".into()),
            options: vec![
                DshQuestionOption {
                    label: "Alpha".into(),
                    description: Some("the first".into()),
                },
                DshQuestionOption {
                    label: "Beta".into(),
                    description: None,
                },
            ],
            multi_select,
        }
    }

    /// Verbatim `questions[]` entry shape from dsh's own `AskUserQuestionItem`.
    #[test]
    fn decodes_a_question_item() {
        let v = json!({
            "id": "pick-db",
            "question": "Which database?",
            "detail": "Both are already provisioned.",
            "header": "Database",
            "multiSelect": true,
            "options": [
                { "label": "Postgres", "description": "relational" },
                { "label": "SQLite" }
            ]
        });
        let q = DshQuestion::from_value(&v).expect("decoded");
        assert_eq!(q.id, "pick-db");
        assert_eq!(q.detail.as_deref(), Some("Both are already provisioned."));
        assert!(q.multi_select);
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[1].label, "SQLite");
        assert_eq!(q.options[1].description, None);
    }

    /// A free-text question carries no `options`; dsh's field is optional and an
    /// absent menu is not a malformed question.
    #[test]
    fn a_question_without_options_decodes_to_an_empty_menu() {
        let q = DshQuestion::from_value(&json!({ "id": "q", "question": "Why?" })).expect("decoded");
        assert!(q.options.is_empty());
        assert!(!q.multi_select);
        assert_eq!(q.header, None);
    }

    #[test]
    fn a_question_without_id_or_text_is_not_answerable() {
        assert!(DshQuestion::from_value(&json!({ "question": "no id" })).is_none());
        assert!(DshQuestion::from_value(&json!({ "id": "q" })).is_none());
    }

    /// The card has no `detail` slot, and dropping it would hide the context the
    /// agent deliberately kept out of the option labels.
    #[test]
    fn detail_is_folded_into_the_card_prompt() {
        let mut q = question(false);
        q.detail = Some("Extra context.".into());
        let card = card_question(&q);
        assert_eq!(card.question, "Which one?\n\nExtra context.");
        assert_eq!(card.header, "Choice");
        assert_eq!(card.options[0].label, "Alpha");
        assert_eq!(card.options[0].description, "the first");
        // dsh's description is optional; the card's is not.
        assert_eq!(card.options[1].description, "");
    }

    #[test]
    fn a_headerless_question_gets_a_short_default() {
        let mut q = question(false);
        q.header = None;
        assert_eq!(card_question(&q).header, "dsh");
    }

    /// Chips truncate past 12 characters, so the header is cut here rather than
    /// letting the panel do it.
    #[test]
    fn an_overlong_header_is_cut_to_the_chip_width() {
        let mut q = question(false);
        q.header = Some("一二三四五六七八九十一二三".into());
        assert_eq!(card_question(&q).header.chars().count(), HEADER_MAX);
    }

    #[test]
    fn a_single_selection_decodes_to_that_label() {
        assert_eq!(
            decode_answer("Alpha", &question(false)),
            json!({ "id": "q1", "selected": ["Alpha"] })
        );
    }

    /// The card joins multiple selections with ", " before handing them back.
    #[test]
    fn a_multi_selection_decodes_to_every_label() {
        assert_eq!(
            decode_answer("Alpha, Beta", &question(true)),
            json!({ "id": "q1", "selected": ["Alpha", "Beta"] })
        );
    }

    /// Free text the user typed into the "Other" box is not a label; dsh takes
    /// it as `custom` rather than us dropping the only thing they said.
    #[test]
    fn free_text_decodes_to_custom() {
        assert_eq!(
            decode_answer("neither, use Gamma", &question(false)),
            json!({ "id": "q1", "selected": [], "custom": "neither, use Gamma" })
        );
    }

    /// `matchesQuestions` refuses a single-select answer that carries both a
    /// selection and free text, so when the card decorates a chosen label the
    /// selection wins and the decoration is dropped — sending both would have
    /// the server refuse the whole answer and leave the turn parked.
    #[test]
    fn a_single_select_answer_never_carries_both_a_label_and_custom() {
        let decoded = decode_answer("Alpha, and also Beta please", &question(false));
        assert_eq!(decoded["selected"], json!(["Alpha"]));
        assert!(
            decoded.get("custom").is_none(),
            "custom must be dropped: {decoded}"
        );
    }

    /// The panel appends its own note when the user widens a single-select
    /// question, and the note is not part of any label. Nothing here tries to
    /// unpick it: the whole string becomes `custom`, which is a valid
    /// single-select answer and keeps every word the user actually chose,
    /// rather than a clever strip that could eat a label ending in a bracket.
    #[test]
    fn a_decorated_single_label_degrades_to_free_text_rather_than_being_guessed() {
        let decorated = "Alpha [用户将此题从单选改为多选 / user switched this question from single-select to multi-select]";
        let decoded = decode_answer(decorated, &question(false));
        assert_eq!(decoded["selected"], json!([]));
        assert_eq!(decoded["custom"], json!(decorated));
    }

    /// The same answer on a multi-select question keeps both halves — there the
    /// server allows it.
    #[test]
    fn a_multi_select_answer_may_carry_a_label_and_custom() {
        let decoded = decode_answer("Alpha, something else", &question(true));
        assert_eq!(decoded["selected"], json!(["Alpha"]));
        assert_eq!(decoded["custom"], json!("something else"));
    }

    /// Duplicate labels fail the server's dedupe check.
    #[test]
    fn repeated_labels_collapse() {
        assert_eq!(
            decode_answer("Alpha, Alpha", &question(true))["selected"],
            json!(["Alpha"])
        );
    }

    /// A label containing the join separator would be torn in half by a naive
    /// split, producing two segments that match nothing.
    #[test]
    fn a_label_containing_the_separator_still_matches() {
        let q = DshQuestion {
            options: vec![DshQuestionOption {
                label: "Yes, do it".into(),
                description: None,
            }],
            ..question(false)
        };
        assert_eq!(
            decode_answer("Yes, do it", &q)["selected"],
            json!(["Yes, do it"])
        );
    }

    /// An unanswered question still needs its slot: the server checks the answer
    /// array's length against the question array's.
    #[test]
    fn an_empty_answer_decodes_to_an_empty_selection() {
        assert_eq!(
            decode_answer("   ", &question(false)),
            json!({ "id": "q1", "selected": [] })
        );
    }

    #[test]
    fn approval_payload_names_both_ids_and_a_closed_outcome() {
        assert_eq!(
            approval_response_value("session-a", "ap-1", true),
            json!({ "sessionId": "session-a", "approvalId": "ap-1", "outcome": "allowed-once" })
        );
        assert_eq!(
            approval_response_value("session-a", "ap-1", false)["outcome"],
            json!("rejected")
        );
    }

    #[test]
    fn question_payload_nests_the_answers_under_answer() {
        assert_eq!(
            question_response_value("session-a", vec![json!({ "id": "q1", "selected": [] })]),
            json!({
                "sessionId": "session-a",
                "answer": { "answers": [{ "id": "q1", "selected": [] }] }
            })
        );
    }
}
