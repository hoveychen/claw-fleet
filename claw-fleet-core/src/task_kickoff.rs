//! Kickoff card — the one way a task's type gets settled.
//!
//! A task starts undecided (`TaskType::General`). Once the conversation has
//! made clear what the job is, the agent calls `fleet__task action=kickoff`
//! with the structured first step of that type's method; Fleet renders it as a
//! Decision Card with three options — 开工 / 改一下再开 / 还没聊完. Only 开工
//! settles the type: the profile is written as boss-approved, the type's plan
//! is created, and the type's workflow comes back in the tool result so it
//! applies from that moment rather than from the next turn's injection.
//! Every other answer goes back to the agent unchanged and writes nothing.
//!
//! Phase 1 knows one type, research: question, sub-questions, budget tier,
//! stop condition and deliverable. The research plan gets one P per
//! sub-question plus an empty findings-ledger table.
//!
//! The card rides the ordinary `fleet__ask` producer (see
//! `mcp_server::handle_task_kickoff_call`). A card that times out is parked and
//! answered later through [`crate::parked`], long after the producer is gone —
//! so the kickoff is also filed under the card's request id
//! (`~/.fleet/task-kickoff/<request_id>.json`) and applied from
//! [`on_parked_answer`] when the boss finally presses 开工.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mcp_ipc::{FleetAskOption, FleetAskQuestion};
use crate::task_profile::{ProfileSetter, TaskType};

/// How much fan-out the research is allowed.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Budget {
    /// The lead looks it up itself.
    #[serde(rename = "self")]
    Solo,
    /// 2–4 sub-agents.
    #[serde(rename = "2-4")]
    Few,
    /// 5 or more sub-agents.
    #[serde(rename = "5+")]
    Many,
}

impl Budget {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "self" | "solo" => Some(Self::Solo),
            "2-4" | "2–4" => Some(Self::Few),
            "5+" => Some(Self::Many),
            _ => None,
        }
    }

    fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Solo, true) => "自己查",
            (Self::Few, true) => "派 2–4 个子 agent",
            (Self::Many, true) => "派 5 个以上子 agent",
            (Self::Solo, false) => "look it up myself",
            (Self::Few, false) => "2–4 sub-agents",
            (Self::Many, false) => "5+ sub-agents",
        }
    }
}

/// Where the research result goes.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Deliverable {
    Wiki,
    Reply,
}

impl Deliverable {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wiki" => Some(Self::Wiki),
            "reply" => Some(Self::Reply),
            _ => None,
        }
    }

    fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Wiki, true) => "wiki 报告",
            (Self::Reply, true) => "对话里的结论",
            (Self::Wiki, false) => "a wiki report",
            (Self::Reply, false) => "a conclusion in the conversation",
        }
    }
}

/// The research kickoff the agent proposes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ResearchKickoff {
    pub question: String,
    pub subquestions: Vec<String>,
    pub budget: Budget,
    pub stop: String,
    pub deliverable: Deliverable,
}

/// Collapse a field to one trimmed line: each lands on a single line of
/// TASKS.md, where an embedded newline would break the block.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_field(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(one_line)
        .filter(|s| !s.is_empty())
}

/// Validate a `kickoff` call. Every field is required: a field the agent cannot
/// fill means the conversation has not settled the job yet, which is exactly
/// what the card is meant to surface.
pub fn parse_research(args: &Value) -> Result<ResearchKickoff, String> {
    let mut problems: Vec<String> = Vec::new();
    let question = text_field(args, "question");
    if question.is_none() {
        problems.push("`question` (the research question, one sentence)".into());
    }
    let subquestions: Vec<String> = match args.get("subquestions") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(one_line)
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    if subquestions.is_empty() {
        problems.push("`subquestions` (an array of at least one sub-question; each becomes a P-task)".into());
    }
    let budget = match text_field(args, "budget") {
        None => {
            problems.push("`budget` (one of self / 2-4 / 5+)".into());
            None
        }
        Some(b) => {
            let parsed = Budget::parse(&b);
            if parsed.is_none() {
                problems.push(format!("`budget` must be one of self / 2-4 / 5+, got `{b}`"));
            }
            parsed
        }
    };
    let stop = text_field(args, "stop");
    if stop.is_none() {
        problems.push("`stop` (the stop condition: when is the answer good enough)".into());
    }
    let deliverable = match text_field(args, "deliverable") {
        None => {
            problems.push("`deliverable` (wiki or reply)".into());
            None
        }
        Some(d) => {
            let parsed = Deliverable::parse(&d);
            if parsed.is_none() {
                problems.push(format!("`deliverable` must be wiki or reply, got `{d}`"));
            }
            parsed
        }
    };
    match (question, budget, stop, deliverable) {
        (Some(question), Some(budget), Some(stop), Some(deliverable)) if problems.is_empty() => {
            Ok(ResearchKickoff {
                question,
                subquestions,
                budget,
                stop,
                deliverable,
            })
        }
        _ => Err(format!(
            "kickoff needs every field filled in — if you cannot fill one, the job is not clear \
             yet, keep talking instead. Missing or invalid: {}",
            problems.join("; ")
        )),
    }
}

/// The option that settles the type, per locale.
fn start_label(zh: bool) -> &'static str {
    if zh {
        "开工"
    } else {
        "Start"
    }
}

/// The card's one question: the kickoff rendered as markdown, and the three
/// options. Only the first option settles the type.
pub fn card_question(k: &ResearchKickoff, locale: &str) -> FleetAskQuestion {
    let zh = locale == "zh";
    let subs = k
        .subquestions
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let (header, question, options) = if zh {
        (
            "开工卡",
            format!(
                "**研究开工卡**——点「开工」后本任务定为研究型，按下面的拆法建 research 计划。\n\n\
                 **研究问题**：{q}\n\n**子问题**\n{subs}\n\n\
                 **预算**：{b}\n\n**停止条件**：{stop}\n\n**交付形式**：{d}",
                q = k.question,
                b = k.budget.label(true),
                stop = k.stop,
                d = k.deliverable.label(true),
            ),
            [
                ("开工", "定为研究型，建 research 计划，按这张卡开始"),
                ("改一下再开", "在下面写要改哪里，改完重新发卡"),
                ("还没聊完", "先不定型，继续沟通"),
            ],
        )
    } else {
        (
            "Kickoff",
            format!(
                "**Research kickoff** — pressing Start makes this a research task and creates a \
                 research plan split as below.\n\n**Question**: {q}\n\n**Sub-questions**\n{subs}\n\n\
                 **Budget**: {b}\n\n**Stop when**: {stop}\n\n**Deliverable**: {d}",
                q = k.question,
                b = k.budget.label(false),
                stop = k.stop,
                d = k.deliverable.label(false),
            ),
            [
                ("Start", "Make this a research task, create the plan and begin"),
                ("Revise first", "Say below what to change; the card comes back revised"),
                ("Not done talking", "Leave the task undecided and keep discussing"),
            ],
        )
    };
    FleetAskQuestion {
        question,
        header: header.to_string(),
        multi_select: false,
        options: options
            .iter()
            .map(|(label, description)| FleetAskOption {
                label: label.to_string(),
                description: description.to_string(),
                preview: None,
            })
            .collect(),
        html: None,
        form_fields: Vec::new(),
        images: Vec::new(),
    }
}

/// True when the boss's answer to the card is exactly the start option. Typed
/// text replaces the picked option on every client, so anything the boss wrote
/// is a revision request, not a start.
pub fn is_start_answer(answer: &str) -> bool {
    let a = answer.trim();
    a == start_label(true) || a.eq_ignore_ascii_case(start_label(false))
}

/// The card's answer, out of a `fleet__ask` answers map keyed on question text.
pub fn answer_of<'a>(
    answers: &'a std::collections::BTreeMap<String, String>,
    question: &str,
) -> Option<&'a str> {
    answers.get(question).map(String::as_str)
}

/// The note appended when the boss did not press start.
pub fn not_started_note(locale: &str) -> String {
    if locale == "zh" {
        "[fleet] 老板没有点「开工」，任务仍是未定，什么都没写。按上面的回答改了再发 kickoff，或继续沟通。"
            .to_string()
    } else {
        "[fleet] The boss did not press Start: the task stays undecided and nothing was written. \
         Revise per the answer above and kick off again, or keep talking."
            .to_string()
    }
}

/// Plan id for the task's research plan: `research-<task id prefix>`, with a
/// numeric suffix when the workspace already has one by that name.
fn plan_id_for(task_id: &str, taken: impl Fn(&str) -> bool) -> String {
    let short: String = task_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    let base = if short.is_empty() {
        "research".to_string()
    } else {
        format!("research-{short}")
    };
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("an unbounded range always yields a free id")
}

/// The body lines filled into the freshly created plan block: the kickoff's
/// budget/stop/deliverable, one P per sub-question, and the empty findings
/// ledger the research workflow records into (its columns follow
/// `task_workflow_guidance::research_lead_*`).
fn plan_body_lines(k: &ResearchKickoff, zh: bool) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(if zh {
        format!(
            "> 开工卡：预算 {}｜停止条件 {}｜交付 {}",
            k.budget.label(true),
            k.stop,
            k.deliverable.label(true)
        )
    } else {
        format!(
            "> Kickoff: budget {} | stop when {} | deliverable {}",
            k.budget.label(false),
            k.stop,
            k.deliverable.label(false)
        )
    });
    lines.push(String::new());
    for (i, s) in k.subquestions.iter().enumerate() {
        lines.push(format!("- [ ] **P{}** — {s}", i + 1));
    }
    lines.push(String::new());
    if zh {
        lines.push("**知识台账**".into());
        lines.push(String::new());
        lines.push("| 结论 | 证据指针 | 类型（数据/文献/推断） | 置信度（高/中/低） | 状态（证实/证伪/不确定） |".into());
        lines.push("|---|---|---|---|---|".into());
        lines.push(String::new());
        lines.push("开放问题 / 矛盾 / 走不通的路：".into());
    } else {
        lines.push("**Findings ledger**".into());
        lines.push(String::new());
        lines.push("| Conclusion | Evidence pointer | Type (data/literature/inference) | Confidence (high/medium/low) | Status (confirmed/refuted/uncertain) |".into());
        lines.push("|---|---|---|---|---|".into());
        lines.push(String::new());
        lines.push("Open questions / contradictions / dead ends:".into());
    }
    lines.push(String::new());
    lines
}

/// Insert `lines` just before plan `plan_id`'s end sentinel.
fn fill_plan(content: &str, plan_id: &str, lines: &[String]) -> Result<String, String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut filled = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(attrs) = crate::prd_tasks::parse_sentinel(trimmed, "begin") {
            in_target = attrs.id.as_deref() == Some(plan_id);
        } else if crate::prd_tasks::parse_sentinel(trimmed, "end").is_some() {
            if in_target && !filled {
                out.extend(lines.iter().cloned());
                filled = true;
            }
            in_target = false;
        }
        out.push(line.to_string());
    }
    if !filled {
        return Err(format!("plan '{plan_id}' not found"));
    }
    let eol = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut joined = out.join(eol);
    if content.ends_with('\n') {
        joined.push_str(eol);
    }
    Ok(joined)
}

/// Settle the task as research: what pressing 开工 does. Writes the profile as
/// boss-approved, creates the `kind=research` plan in `cwd`'s TASKS.md and
/// claims it for `session_id`, and returns the confirmation followed by the
/// research workflow block.
pub fn apply_start(
    session_id: &str,
    cwd: &Path,
    k: &ResearchKickoff,
    title: &str,
    locale: &str,
) -> Result<String, String> {
    use crate::plan_ops as po;
    use crate::task_workflow_guidance as wf;
    let zh = locale == "zh";
    let profile = crate::task_profile::set_for_session(
        session_id,
        Some(TaskType::Research),
        None,
        None,
        ProfileSetter::Boss,
    )?;
    let plan_id = plan_id_for(&profile.task_id, |id| {
        crate::prd_tasks::find_plan_source(cwd, id).is_some()
    });
    let created = po::create(
        cwd,
        &plan_id,
        &k.question,
        None,
        false,
        None,
        crate::prd_tasks::PlanKind::Research,
        Some(session_id),
    )?;
    let path = po::workspace_tasks_path(cwd);
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let filled = fill_plan(&content, &plan_id, &plan_body_lines(k, zh))?;
    std::fs::write(&path, &filled).map_err(|e| format!("write {}: {e}", path.display()))?;
    // `create` claimed the plan while it was still empty; point the focus at P1.
    let _ = po::resume(cwd, &plan_id, None, Some(session_id));

    let parent_note = created
        .message
        .contains("parent '")
        .then(|| created.message.clone());
    let confirm = if zh {
        format!(
            "ok: 老板点了「开工」——任务 {task} 定为研究型。已在 {path} 建 research 计划 \
             '{plan_id}'（P1–P{n} 对应子问题，块里有知识台账表头），焦点已指向 P1。",
            task = profile.task_id,
            path = path.display(),
            n = k.subquestions.len(),
        )
    } else {
        format!(
            "ok: the boss pressed Start — task {task} is now a research task. Created research \
             plan '{plan_id}' in {path} (P1–P{n} are the sub-questions; the block carries the \
             findings-ledger header) and pointed your focus at P1.",
            task = profile.task_id,
            path = path.display(),
            n = k.subquestions.len(),
        )
    };
    let mut out = confirm;
    if let Some(note) = parent_note {
        out.push_str(&format!("\n({note})"));
    }
    for w in created.warnings {
        out.push_str(&format!("\nwarning: {w}"));
    }
    // The task-type ablation's control arm gets no workflow block here either.
    let block = (!wf::flow_block_off(cwd))
        .then(|| wf::render_block(TaskType::Research, wf::Role::Lead, title, locale))
        .flatten();
    if let Some(block) = block {
        out.push_str("\n\n");
        out.push_str(&block);
    }
    Ok(out)
}

// ── Pending kickoffs (for parked cards) ─────────────────────────────────────

/// A kickoff whose card is in flight, keyed by the card's request id.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingKickoff {
    pub session_id: String,
    pub cwd: String,
    /// The card's question text — the key its answer comes back under.
    pub question: String,
    pub title: String,
    pub locale: String,
    pub kickoff: ResearchKickoff,
}

fn pending_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("task-kickoff"))
}

fn pending_path(request_id: &str) -> Option<PathBuf> {
    let valid = !request_id.is_empty()
        && request_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return None;
    }
    pending_dir().map(|d| d.join(format!("{request_id}.json")))
}

/// File the kickoff under its card's request id.
pub fn save_pending(request_id: &str, p: &PendingKickoff) -> Result<(), String> {
    let path = pending_path(request_id).ok_or("cannot resolve the kickoff store path")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(p).map_err(|e| format!("serialize: {e}"))?;
    crate::atomic_json::write_atomic(&path, &json).map_err(|e| format!("write kickoff: {e}"))
}

/// Remove and return the kickoff filed under `request_id`.
pub fn take_pending(request_id: &str) -> Option<PendingKickoff> {
    let path = pending_path(request_id)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&raw).ok()
}

/// What a parked card's answer adds to the resume prompt, when the card was a
/// kickoff. `None` for every other card. On 开工 this settles the task before
/// the session wakes, so the resumed turn already runs as research.
pub fn on_parked_answer(request_id: &str, response: &Value) -> Option<String> {
    let pending = take_pending(request_id)?;
    let answers: std::collections::BTreeMap<String, String> = response
        .get("answers")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let answer = answer_of(&answers, &pending.question).unwrap_or("");
    if !is_start_answer(answer) {
        return Some(not_started_note(&pending.locale));
    }
    Some(
        match apply_start(
            &pending.session_id,
            Path::new(&pending.cwd),
            &pending.kickoff,
            &pending.title,
            &pending.locale,
        ) {
            Ok(text) => text,
            Err(e) => format!("[fleet] 开工没有落地：{e}"),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn full_args() -> Value {
        json!({
            "action": "kickoff",
            "question": "哪种缓存策略最省钱？",
            "subquestions": ["现有缓存命中率", "竞品做法"],
            "budget": "2-4",
            "stop": "两种方案的成本差算清楚",
            "deliverable": "wiki"
        })
    }

    #[test]
    fn every_field_is_required() {
        let k = parse_research(&full_args()).unwrap();
        assert_eq!(k.subquestions.len(), 2);
        assert_eq!((k.budget, k.deliverable), (Budget::Few, Deliverable::Wiki));

        for key in ["question", "subquestions", "budget", "stop", "deliverable"] {
            let mut args = full_args();
            args.as_object_mut().unwrap().remove(key);
            let err = parse_research(&args).unwrap_err();
            assert!(err.contains(&format!("`{key}`")), "{key}: {err}");
        }
        let mut args = full_args();
        args["subquestions"] = json!([]);
        assert!(parse_research(&args).unwrap_err().contains("`subquestions`"));
        args["subquestions"] = json!("not an array");
        assert!(parse_research(&args).is_err());
        let mut args = full_args();
        args["budget"] = json!("lots");
        assert!(parse_research(&args).unwrap_err().contains("got `lots`"));
        let mut args = full_args();
        args["deliverable"] = json!("pdf");
        assert!(parse_research(&args).unwrap_err().contains("got `pdf`"));
        let mut args = full_args();
        args["stop"] = json!("   ");
        assert!(parse_research(&args).unwrap_err().contains("`stop`"));
    }

    #[test]
    fn card_renders_every_field_and_the_three_options() {
        let k = parse_research(&full_args()).unwrap();
        let q = card_question(&k, "zh");
        for needle in ["哪种缓存策略最省钱", "1. 现有缓存命中率", "2. 竞品做法", "派 2–4 个子 agent", "成本差", "wiki 报告"] {
            assert!(q.question.contains(needle), "{needle}: {}", q.question);
        }
        let labels: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["开工", "改一下再开", "还没聊完"]);
        let en = card_question(&k, "en");
        assert_eq!(en.options[0].label, "Start");
    }

    #[test]
    fn only_the_bare_start_option_starts() {
        assert!(is_start_answer("开工"));
        assert!(is_start_answer(" Start "));
        assert!(!is_start_answer("改一下再开"));
        assert!(!is_start_answer("还没聊完"));
        assert!(!is_start_answer("开工，但子问题 2 换成 X"));
        assert!(!is_start_answer(""));
    }

    #[test]
    fn plan_ids_are_derived_from_the_task_and_never_collide() {
        assert_eq!(plan_id_for("9b643bfc-5988", |_| false), "research-9b643bfc");
        assert_eq!(
            plan_id_for("9b643bfc-5988", |id| id == "research-9b643bfc"),
            "research-9b643bfc-2"
        );
        assert_eq!(plan_id_for("", |_| false), "research");
    }

    /// Pressing 开工 settles the profile as research (boss-approved, so the
    /// read-time normalisation keeps it) and creates a research plan with one P
    /// per sub-question and the ledger header.
    #[test]
    fn start_settles_the_type_and_creates_the_research_plan() {
        let home = tempfile::tempdir().unwrap();
        let _home = crate::paths::fleet_home_guard(home.path());
        let ws = tempfile::tempdir().unwrap();
        let k = parse_research(&full_args()).unwrap();

        let out = apply_start("lead-1", ws.path(), &k, "", "zh").unwrap();
        assert!(out.starts_with("ok: 老板点了「开工」"), "{out}");
        assert!(out.contains("本任务的流程：研究"), "carries the research block: {out}");

        let p = crate::task_profile::read("lead-1").unwrap();
        assert_eq!(p.task_type, TaskType::Research);

        let content = std::fs::read_to_string(ws.path().join("TASKS.md")).unwrap();
        let block = crate::prd_tasks::plan_block(&content, "research-lead1").expect("plan");
        assert_eq!(block.kind, crate::prd_tasks::PlanKind::Research);
        assert!(block.body.contains("**Plan:** 哪种缓存策略最省钱？"), "{}", block.body);
        assert!(block.body.contains("- [ ] **P1** — 现有缓存命中率"), "{}", block.body);
        assert!(block.body.contains("- [ ] **P2** — 竞品做法"), "{}", block.body);
        assert!(block.body.contains("**知识台账**"), "{}", block.body);
        assert!(block.body.contains("| 结论 | 证据指针 |"), "{}", block.body);
        assert!(block.body.contains("> 开工卡：预算 派 2–4 个子 agent"), "{}", block.body);
        assert_eq!(crate::prd_tasks::count_tasks(&block.body).1, 2, "{}", block.body);
        let focus = crate::task_progress::read("lead-1").unwrap();
        assert_eq!(focus.plan_id, "research-lead1");

        // A second kickoff in the same workspace gets a fresh plan id.
        apply_start("lead-1", ws.path(), &k, "", "zh").unwrap();
        let content = std::fs::read_to_string(ws.path().join("TASKS.md")).unwrap();
        assert!(crate::prd_tasks::plan_block(&content, "research-lead1-2").is_some());
    }

    /// A parked kickoff card answered later: 开工 settles the task before the
    /// session wakes; anything else writes nothing; either way the pending
    /// record is consumed.
    #[test]
    fn a_parked_kickoff_is_applied_only_on_start() {
        let home = tempfile::tempdir().unwrap();
        let _home = crate::paths::fleet_home_guard(home.path());
        let ws = tempfile::tempdir().unwrap();
        let k = parse_research(&full_args()).unwrap();
        let question = card_question(&k, "zh").question;
        let pending = PendingKickoff {
            session_id: "lead-2".into(),
            cwd: ws.path().to_string_lossy().into_owned(),
            question: question.clone(),
            title: String::new(),
            locale: "zh".into(),
            kickoff: k,
        };

        save_pending("req-a", &pending).unwrap();
        let note = on_parked_answer("req-a", &json!({ "answers": { question.clone(): "还没聊完" } }))
            .expect("a kickoff card");
        assert!(note.contains("任务仍是未定"), "{note}");
        assert!(crate::task_profile::read("lead-2").is_none());
        assert!(!ws.path().join("TASKS.md").exists());

        save_pending("req-b", &pending).unwrap();
        let out = on_parked_answer("req-b", &json!({ "answers": { question: "开工" } })).unwrap();
        assert!(out.contains("知识台账"), "{out}");
        assert_eq!(crate::task_profile::read("lead-2").unwrap().task_type, TaskType::Research);
        assert!(on_parked_answer("req-b", &json!({})).is_none(), "consumed");
        assert!(on_parked_answer("not-a-kickoff", &json!({})).is_none());
    }
}
