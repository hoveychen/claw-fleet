//! Turn-completion cards — wrap a task turn's final message into a decision
//! card when the turn ended without one, so the decision-card notification
//! pipeline (desktop panel → relay → phone push) fires, and then inject a
//! friendly reminder into the session to answer with a decision card next time.
//!
//! ## Why a card and not a plain notification
//!
//! The phone push is not a separate feature: [`crate::mobile_relay`]'s
//! `publish_decision_created` fires whenever a decision card lands on disk, and
//! that is the only channel that reaches a phone with the browser closed. A turn
//! that ends in plain text raises no card, so no push happens. Raising a
//! "fake" [`crate::elicitation`] card at turn-end reuses that whole pipeline
//! for free.
//!
//! ## What counts as a task
//!
//! Boss's discriminator is the launch-time mode: the pure-chat workspace
//! ([`crate::chat_workspace::is_chat_workspace`]) is chat, everything else is a
//! task. A task turn that ends without having raised any decision card gets the
//! completion card; a turn that already raised one (ask / plan / fleet-ask)
//! already notified and is skipped.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::agent_source::{resume_session, ResumeSpec};
use crate::decision_history::{self, DecisionHistoryRecord};
use crate::elicitation::{
    self, ElicitationOption, ElicitationQuestion, ElicitationRequest, ElicitationResponse,
};
use crate::session::SessionInfo;

/// How often the worker re-checks the answer files of the cards it is holding.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Extra grace on top of the decision-panel wait window before a completion
/// card is withdrawn as unanswered.
const DEADLINE_GRACE: Duration = Duration::from_secs(30);

/// How long after a reminder is injected do we suppress further completion
/// cards for that session. The reminder itself spawns a turn (the agent reads
/// it and acknowledges), and that acknowledgement turn must not re-trigger the
/// card — otherwise reminder → card → reminder loops forever.
const REMINDER_COOLDOWN_MS: u64 = 15 * 60 * 1000;

/// `session_id → when the reminder was last injected`, epoch ms. Process-local:
/// the detector and the broker worker always run in the same process.
static REMINDED_AT: Mutex<Option<std::collections::HashMap<String, u64>>> = Mutex::new(None);

fn recently_reminded(session_id: &str, now_ms: u64) -> bool {
    let guard = REMINDED_AT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .as_ref()
        .and_then(|m| m.get(session_id))
        .is_some_and(|&at| now_ms.saturating_sub(at) < REMINDER_COOLDOWN_MS)
}

fn mark_reminded(session_id: &str) {
    let now = now_ms();
    let mut guard = REMINDED_AT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    map.insert(session_id.to_string(), now);
    map.retain(|_, &mut at| now.saturating_sub(at) < REMINDER_COOLDOWN_MS);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Discriminators ──────────────────────────────────────────────────────────

/// Whether a session is a "task" (not the pure-chat workspace).
pub fn is_task_session(session: &SessionInfo) -> bool {
    !crate::chat_workspace::is_chat_workspace(&session.workspace_path)
}

/// Whether the session raised any decision card at or after `since_epoch_ms`.
///
/// `UserPrompt` records are the user's own typed prompts, not cards, so they are
/// ignored. Elicitation (AskUserQuestion / guard), plan-approval and fleet-ask
/// are the three card shapes that already fire a notification.
pub fn raised_card_since(session_id: &str, since_epoch_ms: u64) -> bool {
    decision_history::list_session_records(session_id)
        .iter()
        .any(|record| {
            let requested_at = match record {
                DecisionHistoryRecord::Elicitation(e) => &e.requested_at,
                DecisionHistoryRecord::PlanApproval(p) => &p.requested_at,
                DecisionHistoryRecord::FleetAsk(f) => &f.requested_at,
                DecisionHistoryRecord::UserPrompt(_) => return false,
            };
            epoch_ms_of(requested_at).is_some_and(|t| t >= since_epoch_ms)
        })
}

fn epoch_ms_of(rfc3339: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
}

// ── The card ────────────────────────────────────────────────────────────────

/// The reminder injected into the session after the user answers the card.
pub fn reminder_prompt() -> String {
    "（Fleet 温馨提示）上一轮任务结束时没有用决策卡汇报，老板因此没收到手机通知。\
     下次收尾时请把结论/下一步包装成一张决策卡（询问用户的 ask 工具），让老板在卡片上直接确认。"
        .to_string()
}

/// Render the completion card for one finished task turn.
///
/// The question wraps the turn's final message so the notification preview and
/// the panel both carry what the agent actually concluded, rather than a
/// generic "done".
pub fn build_turn_card(session: &SessionInfo, last_text: &str) -> ElicitationRequest {
    let body = last_text.trim();
    let question = if body.is_empty() {
        "任务已结束，但会话没有留下可展示的结论。".to_string()
    } else {
        format!("任务已完成：\n\n{body}")
    };
    ElicitationRequest {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session.id.clone(),
        workspace_name: session.workspace_name.clone(),
        ai_title: session.ai_title.clone(),
        questions: vec![ElicitationQuestion {
            question,
            header: "任务完成".to_string(),
            options: vec![
                ElicitationOption {
                    label: "收到".into(),
                    description: "确认收到，并提醒会话下次用决策卡汇报".into(),
                    preview: None,
                },
                ElicitationOption {
                    label: "继续".into(),
                    description: "收到并让会话继续推进".into(),
                    preview: None,
                },
            ],
            multi_select: false,
        }],
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
    }
}

/// Decide whether `session`'s finished turn needs a completion card, and write
/// it. Returns the card id when raised.
pub fn maybe_raise(
    session: &SessionInfo,
    last_text: &str,
    since_epoch_ms: u64,
) -> Result<Option<String>, String> {
    if !is_task_session(session) {
        return Ok(None);
    }
    // A live process at WaitingInput means "parked on a decision card", not
    // "turn ended" — resuming would race the live turn, so skip it. dsh has no
    // per-session process, so this guard is a no-op there.
    if session.proc_alive {
        return Ok(None);
    }
    // Just injected a reminder? The acknowledgement turn it spawned must not
    // re-trigger a card (otherwise reminder → card → reminder loops).
    if recently_reminded(&session.id, now_ms()) {
        return Ok(None);
    }
    if raised_card_since(&session.id, since_epoch_ms) {
        return Ok(None);
    }
    let card = build_turn_card(session, last_text);
    let id = card.id.clone();
    elicitation::write_request(&card)?;
    Ok(Some(id))
}

// ── The broker ──────────────────────────────────────────────────────────────

/// One completion card the broker is waiting on.
pub struct TurnCardJob {
    pub card_id: String,
    pub session: SessionInfo,
    /// Latest instant at which an answer still triggers the reminder.
    pub deadline: Instant,
}

/// Hands completion cards to a worker thread and forgets about them.
///
/// Dropping it closes the channel, which tells the worker to stop. Same shape
/// as [`crate::dsh_decisions::DecisionBridge`]: the polling and the blocking
/// resume call stay off the scan/SSE threads that raise the card.
pub struct TurnCardBroker {
    tx: Sender<TurnCardJob>,
}

impl TurnCardBroker {
    /// Start the worker. Never blocks; a failed spawn degrades to "no reminder",
    /// which is the same outcome as never having raised the card.
    pub fn start() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("turn-card-remind".into())
            .spawn(move || worker(rx));
        if let Err(e) = spawned {
            crate::log_debug(&format!("turn card: cannot spawn worker: {e}"));
        }
        Self { tx }
    }

    /// Hand one job over. Never blocks; a dead worker silently drops it.
    pub fn offer(&self, job: TurnCardJob) {
        let _ = self.tx.send(job);
    }
}

fn worker(rx: Receiver<TurnCardJob>) {
    let mut jobs: Vec<TurnCardJob> = Vec::new();
    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(job) => jobs.push(job),
            Err(RecvTimeoutError::Timeout) => {}
            // The broker was dropped; nothing left to do.
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let now = Instant::now();
        let mut i = 0;
        while i < jobs.len() {
            // Withdraw unanswered cards past their window so they cannot pile up
            // forever behind a panel the user never touches.
            if now >= jobs[i].deadline {
                let job = jobs.remove(i);
                elicitation::cleanup(&job.card_id);
                continue;
            }
            match elicitation::try_read_response(&jobs[i].card_id) {
                Some(resp) => {
                    let job = jobs.remove(i);
                    deliver_reminder(&job, &resp);
                    elicitation::cleanup(&job.card_id);
                }
                None => i += 1,
            }
        }
    }
}

/// Inject the reminder into the session, forwarding any substantive answer the
/// user typed so nothing the user said is lost.
fn deliver_reminder(job: &TurnCardJob, resp: &ElicitationResponse) {
    // Declined means the user actively dismissed the card; do not nag.
    if resp.declined {
        return;
    }
    let prompt = reminder_prompt_with_answer(first_answer(resp).as_deref());

    let spec = ResumeSpec {
        session_id: job.session.id.clone(),
        workspace_path: job.session.workspace_path.clone(),
        prompt,
        model: job.session.model.clone(),
        effort: job.session.effort.clone(),
        permission_mode: None,
        images: Vec::new(),
    };
    match resume_session(&job.session.agent_source, &spec, Box::new(|_| {})) {
        Ok(()) => mark_reminded(&job.session.id),
        Err(e) => crate::log_debug(&format!(
            "turn card: resume reminder for {} failed: {e}",
            job.session.id
        )),
    }
}

/// Build the reminder prompt, folding in the user's answer when it carries
/// substance beyond a bare "收到".
fn reminder_prompt_with_answer(answer: Option<&str>) -> String {
    let mut prompt = reminder_prompt();
    match answer {
        // "收到" is a pure acknowledgement — the reminder alone is enough.
        Some("收到") | None => {}
        Some("继续") => prompt = format!("{prompt}\n\n请继续推进任务。"),
        Some(other) => prompt = format!("{prompt}\n\n（老板在确认卡片上的回复：{other}）"),
    }
    prompt
}

/// First non-empty answer string off the card's flat answer map.
fn first_answer(resp: &ElicitationResponse) -> Option<String> {
    resp.answers
        .values()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// Deadline for a completion card: the decision-panel wait window plus grace.
pub fn default_deadline() -> Instant {
    Instant::now() + crate::decision_panel_config::load().wait_duration() + DEADLINE_GRACE
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn session(workspace: &str) -> SessionInfo {
        SessionInfo {
            id: "s1".into(),
            workspace_path: workspace.into(),
            workspace_name: "w".into(),
            agent_source: "claude-code".into(),
            last_message_preview: Some("done".into()),
            ..Default::default()
        }
    }

    #[test]
    fn chat_workspace_is_not_a_task() {
        // Reads the chat path, then has `is_task_session` read it again — and
        // that path is derived from FLEET_HOME. Without the lock, a sibling
        // test redirecting FLEET_HOME between the two reads makes them
        // disagree. Tests that only *read* a FLEET_HOME-derived path need the
        // lock just as much as the ones that set it.
        let _env_guard = crate::session::fleet_home_lock();
        // A project path is always a task; the chat workspace is not.
        let mut s = session("/definitely/not/chat/project");
        assert!(is_task_session(&s));
        if let Some(chat) = crate::chat_workspace::chat_workspace_path() {
            s.workspace_path = chat.to_string_lossy().to_string();
            assert!(!is_task_session(&s));
        }
    }

    #[test]
    fn maybe_raise_skips_chat_and_parked_sessions() {
        // Same reason as `chat_workspace_is_not_a_task`: the chat path is read
        // once here and again inside `maybe_raise`. Observed failing under
        // `--test-threads=16` before this lock, at the chat assert.
        let _env_guard = crate::session::fleet_home_lock();
        // A session whose process is still alive at WaitingInput is parked on a
        // decision card (or interactive) — never wrap it again.
        let mut parked = session("/p");
        parked.proc_alive = true;
        assert!(matches!(maybe_raise(&parked, "done", 0), Ok(None)));
        // A chat-workspace session is not a task.
        if let Some(chat) = crate::chat_workspace::chat_workspace_path() {
            let chat = session(&chat.to_string_lossy());
            assert!(matches!(maybe_raise(&chat, "done", 0), Ok(None)));
        }
    }

    #[test]
    fn build_turn_card_wraps_the_last_message() {
        let card = build_turn_card(&session("/p"), "fixed the bug");
        assert!(card.questions[0].question.contains("fixed the bug"));
        assert_eq!(card.questions[0].header, "任务完成");
        assert_eq!(card.questions[0].options.len(), 2);
        assert_eq!(card.session_id, "s1");
        assert!(!card.parked);
    }

    #[test]
    fn build_turn_card_degrades_without_a_message() {
        let card = build_turn_card(&session("/p"), "   ");
        assert!(!card.questions[0].question.contains("fixed"));
    }

    #[test]
    fn reminder_is_a_nonempty_directive() {
        assert!(!reminder_prompt().is_empty());
        assert!(reminder_prompt().contains("决策卡"));
    }

    #[test]
    fn reminder_folds_in_a_substantive_answer() {
        assert!(reminder_prompt_with_answer(Some("继续")).contains("继续推进"));
        // A bare acknowledgement adds nothing beyond the reminder itself.
        assert_eq!(reminder_prompt_with_answer(Some("收到")), reminder_prompt());
        assert!(reminder_prompt_with_answer(Some("换个方案")).contains("换个方案"));
    }

    #[test]
    fn epoch_ms_parses_rfc3339() {
        assert_eq!(epoch_ms_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_ms_of("not-a-time"), None);
    }
}
