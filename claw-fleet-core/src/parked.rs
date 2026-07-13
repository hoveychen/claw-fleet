//! Parked decision cards — what happens when a Decision Card times out.
//!
//! Before this module, a card that nobody answered within `wait_seconds` was
//! simply destroyed: the producer (`fleet mcp` for `fleet__ask` /
//! `fleet__render_a2ui`, the `fleet elicitation` / `fleet plan-approval` hook
//! CLIs) deleted its request file — so the card vanished from the desktop and
//! the phone — and handed the agent an error / a `deny`. The agent, still mid
//! turn, would then do the one thing nobody wanted: retry the question through
//! another channel, or press on with the work *without* the answer it had just
//! declared it needed.
//!
//! For sessions Fleet launched itself, both halves of that are now avoidable,
//! because Fleet owns the process:
//!
//! * **The question survives.** On timeout the producer moves the request into
//!   `~/.fleet/parked/<id>.json` instead of deleting it. The card keeps showing
//!   up in `list_pending_decisions` (flagged `parked`), so it is still sitting
//!   there when the user comes back — hours later, across an app restart.
//! * **The turn stops.** Fleet SIGINTs the session's `claude` process. For a
//!   headless `-p` CLI — which is exactly what
//!   [`crate::session_launch::spawn_new_session`] spawns — that aborts the
//!   in-flight tool call, writes `[Request interrupted by user for tool use]`
//!   into the transcript and exits 0 (see [`crate::session::interrupt_pid_impl`]
//!   for the empirics). No reliance on the agent choosing to behave.
//! * **The answer is delivered later.** When the user finally resolves the card,
//!   [`answer`] spawns `claude --resume <id> -p "<question + answer>"`, so the
//!   agent picks the conversation back up holding the reply it was waiting for.
//!
//! Sessions Fleet did *not* launch keep the old behaviour (delete + error):
//! an interactive CLI attached to a pty reads SIGINT as "quit" and abandons its
//! tool child, and resuming it out from under the user's terminal would fork the
//! conversation. [`parkable_workspace`] is the gate.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which Decision Card channel a parked card came from. Decides how [`answer`]
/// renders the resume prompt, and which pending-list the card rejoins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParkedKind {
    FleetAsk,
    Elicitation,
    A2uiRender,
    PlanApproval,
}

/// A timed-out card, preserved verbatim so it can be re-rendered and answered
/// long after the process that asked the question is gone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParkedCard {
    pub id: String,
    pub kind: ParkedKind,
    pub session_id: String,
    /// The session's launch directory (from its transcript), which is where the
    /// `claude --resume` must run. Captured at park time because the transcript
    /// lookup needs a live `~/.claude/projects` and we want [`answer`] to be a
    /// pure replay of what we already resolved.
    pub workspace_path: String,
    pub parked_at: String,
    /// The original request payload (`FleetAskRequest`, `ElicitationRequest`,
    /// `A2uiRenderRequest` or `PlanApprovalRequest`), verbatim.
    pub request: Value,
}

/// Handed to the agent the moment its card is parked. The SIGINT is what
/// actually stops the turn, but it lands a beat later — this text is what the
/// agent reads first, and it must not leave "retry" or "carry on" open.
pub const STOP_NOTICE: &str = "\
决策卡等待超时，问题已被挂起在 Fleet 里等老板回复——它没有丢失，仍然显示在桌面端和手机上。

本轮到此为止。不要重试这个工具，不要改用别的方式重新提问，也不要在没有答复的情况下自行继续执行。\
请立刻结束本轮，不要再调用任何工具。老板回复后 Fleet 会自动 resume 这个会话，并把回复原样交给你，你再从这里继续。";

// ── Store ────────────────────────────────────────────────────────────────────

fn parked_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("parked"))
}

fn card_path(id: &str) -> Option<PathBuf> {
    parked_dir().map(|d| d.join(format!("{id}.json")))
}

/// The workspace a card for `session_id` could be resumed in — `Some` only when
/// Fleet launched the session itself (its transcript carries a Fleet
/// entrypoint) *and* that transcript tells us where it was launched.
///
/// This is the single gate for the whole feature: a `None` here means the
/// producer falls back to the historical delete-and-error path.
pub fn parkable_workspace(session_id: &str) -> Option<String> {
    if session_id.trim().is_empty() {
        return None;
    }
    let entrypoint = crate::session::session_entrypoint(session_id);
    if !crate::session_launch::is_fleet_owned_entrypoint(entrypoint.as_deref()) {
        return None;
    }
    crate::session::resolve_session_cwd(session_id)
}

/// Move a timed-out request into the parked store. `request` is the channel's
/// own request struct; it is stored verbatim and handed back to the desktop.
pub fn park(
    id: &str,
    kind: ParkedKind,
    session_id: &str,
    workspace_path: &str,
    request: &impl Serialize,
) -> Result<ParkedCard, String> {
    let dir = parked_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create parked dir: {e}"))?;
    let card = ParkedCard {
        id: id.to_string(),
        kind,
        session_id: session_id.to_string(),
        workspace_path: workspace_path.to_string(),
        parked_at: chrono::Utc::now().to_rfc3339(),
        request: serde_json::to_value(request).map_err(|e| format!("serialize request: {e}"))?,
    };
    let path = card_path(id).unwrap();
    let json = serde_json::to_string_pretty(&card).map_err(|e| format!("serialize card: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write parked card: {e}"))?;
    Ok(card)
}

pub fn get(id: &str) -> Option<ParkedCard> {
    let path = card_path(id)?;
    serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()
}

/// Every parked card, oldest first. Soft: unreadable entries are skipped rather
/// than taking the whole listing down.
pub fn list() -> Vec<ParkedCard> {
    let Some(dir) = parked_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut cards: Vec<ParkedCard> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str::<ParkedCard>(&raw).ok())
        .collect();
    cards.sort_by(|a, b| a.parked_at.cmp(&b.parked_at));
    cards
}

/// Parked cards of one kind, with their request payloads deserialized back into
/// the channel's own request type. This is what each `/x/pending` listing folds
/// into its live requests.
pub fn list_requests<T: for<'de> Deserialize<'de>>(kind: ParkedKind) -> Vec<T> {
    list()
        .into_iter()
        .filter(|c| c.kind == kind)
        .filter_map(|c| serde_json::from_value::<T>(c.request).ok())
        .collect()
}

/// True when this session already has a card waiting for the user. The producers
/// use it as a re-entry guard: an agent that ignored [`STOP_NOTICE`] and asked
/// again must not get a second card queued behind the first — it gets the notice
/// straight back instead.
pub fn has_parked_for_session(session_id: &str) -> bool {
    !session_id.trim().is_empty()
        && list().iter().any(|c| c.session_id == session_id)
}

pub fn is_parked(id: &str) -> bool {
    card_path(id).map(|p| p.is_file()).unwrap_or(false)
}

/// Drop a parked card without resuming anything — the user dismissed the
/// question rather than answering it.
pub fn discard(id: &str) -> Result<(), String> {
    let Some(path) = card_path(id) else {
        return Err("cannot determine home dir".into());
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove parked card: {e}")),
    }
}

// ── Interrupting the session ─────────────────────────────────────────────────

/// The pid of the `claude` process running `session_id`, if it is still up.
///
/// Only the exact-argv match is accepted (`--session-id <id>` / `--resume <id>`,
/// which every Fleet spawn carries). The looser cwd-based fallbacks in
/// [`crate::session::resolve_pid`] would happily hand back *some other* session
/// in the same workspace — fine for a status badge, catastrophic for a SIGINT.
pub fn session_pid(session_id: &str) -> Option<u32> {
    crate::session::scan_cli_processes()
        .into_iter()
        .find(|p| p.resume_session_id.as_deref() == Some(session_id))
        .map(|p| p.pid)
}

/// SIGINT the session's CLI and wait (briefly) for it to go. Returns `true` when
/// it was running and is now gone.
///
/// Called twice in a card's life: once at park time (stop the turn that asked),
/// and again from [`answer`] as a safety net — if the first SIGINT never landed,
/// resuming would put a second `claude` on the same transcript.
pub fn interrupt_session(session_id: &str) -> bool {
    let Some(pid) = session_pid(session_id) else {
        return false;
    };
    if let Err(e) = crate::session::interrupt_pid_impl(pid) {
        crate::log_debug(&format!("parked: interrupt {session_id} (pid {pid}): {e}"));
        return false;
    }
    wait_for_exit(pid, Duration::from_secs(8))
}

fn wait_for_exit(pid: u32, budget: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < budget {
        if !crate::session::is_process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    crate::log_debug(&format!(
        "parked: pid {pid} still alive {budget:?} after SIGINT"
    ));
    false
}

// ── Answering: resume the session with the reply ─────────────────────────────

/// Resolve a parked card with the user's response and wake the session up.
///
/// `response` is the channel's own response payload — `FleetAskResponse`,
/// `ElicitationResponse`, `A2uiRenderResponse` or `PlanApprovalResponse` — the
/// same value the desktop would have written to the (long-gone) response file.
/// It is rendered, together with the original question, into the prompt of a
/// `claude --resume`.
pub fn answer(id: &str, response: &Value) -> Result<(), String> {
    let card = get(id).ok_or_else(|| format!("no parked card {id}"))?;

    // The turn that asked should already be dead (we SIGINT'd it at park time).
    // If it isn't — the signal never landed, or the card was parked by an older
    // build — kill it before resuming: two `claude` processes appending to one
    // transcript is how you corrupt a session.
    if session_pid(&card.session_id).is_some() {
        crate::log_debug(&format!(
            "parked: session {} still running at answer time; interrupting before resume",
            card.session_id
        ));
        interrupt_session(&card.session_id);
    }

    let prompt = build_resume_prompt(&card, response);
    crate::auto_resume::spawn_resume_prompt(
        &card.session_id,
        &card.workspace_path,
        &prompt,
        // No model / effort / permission-mode override: `--resume` inherits the
        // CLI's configured default, exactly like the desktop's own 「恢复会话」
        // box and auto-resume do. The session's original `--model` spec is not
        // recoverable from the transcript (it records a bare model id, dropping
        // suffixes like `[1m]`), so guessing one here could silently downgrade
        // the session.
        None,
        None,
        None,
    )?;
    discard(id)
}

/// Render "here is what you asked, here is what the boss said" for the resume
/// prompt. The agent reads this as a fresh user turn, so it has to carry enough
/// context to stand alone — the tool call it came from was interrupted and never
/// produced a result.
fn build_resume_prompt(card: &ParkedCard, response: &Value) -> String {
    let mut out = String::from(
        "[Fleet] 你上一轮通过决策卡向老板提问，等待超时，那一轮已被中断。老板现在回复了：\n\n",
    );
    match card.kind {
        ParkedKind::FleetAsk | ParkedKind::Elicitation => {
            render_question_answers(&mut out, &card.request, response);
        }
        ParkedKind::A2uiRender => {
            let action = response
                .get("actionName")
                .and_then(|v| v.as_str())
                .unwrap_or("(未触发任何 Action)");
            out.push_str(&format!("【A2UI 操作】{action}\n"));
            if let Some(ctx) = response.get("actionContext").and_then(|v| v.as_object()) {
                for (k, v) in ctx {
                    out.push_str(&format!("【{k}】{}\n", value_text(v)));
                }
            }
        }
        ParkedKind::PlanApproval => {
            let decision = response
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            out.push_str(&format!("【计划审批】{decision}\n"));
            if let Some(edited) = response.get("editedPlan").and_then(|v| v.as_str()) {
                out.push_str(&format!("【老板改过的计划】\n{edited}\n"));
            }
            if let Some(fb) = response.get("feedback").and_then(|v| v.as_str()) {
                out.push_str(&format!("【老板的意见】{fb}\n"));
            }
        }
    }
    out.push_str(
        "\n请直接基于这个回复，从被中断的地方继续之前的工作——不要重新问一遍同样的问题。",
    );
    out
}

/// `answers` is a flat map on every ask-shaped channel: question text → picked
/// option label, and form-field name → value, in the same map. Pair each entry
/// back with its question so the agent sees "问 X / 答 Y" rather than a bare map.
fn render_question_answers(out: &mut String, request: &Value, response: &Value) {
    let answers: BTreeMap<String, String> = response
        .get("answers")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let questions = request
        .get("questions")
        .and_then(|q| q.as_array())
        .cloned()
        .unwrap_or_default();

    let mut rendered: Vec<&String> = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        let text = q.get("question").and_then(|v| v.as_str()).unwrap_or("");
        let header = q.get("header").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("【问题 {}｜{}】{}\n", i + 1, header, text.trim()));
        match answers.get_key_value(text) {
            Some((k, v)) => {
                out.push_str(&format!("【老板的选择】{v}\n\n"));
                rendered.push(k);
            }
            None => out.push_str("【老板的选择】(未作答)\n\n"),
        }
    }

    // Form-field answers (and anything else the card collected) key on the field
    // name, not on a question, so they never match above.
    for (k, v) in &answers {
        if !rendered.contains(&k) {
            out.push_str(&format!("【{k}】{v}\n"));
        }
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TmpHome {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-parked-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialized on the process-wide FLEET_HOME lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }

        /// Plant a transcript for `session_id` carrying `entrypoint` + `cwd`,
        /// exactly as the Claude CLI would write it.
        fn plant_session(&self, session_id: &str, entrypoint: Option<&str>, cwd: &str) {
            let proj = self.dir.join(".claude").join("projects").join("proj");
            fs::create_dir_all(&proj).unwrap();
            let mut rec = json!({ "type": "user", "cwd": cwd });
            if let Some(e) = entrypoint {
                rec["entrypoint"] = json!(e);
            }
            fs::write(
                proj.join(format!("{session_id}.jsonl")),
                format!("{rec}\n"),
            )
            .unwrap();
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn fleet_ask_request(id: &str, session_id: &str) -> crate::mcp_ipc::FleetAskRequest {
        crate::mcp_ipc::FleetAskRequest {
            id: id.into(),
            session_id: session_id.into(),
            workspace_name: "claude-fleet".into(),
            ai_title: None,
            timestamp: "2026-07-13T00:00:00Z".into(),
            questions: vec![crate::mcp_ipc::FleetAskQuestion {
                question: "要不要保留向后兼容？".into(),
                header: "兼容性".into(),
                multi_select: false,
                options: vec![],
                html: None,
                form_fields: vec![],
                images: vec![],
            }],
        }
    }

    /// The gate: only a session Fleet launched itself may be parked. A session
    /// the user started in their own terminal reads SIGINT as "quit" and cannot
    /// be resumed out from under them, so it must keep the old delete-and-error
    /// path — a `None` here is what routes it there.
    #[test]
    fn only_fleet_owned_sessions_are_parkable() {
        let home = TmpHome::new("gate");
        home.plant_session("fleet-one", Some(crate::session_launch::NEW_SESSION_ENTRYPOINT), "/ws/a");
        home.plant_session("handoff-one", Some(crate::handoff::HANDOFF_ENTRYPOINT), "/ws/b");
        home.plant_session("terminal-one", Some("cli"), "/ws/c");
        home.plant_session("no-entrypoint", None, "/ws/d");

        assert_eq!(parkable_workspace("fleet-one").as_deref(), Some("/ws/a"));
        assert_eq!(parkable_workspace("handoff-one").as_deref(), Some("/ws/b"));
        assert_eq!(parkable_workspace("terminal-one"), None);
        assert_eq!(parkable_workspace("no-entrypoint"), None);
        assert_eq!(parkable_workspace("never-existed"), None);
        assert_eq!(parkable_workspace(""), None);
    }

    /// A parked card outlives the process that asked the question: it must round
    /// trip through disk with its request intact, stay listed until resolved, and
    /// come back typed so the pending listing can re-render the very same card.
    #[test]
    fn park_survives_and_relists_as_its_own_request_type() {
        let _home = TmpHome::new("store");
        let req = fleet_ask_request("card-1", "sess-1");
        park("card-1", ParkedKind::FleetAsk, "sess-1", "/ws/a", &req).unwrap();

        assert!(is_parked("card-1"));
        assert!(has_parked_for_session("sess-1"));
        assert!(!has_parked_for_session("sess-other"));

        let back: Vec<crate::mcp_ipc::FleetAskRequest> = list_requests(ParkedKind::FleetAsk);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, "card-1");
        assert_eq!(back[0].questions[0].question, "要不要保留向后兼容？");
        // A card of another kind must not leak into this channel's listing.
        assert!(list_requests::<crate::elicitation::ElicitationRequest>(ParkedKind::Elicitation).is_empty());

        discard("card-1").unwrap();
        assert!(!is_parked("card-1"));
        assert!(list().is_empty());
        // Discarding twice is not an error — the desktop and the phone can both
        // resolve the same card.
        discard("card-1").unwrap();
    }

    /// The resume prompt is the agent's *only* view of what happened: the tool
    /// call it asked from was interrupted and never returned a result. So it has
    /// to restate the question, attach the boss's pick, carry any form fields,
    /// and tell the agent not to ask again.
    #[test]
    fn resume_prompt_restates_question_with_answer() {
        let _home = TmpHome::new("prompt");
        let req = fleet_ask_request("card-2", "sess-2");
        let card = park("card-2", ParkedKind::FleetAsk, "sess-2", "/ws/a", &req).unwrap();

        let resp = json!({
            "id": "card-2",
            "answers": {
                "要不要保留向后兼容？": "保留",
                "commit_msg": "fix: 兼容旧字段"
            },
            "cancelled": false
        });
        let prompt = build_resume_prompt(&card, &resp);

        assert!(prompt.contains("要不要保留向后兼容？"), "{prompt}");
        assert!(prompt.contains("【老板的选择】保留"), "{prompt}");
        // Form fields key on the field name, not a question — they must still
        // reach the agent rather than being silently dropped.
        assert!(prompt.contains("【commit_msg】fix: 兼容旧字段"), "{prompt}");
        assert!(prompt.contains("不要重新问一遍同样的问题"), "{prompt}");
        // An answered question is rendered once, under its question — never a
        // second time as a bare `【<question text>】<answer>` k/v line.
        assert!(!prompt.contains("【要不要保留向后兼容？】"), "{prompt}");
    }

    #[test]
    fn resume_prompt_renders_plan_and_a2ui_channels() {
        let _home = TmpHome::new("prompt-kinds");

        let plan = park(
            "card-3",
            ParkedKind::PlanApproval,
            "sess-3",
            "/ws/a",
            &json!({ "id": "card-3", "planContent": "# 计划" }),
        )
        .unwrap();
        let p = build_resume_prompt(
            &plan,
            &json!({ "id": "card-3", "decision": "approve", "feedback": "先做 P1" }),
        );
        assert!(p.contains("【计划审批】approve"), "{p}");
        assert!(p.contains("【老板的意见】先做 P1"), "{p}");

        let a2ui = park(
            "card-4",
            ParkedKind::A2uiRender,
            "sess-4",
            "/ws/a",
            &json!({ "id": "card-4", "messageTree": {} }),
        )
        .unwrap();
        let a = build_resume_prompt(
            &a2ui,
            &json!({ "actionName": "submit", "actionContext": { "score": "7" } }),
        );
        assert!(a.contains("【A2UI 操作】submit"), "{a}");
        assert!(a.contains("【score】7"), "{a}");
    }

    /// A card whose question the boss never answered (unanswered question in a
    /// multi-question card) must say so rather than silently omitting it — the
    /// agent needs to know which half it still doesn't have.
    #[test]
    fn resume_prompt_marks_unanswered_questions() {
        let _home = TmpHome::new("prompt-unanswered");
        let req = fleet_ask_request("card-5", "sess-5");
        let card = park("card-5", ParkedKind::FleetAsk, "sess-5", "/ws/a", &req).unwrap();
        let prompt = build_resume_prompt(&card, &json!({ "id": "card-5", "answers": {} }));
        assert!(prompt.contains("(未作答)"), "{prompt}");
    }

    #[test]
    fn interrupt_and_answer_are_noops_without_a_live_session() {
        let _home = TmpHome::new("no-proc");
        // No claude process carries this id, so there is nothing to signal.
        assert!(!interrupt_session("sess-ghost"));
        assert_eq!(session_pid("sess-ghost"), None);
        // Answering a card that was never parked is an error, not a panic.
        assert!(answer("nope", &json!({})).is_err());
    }
}
