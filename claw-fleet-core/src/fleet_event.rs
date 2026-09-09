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

pub fn annotate(message: &mut Value) {
    if message.get("fleetEvent").is_some() || message.get("isMeta") == Some(&Value::Bool(true)) {
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
