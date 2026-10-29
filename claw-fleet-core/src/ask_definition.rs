//! Guard rails for `fleet__ask` questions that settle a *definition*: how a
//! term, label, metric or cohort is measured ("what counts as a comeback?").
//!
//! Why this exists: on 2026-10-28 a research session asked the boss to pick a
//! comeback definition from a menu whose options carried only statistical
//! reasons ("plenty of positives, stable across years"). None said what each
//! definition would misclassify, the definition a domain expert would use was
//! missing from the menu, and the "e.g. 3k" threshold buried in an option
//! description was later frozen by a sub-task and treated downstream as
//! something the boss had approved. The whole study had to be redone.
//!
//! A prompt rule alone did not stop that, so the agent declares the question's
//! `kind`, and a `definition` question must carry, per option, the cases it
//! misjudges (`misjudges`), name the option that matches the domain expert's
//! definition (`expertOption`, which has to be on the menu), and list every
//! numeric threshold it leaves open (`pendingParams`). The server checks this,
//! then folds it into the option descriptions and a footer on the question
//! text, so neither card renderer needs to know about it.
//!
//! A question that reads like a definition question but declares no `kind` is
//! bounced once and asked to declare one (`other` waves it through) — that is
//! the backstop for an agent that forgets to opt in.

use serde_json::Value;

/// `kind` value for a question that defines a term, label or metric.
pub const KIND_DEFINITION: &str = "definition";
/// `kind` value for a question that only picks a value for an already-agreed
/// definition (a threshold, a window). Its options are numbers by design.
pub const KIND_PARAMETER: &str = "parameter";
/// `kind` value that opts out of the definition checks.
pub const KIND_OTHER: &str = "other";

/// First line of the footer folded into a definition question's text. The
/// answer reminder keys on these, so they are part of the contract.
const FOOTER_HEADING_ZH: &str = "**口径说明**（定义类问题）";
const FOOTER_HEADING_EN: &str = "**Definition notes** (definition question)";

/// Words that mark a question as settling a definition. Matched against the
/// header, the ask line and the option labels only — never the report body,
/// which mentions "definition" in unrelated senses far too often.
const DEFINITION_CUES: &[&str] = &[
    "口径", "定义", "怎么算", "算作", "算不算", "阈值", "界定", "判定标准", "打标", "正例", "反例",
    "definition", "define", "threshold", "criterion", "criteria", "labeling", "labelling",
];

/// Comparators and words that, right before a number, make it a threshold.
const THRESHOLD_PREFIXES: &[&str] = &[
    "≥", "≤", ">", "<", "＞", "＜", "超过", "低于", "高于", "至少", "不少于", "不超过", "大于", "小于",
    "比如", "例如",
];

/// Units that, right after a number, make it a threshold.
const THRESHOLD_SUFFIXES: &[&str] = &["k", "K", "%", "％", "分钟", "秒", "min", "倍", "x"];

/// Check every question in a raw `fleet__ask` argument object and fold the
/// definition metadata into renderable text. Removes the fields it consumed
/// (`kind`, `expertOption`, `pendingParams`, options' `misjudges`), so the
/// stored request looks like any other card.
///
/// `Err` carries a message for the agent explaining what to fix.
pub fn vet_and_fold(args: &mut Value, locale: &str) -> Result<(), String> {
    let zh = locale.starts_with("zh");
    let Some(questions) = args.get_mut("questions").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for (idx, q) in questions.iter_mut().enumerate() {
        let Some(q) = q.as_object_mut() else { continue };
        let n = idx + 1;
        let kind = q.remove("kind");
        let expert = q.remove("expertOption");
        let pending = q.remove("pendingParams");
        match kind.as_ref().and_then(Value::as_str) {
            Some(KIND_DEFINITION) => fold_definition(q, n, expert, pending, zh)?,
            Some(KIND_PARAMETER) | Some(KIND_OTHER) => strip_misjudges(q),
            Some(other) => {
                return Err(format!(
                    "Question {n} has unknown `kind` {other:?}: use \"{KIND_DEFINITION}\", \
                     \"{KIND_PARAMETER}\" or \"{KIND_OTHER}\"."
                ));
            }
            None => {
                if let Some(cue) = definition_cue(q) {
                    return Err(format!(
                        "Question {n} looks like it settles a definition (it says {cue:?}) but \
                         declares no `kind`. If the boss is choosing how a term, label, metric or \
                         cohort is defined, set `kind: \"{KIND_DEFINITION}\"` and give every option \
                         `misjudges` (which cases it would wrongly count in or out, in domain terms), \
                         `expertOption` (the label of the option a domain expert would pick — add \
                         that option if it is missing) and `pendingParams` (every numeric threshold \
                         still open). If it only picks a value for an agreed definition, set \
                         `kind: \"{KIND_PARAMETER}\"`. Otherwise set `kind: \"{KIND_OTHER}\"`."
                    ));
                }
                strip_misjudges(q);
            }
        }
    }
    Ok(())
}

fn strip_misjudges(q: &mut serde_json::Map<String, Value>) {
    if let Some(opts) = q.get_mut("options").and_then(Value::as_array_mut) {
        for o in opts.iter_mut().filter_map(Value::as_object_mut) {
            o.remove("misjudges");
        }
    }
}

fn fold_definition(
    q: &mut serde_json::Map<String, Value>,
    n: usize,
    expert: Option<Value>,
    pending: Option<Value>,
    zh: bool,
) -> Result<(), String> {
    let pending: Vec<String> = match pending {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        Some(_) => {
            return Err(format!(
                "Question {n}: `pendingParams` must be an array of strings."
            ));
        }
    };
    let expert = expert
        .as_ref()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "Question {n} is a definition question but has no `expertOption`. Ask yourself \
                 how a domain expert would define this, make sure that definition is one of the \
                 options, and put its label in `expertOption`."
            )
        })?
        .to_string();

    let Some(opts) = q.get_mut("options").and_then(Value::as_array_mut) else {
        return Err(format!(
            "Question {n} is a definition question but has no `options`: list the candidate \
             definitions as options."
        ));
    };
    if opts.len() < 2 {
        return Err(format!(
            "Question {n} is a definition question with fewer than 2 options: a definition is \
             only a choice when the menu has alternatives."
        ));
    }

    let labels: Vec<String> = opts
        .iter()
        .map(|o| o.get("label").and_then(Value::as_str).unwrap_or("").to_string())
        .collect();
    let expert_idx = labels
        .iter()
        .position(|l| same_label(l, &expert))
        .ok_or_else(|| {
            format!(
                "Question {n}: `expertOption` {expert:?} is not one of the option labels ({}). \
                 If the definition a domain expert would use is not on your menu, add it as an \
                 option — a menu without it is the failure this check exists for.",
                labels
                    .iter()
                    .map(|l| format!("{l:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    for (i, o) in opts.iter().enumerate() {
        let misjudges = o.get("misjudges").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if is_empty_misjudgement(misjudges) {
            return Err(format!(
                "Question {n}, option {:?}: a definition option needs `misjudges` — the cases \
                 it would wrongly count as positive or negative, in domain terms (e.g. \"a \
                 late-game composition that is behind early is counted as a comeback\"). \
                 Statistical convenience is not a misjudgement; every definition has one.",
                labels[i]
            ));
        }
        if pending.is_empty() {
            let label = o.get("label").and_then(Value::as_str).unwrap_or("");
            let desc = o.get("description").and_then(Value::as_str).unwrap_or("");
            if let Some(snippet) = numeric_threshold(label).or_else(|| numeric_threshold(desc)) {
                return Err(format!(
                    "Question {n}, option {:?} carries a numeric threshold ({snippet:?}) but \
                     `pendingParams` is empty. A number tucked into an option gets treated as \
                     approved once the option is picked. Either list it in `pendingParams` \
                     (and keep calling it unconfirmed in later cards until the boss confirms \
                     it), or drop it from the option and ask it as its own question with \
                     `kind: \"{KIND_PARAMETER}\"`.",
                    labels[i]
                ));
            }
        }
    }

    for (i, o) in opts.iter_mut().enumerate() {
        let Some(o) = o.as_object_mut() else { continue };
        let misjudges = o
            .remove("misjudges")
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
            .unwrap_or_default();
        let desc = o.get("description").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let tag = if i == expert_idx {
            if zh { "【领域专家口径】" } else { "[Domain-expert definition] " }
        } else {
            ""
        };
        let mis = if zh { "会误判：" } else { "Misjudges: " };
        o.insert(
            "description".into(),
            Value::String(format!("{tag}{desc}\n{mis}{misjudges}")),
        );
    }

    let footer = if zh {
        let params = if pending.is_empty() {
            "无".to_string()
        } else {
            pending.join("；")
        };
        format!(
            "\n\n{FOOTER_HEADING_ZH}\n- 领域专家口径：「{}」\n- 待定参数（未经你确认，选完口径后会单独确认）：{params}",
            labels[expert_idx]
        )
    } else {
        let params = if pending.is_empty() {
            "none".to_string()
        } else {
            pending.join("; ")
        };
        format!(
            "\n\n{FOOTER_HEADING_EN}\n- Domain-expert definition: \"{}\"\n- Open parameters (not confirmed by you; asked separately once the definition is picked): {params}",
            labels[expert_idx]
        )
    };
    let text = q.get("question").and_then(Value::as_str).unwrap_or("").to_string();
    q.insert("question".into(), Value::String(format!("{text}{footer}")));
    Ok(())
}

/// Reminder appended to the tool result once a card that held definition
/// questions is answered. `None` when the card had none.
///
/// Recognises those questions by the footer [`vet_and_fold`] wrote, so it works
/// on a request read back from disk (`wait`) as well as a fresh one.
pub fn answer_reminder(questions: &[crate::mcp_ipc::FleetAskQuestion]) -> Option<String> {
    let footers: Vec<(&str, &str)> = questions
        .iter()
        .filter_map(|q| {
            let text = q.question.as_str();
            text.find(FOOTER_HEADING_ZH)
                .or_else(|| text.find(FOOTER_HEADING_EN))
                .map(|at| (q.header.as_str(), text[at..].trim()))
        })
        .collect();
    if footers.is_empty() {
        return None;
    }
    let listed = footers
        .iter()
        .map(|(h, f)| format!("[{h}]\n{f}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(format!(
        "[fleet] This card settled a definition. Before going on:\n\
         1. Record the chosen definition in your notes (fleet__notes checkpoint.md) as \
         boss-confirmed, and every open parameter below as NOT confirmed. Until the boss \
         confirms a parameter, every later card, report and sub-task brief that relies on it \
         must say it is unconfirmed and who picked it.\n\
         2. The option the boss picked is the contract. If you later run it in a narrower form \
         (\"do both\" turning into one main track plus a side check, a parameter fixed by data), \
         say so plainly on your next card.\n\
         3. If the data contradicts what this definition is meant to capture, stop and raise a \
         card about the definition before building more on it.\n\
         4. Sub-tasks you brief inherit this definition: state where it came from and what is \
         still unconfirmed.\n\n{listed}"
    ))
}

/// The first cue word found in the parts of a question that state the ask.
fn definition_cue(q: &serde_json::Map<String, Value>) -> Option<&'static str> {
    let header = q.get("header").and_then(Value::as_str).unwrap_or("");
    let text = q.get("question").and_then(Value::as_str).unwrap_or("");
    let ask_line = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let labels = q
        .get("options")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|o| o.get("label").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let hay = format!("{header}\n{ask_line}\n{labels}").to_lowercase();
    DEFINITION_CUES.iter().copied().find(|c| hay.contains(c))
}

fn same_label(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> String {
        let mut s = s.trim().to_lowercase();
        for suffix in [" (recommended)", "（recommended）", " (推荐)", "（推荐）"] {
            if let Some(stripped) = s.strip_suffix(suffix) {
                s = stripped.to_string();
            }
        }
        s.trim().to_string()
    }
    norm(a) == norm(b)
}

fn is_empty_misjudgement(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.chars().filter(|c| !c.is_whitespace()).count() < 4
        || ["无", "没有", "none", "n/a", "nothing", "不会误判"].contains(&lower.as_str())
}

/// A number in `s` that reads as a threshold — preceded by a comparator or
/// "e.g.", or followed by a unit — returned with a little context.
fn numeric_threshold(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
            i += 1;
        }
        let end = i;
        let before: String = chars[..start].iter().collect::<String>();
        let before = before.trim_end();
        let after: String = chars[end..].iter().collect::<String>();
        let after = after.trim_start();
        let prefixed = THRESHOLD_PREFIXES
            .iter()
            .any(|p| before.ends_with(p) || before.ends_with(&format!("{p}=")));
        let suffixed = THRESHOLD_SUFFIXES.iter().any(|u| after.starts_with(u));
        if prefixed || suffixed {
            let lo = start.saturating_sub(6);
            let hi = (end + 4).min(chars.len());
            return Some(chars[lo..hi].iter().collect::<String>().trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The 2026-10-28 comeback-definition question, as it was sent.
    fn original_card() -> Value {
        json!({ "questions": [{
            "header": "翻盘口径",
            "multiSelect": false,
            "question": "「局内翻盘」按哪个口径定义？",
            "options": [
                { "label": "经济落后后赢 (Recommended)", "description": "某一时刻经济落后 ≥ X（比如 3k）的一方最终赢下这一局" },
                { "label": "盘口低价后赢", "description": "PM/Kalshi 价格一度跌到 ≤ 0.25 的一方最终赢" },
                { "label": "两种都做", "description": "经济口径当主标签，盘口口径做交易验证" }
            ]
        }]})
    }

    fn rewritten_card() -> Value {
        json!({ "questions": [{
            "header": "翻盘口径",
            "multiSelect": false,
            "kind": "definition",
            "expertOption": "模型胜率低后赢",
            "pendingParams": ["经济落后阈值（候选 2k/3k/5k）", "胜率阈值（候选 25%/30%）"],
            "question": "「局内翻盘」按哪个口径定义？",
            "options": [
                { "label": "模型胜率低后赢 (Recommended)", "description": "考虑阵容、经济、资源的胜率模型一度判落后方胜率低于阈值，最终赢", "misjudges": "模型本身偏差会带进标签；阵容强度估不准时把正常发育算成翻盘" },
                { "label": "经济落后后赢", "description": "某一时刻经济落后超过阈值的一方最终赢", "misjudges": "后期阵容前期落后属正常，会被算成翻盘" },
                { "label": "盘口低价后赢", "description": "盘口价格一度跌破阈值的一方最终赢", "misjudges": "盘口滞后和低流动性造成的假低价会算成翻盘" }
            ]
        }]})
    }

    #[test]
    fn original_comeback_card_is_bounced_for_missing_kind() {
        let mut args = original_card();
        let err = vet_and_fold(&mut args, "zh").unwrap_err();
        assert!(err.contains("declares no `kind`"), "{err}");
    }

    #[test]
    fn original_card_declared_as_definition_still_fails_on_misjudges() {
        let mut args = original_card();
        args["questions"][0]["kind"] = json!("definition");
        args["questions"][0]["expertOption"] = json!("经济落后后赢");
        let err = vet_and_fold(&mut args, "zh").unwrap_err();
        assert!(err.contains("misjudges"), "{err}");
    }

    #[test]
    fn rewritten_card_passes_and_is_folded() {
        let mut args = rewritten_card();
        vet_and_fold(&mut args, "zh").expect("rewritten card passes");
        let q = &args["questions"][0];
        assert!(q.get("kind").is_none() && q.get("expertOption").is_none() && q.get("pendingParams").is_none());
        let first = q["options"][0]["description"].as_str().unwrap();
        assert!(first.starts_with("【领域专家口径】"), "{first}");
        assert!(first.contains("会误判：模型本身偏差"), "{first}");
        assert!(q["options"][1].get("misjudges").is_none());
        let text = q["question"].as_str().unwrap();
        assert!(text.contains(FOOTER_HEADING_ZH));
        assert!(text.contains("经济落后阈值（候选 2k/3k/5k）；胜率阈值"), "{text}");
    }

    #[test]
    fn expert_option_must_be_on_the_menu() {
        let mut args = rewritten_card();
        args["questions"][0]["expertOption"] = json!("阵容感知胜率");
        let err = vet_and_fold(&mut args, "zh").unwrap_err();
        assert!(err.contains("not one of the option labels"), "{err}");
        let mut args = rewritten_card();
        args["questions"][0].as_object_mut().unwrap().remove("expertOption");
        assert!(vet_and_fold(&mut args, "zh").unwrap_err().contains("no `expertOption`"));
    }

    #[test]
    fn threshold_in_option_needs_pending_params() {
        let mut args = rewritten_card();
        args["questions"][0].as_object_mut().unwrap().remove("pendingParams");
        args["questions"][0]["options"][1]["description"] = json!("经济落后 ≥ X（比如 3k）的一方最终赢");
        let err = vet_and_fold(&mut args, "zh").unwrap_err();
        assert!(err.contains("numeric threshold") && err.contains("pendingParams"), "{err}");
    }

    #[test]
    fn placeholder_misjudgement_is_rejected() {
        let mut args = rewritten_card();
        args["questions"][0]["options"][2]["misjudges"] = json!("无");
        assert!(vet_and_fold(&mut args, "zh").unwrap_err().contains("misjudges"));
    }

    #[test]
    fn unrelated_question_passes_untouched() {
        let mut args = json!({ "questions": [{
            "header": "清哪些",
            "multiSelect": false,
            "question": "旧规则产出都找出来了。\n---\n要清到哪一步？",
            "options": [
                { "label": "页面+本机目录 (Recommended)", "description": "删查看器三个规则页" },
                { "label": "只撤页面", "description": "本机目录先留着" }
            ]
        }]});
        let before = args.clone();
        vet_and_fold(&mut args, "zh").expect("no cue, no kind needed");
        assert_eq!(args, before);
    }

    #[test]
    fn cue_in_report_body_alone_does_not_trigger() {
        let mut args = json!({ "questions": [{
            "header": "全量范围",
            "multiSelect": false,
            "question": "第 1 步：经济口径主档是落后 3k。\n---\n全量跑多大？",
            "options": [
                { "label": "先跑 A 层", "description": "22k 段" },
                { "label": "直接跑 A+B", "description": "81k 段" }
            ]
        }]});
        vet_and_fold(&mut args, "zh").expect("body mentions are not the ask");
    }

    #[test]
    fn other_and_parameter_kinds_opt_out() {
        for kind in [KIND_OTHER, KIND_PARAMETER] {
            let mut args = original_card();
            args["questions"][0]["kind"] = json!(kind);
            vet_and_fold(&mut args, "zh").expect("explicit opt-out passes");
            assert!(args["questions"][0].get("kind").is_none());
        }
        let mut args = original_card();
        args["questions"][0]["kind"] = json!("metric");
        assert!(vet_and_fold(&mut args, "zh").unwrap_err().contains("unknown `kind`"));
    }

    #[test]
    fn numeric_threshold_spots_comparators_and_units() {
        assert!(numeric_threshold("落后 ≥ X（比如 3k）").is_some());
        assert!(numeric_threshold("价格 ≤ 0.25").is_some());
        assert!(numeric_threshold("35 分钟以后").is_some());
        assert!(numeric_threshold("PM/Kalshi 两个盘口").is_none());
        assert!(numeric_threshold("第 2 步的结果").is_none());
    }

    #[test]
    fn answer_reminder_reads_the_folded_footer() {
        let mut args = rewritten_card();
        vet_and_fold(&mut args, "zh").unwrap();
        let qs: Vec<crate::mcp_ipc::FleetAskQuestion> =
            serde_json::from_value(args["questions"].clone()).unwrap();
        let note = answer_reminder(&qs).expect("definition card gets a reminder");
        assert!(note.contains("NOT confirmed") && note.contains("[翻盘口径]") && note.contains("经济落后阈值"));

        let plain: Vec<crate::mcp_ipc::FleetAskQuestion> = serde_json::from_value(json!([{
            "header": "h", "question": "q", "options": [{"label": "a", "description": "b"}]
        }]))
        .unwrap();
        assert!(answer_reminder(&plain).is_none());
    }

    #[test]
    fn english_locale_folds_in_english() {
        let mut args = rewritten_card();
        vet_and_fold(&mut args, "en").unwrap();
        let q = &args["questions"][0];
        assert!(q["question"].as_str().unwrap().contains(FOOTER_HEADING_EN));
        assert!(q["options"][0]["description"].as_str().unwrap().contains("Misjudges: "));
    }
}
