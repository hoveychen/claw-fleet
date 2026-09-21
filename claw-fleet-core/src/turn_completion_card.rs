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
//! already notified and is skipped — as is a turn that ends within minutes of
//! the user answering a card by hand, because then the user is at the panel and
//! there is no notification left to deliver (see [`answered_card_within`]).

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

/// How recently the user must have answered one of this session's cards for the
/// completion card to be considered redundant. See [`answered_card_within`].
const RECENT_ANSWER_WINDOW_MS: u64 = 3 * 60 * 1000;

/// Whether the user resolved any of this session's decision cards within the
/// last [`RECENT_ANSWER_WINDOW_MS`].
///
/// This is the "the boss is right here" gate. `raised_card_since` only looks at
/// the turn that just ended, so the shape it cannot see is: the agent raises a
/// report card, the user answers it with something that means "we're done"
/// (「不用，东西都在就行」), and the agent spends its next turn on the one-line
/// plain-text acknowledgement the interaction-mode session-end exemption asks
/// for. That acknowledgement turn raises no card, so the detector reads it as an
/// unnotified finish — 29 seconds after the user typed into a card by hand.
/// Wrapping it pushes a notification at someone who is demonstrably watching the
/// panel, and answering it resumes a session the user just closed out.
fn answered_card_within(session_id: &str, window_ms: u64, now_ms: u64) -> bool {
    decision_history::list_session_records(session_id)
        .iter()
        .any(|record| {
            let resolved_at = match record {
                DecisionHistoryRecord::Elicitation(e) => &e.resolved_at,
                DecisionHistoryRecord::PlanApproval(p) => &p.resolved_at,
                DecisionHistoryRecord::FleetAsk(f) => &f.resolved_at,
                // Not a card — the user's own typed prompt. A fresh prompt is a
                // new brief, not a sign the task just ended.
                DecisionHistoryRecord::UserPrompt(_) => return false,
            };
            epoch_ms_of(resolved_at).is_some_and(|t| now_ms.saturating_sub(t) < window_ms)
        })
}

fn epoch_ms_of(rfc3339: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
}

// ── The card ────────────────────────────────────────────────────────────────

/// The reminder injected into the session after the user answers the card.
///
/// It arrives as a *user prompt*, which is the only channel a resumed session
/// has — and a session that reads a user prompt starts a turn and looks for
/// work to do. The first version asked agents to "wrap up conclusions and next
/// steps as a decision card for next time", and agents dutifully took it as a
/// brief to keep going: a relay successor was observed re-planning its whole
/// P-task off the back of it. So the text leads with what it *is* (a
/// meta-notice about the previous turn's shape, not a new task) and names the
/// one action it wants, before anything else.
pub fn reminder_prompt() -> String {
    "（Fleet 系统提示 —— 这不是新任务，也不是老板给你的指令）\
     上一轮任务已经结束了。这条只是告诉你：那一轮收尾用的是纯文本、没有决策卡，\
     所以老板的手机没收到通知。\
     请不要因为这条消息继续推进、重做或扩展任何工作，也不要重新读代码去找活干。"
        .to_string()
}

/// What the session should actually do with the reminder when the boss did not
/// ask for more work: re-send the conclusion as a card, then stop.
const REMINDER_ACTION: &str = "现在只需做一件事：把上一轮的结论用一张决策卡\
（询问用户的 ask 工具）重新汇报一次，让老板在卡片上确认，然后结束回合。\
以后每次收尾都这样做。";

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
        turn_completion: true,
    }
}

/// Re-read the tail of `session`'s transcript for the turn's final assistant
/// text, uncapped.
///
/// Callers hand us `SessionInfo::last_message_preview`, which is clipped to 200
/// chars for the task-list subtitle — as a card body that reads as a sentence
/// cut in half. The card is scrollable, so it wants the whole message. One tail
/// read per raised card, and only after every guard above has passed; the
/// alternative (a second, uncapped field on `SessionInfo`) would carry full
/// turn text for every scanned session through memory and IPC.
///
/// Returns `None` for transcripts this extractor does not understand (codex
/// rollouts, dsh logs), leaving the caller on the preview.
fn full_last_text(session: &SessionInfo) -> Option<String> {
    let lines =
        crate::jsonl_tail::read_tail_lines_as_json(std::path::Path::new(&session.jsonl_path), 100)
            .ok()?;
    crate::session::extract_last_text_full(&lines)
}

/// Decide whether `session`'s finished turn needs a completion card, and write
/// it. Returns the card id when raised.
///
/// `last_text` is only a fallback body: when the transcript can be re-read, the
/// card gets the untruncated final message instead.
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
    // The user answered one of this session's cards moments ago, so they are at
    // the panel and already know where the task stands — the notification this
    // card exists to fire has nothing left to deliver.
    if answered_card_within(&session.id, RECENT_ANSWER_WINDOW_MS, now_ms()) {
        return Ok(None);
    }
    // A registered relay is a legitimate card-less exit, and the *only* correct
    // one: the interaction-mode guidance forbids a card after `fleet handoff`
    // (it hangs the turn the Stop hook needs to end) and `mcp_server`'s
    // `refuse_if_handoff_pending` enforces that on the agent's own cards. This
    // card has a second producer — Fleet itself — which that refusal never saw,
    // so it kept firing on exactly the turns the rule exempts. Worse, answering
    // it resumes a session whose successor already owns the work.
    if crate::handoff::has_relayed(&session.id) {
        return Ok(None);
    }
    let body = full_last_text(session).unwrap_or_else(|| last_text.to_string());
    let card = build_turn_card(session, &body);
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
    // A session that already relayed its baton is retired — its successor owns
    // the work and is very likely running right now. Resuming it here would put
    // two agents on the same plan, and the woken predecessor resumes with a
    // context ending at "I just registered a handoff" (see `handoff.rs`'s
    // `successor_of`). `maybe_raise` already refuses to card such a session, so
    // this is the narrow window it cannot see: the card is raised at turn end
    // and the answer can land minutes later, with the relay registered in
    // between.
    if crate::handoff::has_relayed(&job.session.id) {
        crate::log_debug(&format!(
            "turn card: skip reminder for {} — session already relayed",
            job.session.id
        ));
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
/// substance beyond a bare acknowledgement.
fn reminder_prompt_with_answer(answer: Option<&str>) -> String {
    let notice = reminder_prompt();
    match answer {
        // Acknowledged: a pure acknowledgement — re-report and stop, nothing else.
        Some("收到") | None => format!("{notice}\n\n{REMINDER_ACTION}"),
        // The boss explicitly asked for more work, which is the one thing that
        // overrides the "do not keep working" line above.
        Some("继续") => format!(
            "{notice}\n\n老板在卡片上选了「继续」，所以这一条是例外：请继续推进任务，\
             并在收尾时用决策卡汇报。"
        ),
        Some(other) => format!(
            "{notice}\n\n老板在卡片上回复了：{other}\n\n请按这句回复办；\
             如果它没有指派新的工作，就照上面的做法用决策卡重新汇报一次并结束回合。"
        ),
    }
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
    fn reminder_disclaims_being_a_new_task() {
        let p = reminder_prompt();
        assert!(!p.is_empty());
        // The whole point of the rewrite: it must say up front that it is not a
        // task and that the session should not go looking for work.
        assert!(p.contains("不是新任务"));
        assert!(p.contains("不要"));
    }

    #[test]
    fn reminder_folds_in_a_substantive_answer() {
        assert!(reminder_prompt_with_answer(Some("继续")).contains("继续推进"));
        // A bare acknowledgement asks for the card and nothing else — and must
        // never read as permission to keep working.
        let ack = reminder_prompt_with_answer(Some("收到"));
        assert!(ack.contains("决策卡"));
        assert!(ack.contains("结束回合"));
        // The only sentence that licenses more work is the "continue" ("继续") exception.
        assert!(!ack.contains("这一条是例外"));
        assert_eq!(reminder_prompt_with_answer(None), ack);
        assert!(reminder_prompt_with_answer(Some("换个方案")).contains("换个方案"));
    }

    /// The regression this guards: the card body used to be whatever the caller
    /// passed, and both callers pass `last_message_preview` — clipped to 200
    /// chars by `extract_last_text` for the task-list subtitle, so every report
    /// longer than that arrived on the phone cut off mid-sentence.
    #[test]
    fn card_body_carries_the_whole_final_message_not_the_200_char_preview() {
        let _env_guard = crate::session::fleet_home_lock();
        let home = std::env::temp_dir().join(format!("fleet-turncard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &home);

        let long: String = "报".repeat(650);
        let jsonl = home.join("transcript.jsonl");
        std::fs::write(
            &jsonl,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "assistant",
                    "message": { "model": "claude-opus-5", "content": [
                        { "type": "text", "text": long }
                    ]}
                })
            ),
        )
        .unwrap();

        let mut s = session("/p");
        s.jsonl_path = jsonl.to_string_lossy().to_string();
        let preview: String = long.chars().take(200).collect();
        s.last_message_preview = Some(preview.clone());

        let id = maybe_raise(&s, &preview, 0)
            .expect("raise must not error")
            .expect("a plain card-less task turn raises a card");
        let body = crate::elicitation::read_request(&id)
            .expect("the card was written")
            .questions[0]
            .question
            .clone();
        assert!(
            body.contains(&long),
            "card body must hold all {} chars, got {}",
            long.chars().count(),
            body.chars().count()
        );
        crate::elicitation::cleanup(&id);

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// An unreadable or foreign transcript (codex rollout, dsh log) leaves the
    /// card on the preview the caller passed rather than dropping the body.
    #[test]
    fn card_body_falls_back_to_the_preview_when_the_transcript_is_unreadable() {
        let mut s = session("/p");
        s.jsonl_path = "/definitely/not/a/file.jsonl".into();
        assert!(full_last_text(&s).is_none());
        let card = build_turn_card(&s, "done");
        assert!(card.questions[0].question.contains("done"));
    }

    #[test]
    fn a_session_that_relayed_gets_no_completion_card() {
        let _env_guard = crate::session::fleet_home_lock();
        let home = std::env::temp_dir().join(format!("fleet-turncard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &home);

        let s = session("/p");
        // Baseline: without a relay this very session does get a card.
        let card = maybe_raise(&s, "done", 0).expect("raise must not error");
        assert!(card.is_some(), "a plain card-less task turn still cards");
        if let Some(id) = card {
            crate::elicitation::cleanup(&id);
        }

        // Registering a relay retires the session: the successor owns the work,
        // so the turn is not a finished task and must raise nothing.
        crate::handoff::register(
            &s.id,
            &s.workspace_path,
            None,
            "交接给下一棒",
            None,
            None,
            None,
            None,
            None,
            None,
            "claude-code",
        )
        .expect("register must succeed");
        assert!(crate::handoff::has_relayed(&s.id));
        assert!(matches!(maybe_raise(&s, "done", 0), Ok(None)));

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A session whose card the user answered seconds ago must not be wrapped:
    /// the user is at the panel, and resuming the session would undo the
    /// wrap-up they just asked for.
    #[test]
    fn a_just_answered_session_gets_no_completion_card() {
        let _env_guard = crate::session::fleet_home_lock();
        let home = std::env::temp_dir().join(format!("fleet-turncard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &home);

        let s = session("/p");
        // `requested_at` always sits well before the turn boundary we pass to
        // `maybe_raise`, so the card belongs to an *earlier* turn and the
        // `raised_card_since` gate stays out of the way — this test is about
        // `answered_card_within` alone.
        let record = |resolved_at: chrono::DateTime<chrono::Utc>| {
            DecisionHistoryRecord::FleetAsk(crate::decision_history::FleetAskRecord {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: s.id.clone(),
                workspace_name: s.workspace_name.clone(),
                ai_title: None,
                requested_at: (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339(),
                resolved_at: resolved_at.to_rfc3339(),
                outcome: crate::decision_history::FleetAskOutcome::Answered,
                questions: Vec::new(),
                answers: Default::default(),
            })
        };

        // Answered an hour ago: the task genuinely ran on unattended since, so
        // the completion card still fires.
        crate::decision_history::append_record(&record(
            chrono::Utc::now() - chrono::Duration::hours(1),
        ))
        .expect("append must succeed");
        let card = maybe_raise(&s, "done", now_ms()).expect("raise must not error");
        assert!(card.is_some(), "an hour-old answer must not suppress");
        if let Some(id) = card {
            crate::elicitation::cleanup(&id);
        }

        // Answered seconds ago: the user is right here — stay quiet.
        crate::decision_history::append_record(&record(chrono::Utc::now())).expect("append");
        assert!(matches!(maybe_raise(&s, "done", now_ms()), Ok(None)));

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn epoch_ms_parses_rfc3339() {
        assert_eq!(epoch_ms_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_ms_of("not-a-time"), None);
    }
}
