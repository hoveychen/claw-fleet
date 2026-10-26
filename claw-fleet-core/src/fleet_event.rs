//! Classify prompts Fleet injects to drive watches, handoffs, loops and
//! schedules. Harnesses persist every prompt as a user turn; this annotation
//! lets clients render the turn honestly as a passive automation event.

use serde_json::{json, Value};

fn user_text(message: &Value) -> Option<String> {
    if message.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = message.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let text = content
        .as_array()?
        .iter()
        .filter_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| block.get("text").and_then(Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

fn id_after(text: &str, marker: &str) -> Option<String> {
    let tail = text.split_once(marker)?.1;
    let id = tail.split('`').next()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Metadata for one exact Fleet-owned prompt template. Requiring both its
/// header and its automation-specific footer avoids classifying a user's
/// ordinary mention of `Fleet watch` / `handoff` as an event.
pub fn classify(text: &str) -> Option<Value> {
    if text.starts_with("你注册的 Fleet watch `")
        && text.contains("这是 Fleet watch 在后台轮询到条件后自动 resume 本会话的")
    {
        return Some(json!({
            "kind": "watch",
            "status": if text.contains("已超时——") { "timeout" } else { "fired" },
            "id": id_after(text, "你注册的 Fleet watch `")
        }));
    }
    if text.starts_with("你是一次接力开发的第 ")
        && text.contains("上一棒留下的交接信息：\n\n---\n")
        && text.contains("接手第一件事：")
    {
        return Some(json!({ "kind": "handoff", "status": "successor" }));
    }
    // A Decision Card the user answered after its turn was gone — it timed out
    // and the turn was SIGINTed, or it stepped aside for a message and the turn
    // then ended: `parked::answer` resumes the session with the question and the
    // reply. See `parked::build_resume_prompt` for both templates.
    if crate::parked::is_resume_prompt(text) {
        return Some(json!({ "kind": "decision", "status": "answered" }));
    }
    // `plan_revive` waking a fresh session onto a plan nobody was driving.
    if text.starts_with(crate::plan_revive::REVIVE_PROMPT_HEADER)
        && text.contains("注意：Fleet 连续唤醒这个计划")
    {
        return Some(json!({ "kind": "handoff", "status": "revived" }));
    }
    if text.contains("\n\n---\n（这是 Fleet 循环 `")
        && (text.contains("无人值守") || text.contains("老板现在手动跑了一次"))
    {
        return Some(json!({
            "kind": "loop",
            "status": if text.contains("一次**手动运行**") { "manual" } else { "fired" },
            "id": id_after(text, "（这是 Fleet 循环 `")
        }));
    }
    if text.contains("\n\n---\n（这是 Fleet 定时任务 `")
        && (text.contains("无人值守") || text.contains("老板现在手动跑了一次"))
    {
        return Some(json!({
            "kind": "schedule",
            "status": if text.contains("一次**手动运行**") { "manual" } else { "fired" },
            "id": id_after(text, "（这是 Fleet 定时任务 `")
        }));
    }
    None
}

/// The event for a message whose *origin* is known to be Fleet or another
/// agent — a peer-origin socket delivery without the user's signature, or a
/// row in Fleet's agent frame. A known template keeps its own kind; anything
/// else is a generic notice, so it never reads as the user typing. Mail (a
/// reply-address footer) is `None`: the clients draw it as a mail card.
pub fn agent_event(text: &str) -> Option<Value> {
    if text.contains(crate::mcp_inspect::REPLY_ADDRESS_LEAD) {
        return None;
    }
    classify(text).or_else(|| Some(json!({ "kind": "notice", "status": "delivered" })))
}

/// Drop Fleet's agent frames from a `user` row's text blocks, and mark the row
/// as an agent event when nothing but frames was in it.
///
/// A framed row reaches the transcript two ways: an agent message written to a
/// turn's stdin that opened a turn instead of being absorbed mid-turn, and a
/// queued agent message drained as (part of) a resume prompt — for any agent
/// source, which is why this runs from [`annotate`] too. A turn that mixes
/// frames with the user's own words stays the user's.
pub fn unframe_agent_row(message: &mut Value) {
    if message.get("type").and_then(Value::as_str) != Some("user") {
        return;
    }
    let content = &mut message["message"]["content"];
    let mut any = false;
    let mut only_frames = true;
    if let Some((body, whole)) = content.as_str().and_then(crate::live_inject::strip_agent_frames) {
        *content = json!(body);
        any = true;
        only_frames = whole;
    } else if let Some(blocks) = content.as_array_mut() {
        for block in blocks.iter_mut() {
            let Some(text) = block.get("text").and_then(Value::as_str) else {
                continue;
            };
            match crate::live_inject::strip_agent_frames(text) {
                Some((body, whole)) => {
                    block["text"] = json!(body);
                    any = true;
                    only_frames &= whole;
                }
                None if text.trim().is_empty() => {}
                // A text block outside any frame is somebody's own words.
                None => only_frames = false,
            }
        }
    }
    if !any || !only_frames || message.get("fleetEvent").is_some() {
        return;
    }
    if let Some(event) = user_text(message).as_deref().and_then(agent_event) {
        message["fleetEvent"] = event;
    }
}

pub fn annotate(message: &mut Value) {
    if message.get("fleetEvent").is_some() || message.get("isMeta") == Some(&Value::Bool(true)) {
        return;
    }
    unframe_agent_row(message);
    if message.get("fleetEvent").is_some() {
        return;
    }
    let Some(text) = user_text(message) else {
        return;
    };
    if let Some(event) = classify(&text) {
        message["fleetEvent"] = event;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_all_automation_templates_without_matching_mentions() {
        let watch = "你注册的 Fleet watch `abc` 触发了——等待的条件已满足。\n\n---\n（这是 Fleet watch 在后台轮询到条件后自动 resume 本会话的，不是用户消息。）";
        let handoff = "你是一次接力开发的第 2 棒，接替上一个 session（s1）继续未完成的工作。\n\n上一棒留下的交接信息：\n\n---\nnote\n---\n\n接手第一件事：先验证。";
        let loop_prompt = "do work\n\n---\n（这是 Fleet 循环 `lp1` 的第 2 次迭代。本会话是**无人值守**的自动触发。）";
        let schedule = "ship\n\n---\n（这是 Fleet 定时任务 `sc1`：现在到点了。本会话是**无人值守**的自动触发。）";
        assert_eq!(classify(watch).unwrap()["kind"], "watch");
        assert_eq!(classify(handoff).unwrap()["kind"], "handoff");
        assert_eq!(classify(loop_prompt).unwrap()["id"], "lp1");
        assert_eq!(classify(schedule).unwrap()["id"], "sc1");
        assert!(classify("能不能把 Fleet watch 消息画成卡片？").is_none());

        // The literal header `parked::build_resume_prompt` emits.
        let decision =
            crate::parked::RESUME_PROMPT_HEADER.to_string() + "【问题 1】发版？\n【回答】发";
        let got = classify(&decision).unwrap();
        assert_eq!(got["kind"], "decision");
        assert_eq!(got["status"], "answered");
    }

    #[test]
    fn classifies_both_decision_resume_headers() {
        for header in [
            crate::parked::RESUME_PROMPT_HEADER,
            crate::parked::STEPPED_ASIDE_RESUME_HEADER,
        ] {
            let prompt = header.to_string() + "【问题 1】发版？\n【回答】发";
            let got = classify(&prompt).unwrap_or_else(|| panic!("not classified: {header}"));
            assert_eq!(got["kind"], "decision", "{header}");
            assert_eq!(got["status"], "answered", "{header}");
        }
        assert!(classify("老板现在回答了这张卡").is_none());
    }

    fn user_row(content: Value) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": content}})
    }

    /// A queued agent message drained as a resume prompt — any agent source,
    /// the row's content as a bare string (codex) or as text blocks (claude).
    #[test]
    fn a_drained_agent_frame_becomes_a_notice() {
        let notice = crate::ask_preempt::resolved_notice("req-9");
        let framed = crate::live_inject::frame_as_agent(&notice);
        for content in [json!(framed.clone()), json!([{"type": "text", "text": framed}])] {
            let mut msg = user_row(content);
            annotate(&mut msg);
            assert_eq!(msg["fleetEvent"]["kind"], "notice");
            assert_eq!(user_text(&msg).unwrap(), notice);
        }

        // Two queued agent messages joined into one prompt are still Fleet's.
        let two = format!(
            "{}\n\n{}",
            crate::live_inject::frame_as_agent("a"),
            crate::live_inject::frame_as_agent("b")
        );
        let mut msg = user_row(json!(two));
        annotate(&mut msg);
        assert_eq!(msg["fleetEvent"]["kind"], "notice");
        assert_eq!(msg["message"]["content"], "a\n\nb");
    }

    #[test]
    fn a_drained_frame_keeps_its_own_template_kind() {
        let watch = "你注册的 Fleet watch `w9` 触发了——x\n\n---\n（这是 Fleet watch 在后台轮询到条件后自动 resume 本会话的，不是用户消息。）";
        let mut msg = user_row(json!(crate::live_inject::frame_as_agent(watch)));
        annotate(&mut msg);
        assert_eq!(msg["fleetEvent"]["kind"], "watch");
        assert_eq!(msg["fleetEvent"]["id"], "w9");
    }

    /// The user's words queued next to an agent's: the turn stays theirs, with
    /// the frame lifted out of the bubble.
    #[test]
    fn a_frame_beside_the_users_words_stays_a_bubble() {
        let text = format!("改成蓝色\n\n{}", crate::live_inject::frame_as_agent("notice"));
        let mut msg = user_row(json!([{"type": "text", "text": text}]));
        annotate(&mut msg);
        assert!(msg.get("fleetEvent").is_none());
        assert_eq!(msg["message"]["content"][0]["text"], "改成蓝色\n\nnotice");
    }

    #[test]
    fn framed_mail_stays_mail() {
        let mail = "看下日志\n\n---\n（这条消息来自 Fleet 会话 s1（ws）。要回复它，用 `fleet__control action=send id=s1 text=<回复>`（CLI：`fleet send s1 <回复>`）。）";
        let mut msg = user_row(json!(crate::live_inject::frame_as_agent(mail)));
        annotate(&mut msg);
        assert!(msg.get("fleetEvent").is_none());
        assert_eq!(msg["message"]["content"], mail);
    }

    /// Origin, not wording: the user typing Fleet's notice text is the user.
    #[test]
    fn the_users_own_fleet_lookalike_text_is_not_an_event() {
        let mut msg = user_row(json!(crate::ask_preempt::resolved_notice("r1")));
        annotate(&mut msg);
        assert!(msg.get("fleetEvent").is_none());
        let mut msg = user_row(json!(format!("{}hi", crate::live_inject::AGENT_FRAME_HEAD)));
        annotate(&mut msg);
        assert!(msg.get("fleetEvent").is_none());
    }

    #[test]
    fn classifies_a_plan_revive_prompt() {
        let prompt = crate::plan_revive::REVIVE_PROMPT_HEADER.to_string()
            + "计划 x 还有 1 个 P-task 没完成。\n\n注意：Fleet 连续唤醒这个计划 3 次都没勾掉任何一个框，就会自动静默并去问老板。";
        let got = classify(&prompt).unwrap();
        assert_eq!(got["kind"], "handoff");
        assert_eq!(got["status"], "revived");
        assert!(classify(crate::plan_revive::REVIVE_PROMPT_HEADER).is_none());
    }

    #[test]
    fn annotates_a_user_record_but_not_meta_context() {
        let mut msg = json!({"type":"user","message":{"role":"user","content":"你注册的 Fleet watch `w1` 已超时——x\n\n---\n（这是 Fleet watch 在后台轮询到条件后自动 resume 本会话的，不是用户消息。）"}});
        annotate(&mut msg);
        assert_eq!(msg["fleetEvent"]["status"], "timeout");

        let mut meta = msg.clone();
        meta.as_object_mut().unwrap().remove("fleetEvent");
        meta["isMeta"] = json!(true);
        annotate(&mut meta);
        assert!(meta.get("fleetEvent").is_none());
    }
}
