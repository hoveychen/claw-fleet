//! Review session (P6) — the `/goal`-style gate that fills the orchestrator's
//! `ReviewGate` seam.
//!
//! After a worker exits, the orchestrator runs an isolated, read-only review
//! session to judge whether the P-item's acceptance criteria were *actually*
//! met — defeating the "worker reports done without doing it" failure mode the
//! old five-gate tried (and failed) to catch deterministically.
//!
//! The session is a `claude -p --output-format json --json-schema <schema>`
//! call (verified flags). Its stdout is parsed by [`parse_verdict`] into a
//! [`ReviewVerdict`]. This module owns the prompt / schema / parse (all pure
//! and unit-testable); the actual spawn + stdout capture + `git diff`
//! extraction live in the `fleet-task` `RealReviewGate`.

use crate::orchestrator::ReviewVerdict;
use crate::pitem::{AcceptanceCriterion, PItem};
use crate::spawn_specs::ReviewSpawnSpec;
use crate::task::Task;
use std::path::PathBuf;

/// Review judgment runs on a capable model — false "achieved" verdicts are the
/// expensive mistake.
pub const REVIEW_MODEL: &str = "claude-sonnet-4-6";

/// JSON Schema passed via `--json-schema` to constrain the session's output to
/// exactly `{ achieved: bool, gaps: string[] }`.
pub const REVIEW_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "achieved": { "type": "boolean" },
    "gaps": { "type": "array", "items": { "type": "string" } }
  },
  "required": ["achieved", "gaps"]
}"#;

/// Build the review session spec: the system prompt carries the judging
/// contract + the P-item's `desc` / `acceptance` / the worker's
/// `output_summary` / the worktree `diff` to judge against.
pub fn review_spawn_spec(task: &Task, p_item: &PItem, diff: &str, cwd: PathBuf) -> ReviewSpawnSpec {
    ReviewSpawnSpec {
        task_id: task.id.clone(),
        p_item_id: p_item.id.clone(),
        cwd,
        system_prompt: compose_review_prompt(p_item, diff),
        model: REVIEW_MODEL.to_string(),
    }
}

/// The review session's appended system prompt.
pub fn compose_review_prompt(p_item: &PItem, diff: &str) -> String {
    let summary = p_item.output_summary.as_deref().unwrap_or("(worker 未留 output_summary)");
    let diff_block = if diff.trim().is_empty() {
        "(worktree 无改动 —— worker 没改任何文件)".to_string()
    } else {
        diff.to_string()
    };
    format!(
        r#"# 你是 Fleet 的「验收 review 会话」

你是只读把关者。判断这个 P-item 的 worker **是否真的达成了它的验收标准**——别被 worker 的自述忽悠，要拿真实的 diff 对照标准看。这是为了挡住「没做完就报完成」。

## P-item
- id: {id}
- 要做什么: {desc}

## 验收标准 (acceptance)
{acceptance}

## worker 的自述 (output_summary，不可全信)
{summary}

## worker 在 worktree 里的真实改动 (git diff)
```diff
{diff}
```

## 你的裁决
对照「验收标准」与「真实 diff」：
- 标准是否被 diff 里的改动真正满足？(声称做了但 diff 里没有 → achieved=false)
- 有声明 `builds` / `testsPass` 的，diff 是否包含使其成立的改动？
- 自述与 diff 不符、或 diff 为空却声称完成 → achieved=false。

只输出符合给定 JSON Schema 的结果：`{{"achieved": <bool>, "gaps": [<没达成的点, 字符串>]}}`。
achieved=true 时 gaps 为空数组；achieved=false 时 gaps 必须具体列出缺口。不要输出别的。
"#,
        id = p_item.id,
        desc = p_item.desc,
        acceptance = render_acceptance(&p_item.acceptance),
        summary = summary,
        diff = diff_block,
    )
}

fn render_acceptance(criteria: &[AcceptanceCriterion]) -> String {
    if criteria.is_empty() {
        return "  (worker/planner 未声明验收标准 —— 没有可对照的硬标准，请据 desc 与 diff 谨慎判断)".to_string();
    }
    criteria
        .iter()
        .map(|c| match c {
            AcceptanceCriterion::Builds => "  - builds: 代码必须能编译".to_string(),
            AcceptanceCriterion::TestsPass(cmd) => format!("  - testsPass: `{cmd}` 必须通过"),
            AcceptanceCriterion::HumanReview => "  - humanReview: 需人工复核".to_string(),
            AcceptanceCriterion::Custom(text) => format!("  - {text}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse the review session's stdout into a [`ReviewVerdict`].
///
/// `claude -p --output-format json --json-schema` returns an envelope whose
/// schema-conforming object lives in `structured_output` (the `result` field
/// holds the model's free-text reply, NOT the structured data — verified
/// against the real CLI). This is robust to three shapes:
/// 1. envelope with `structured_output: {achieved, gaps}` (the real path);
/// 2. envelope with `result` as a JSON string/object (older/alt shapes);
/// 3. raw `{achieved, gaps}` (no envelope).
pub fn parse_verdict(stdout: &str) -> Result<ReviewVerdict, String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err("review produced no output".to_string());
    }
    let outer: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("review output is not JSON: {e}"))?;

    let verdict = if let Some(so @ serde_json::Value::Object(_)) = outer.get("structured_output") {
        // The --json-schema structured field — the canonical location.
        so.clone()
    } else {
        match outer.get("result") {
            Some(serde_json::Value::String(s)) => serde_json::from_str::<serde_json::Value>(s)
                .map_err(|e| format!("review `result` string is not JSON: {e}"))?,
            Some(obj @ serde_json::Value::Object(_)) => obj.clone(),
            _ => outer,
        }
    };

    let achieved = verdict
        .get("achieved")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "review verdict missing boolean `achieved`".to_string())?;
    let gaps = verdict
        .get("gaps")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ReviewVerdict { achieved, gaps })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::PItemStatus;
    use std::path::PathBuf;

    fn pitem(acceptance: Vec<AcceptanceCriterion>, summary: Option<&str>) -> PItem {
        PItem {
            id: "p1".into(),
            desc: "实现登录接口".into(),
            touches: vec![PathBuf::from("src/auth.rs")],
            depends_on: vec![],
            acceptance,
            human_gate: false,
            status: PItemStatus::Reviewing,
            agent_session_id: Some("sid".into()),
            started_at: Some(1),
            completed_at: None,
            output_summary: summary.map(|s| s.into()),
            failure_gaps: Vec::new(),
        }
    }

    #[test]
    fn prompt_carries_acceptance_summary_and_diff() {
        let p = pitem(
            vec![AcceptanceCriterion::Builds, AcceptanceCriterion::TestsPass("cargo test auth".into())],
            Some("写好了登录接口并加了测试"),
        );
        let prompt = compose_review_prompt(&p, "+ fn login() {}\n");
        assert!(prompt.contains("实现登录接口"));
        assert!(prompt.contains("cargo test auth"));
        assert!(prompt.contains("写好了登录接口"));
        assert!(prompt.contains("fn login()"));
        assert!(prompt.contains("achieved"), "must ask for the structured verdict");
    }

    #[test]
    fn prompt_flags_empty_diff_and_missing_summary() {
        let p = pitem(vec![AcceptanceCriterion::Builds], None);
        let prompt = compose_review_prompt(&p, "   ");
        assert!(prompt.contains("worker 没改任何文件"));
        assert!(prompt.contains("未留 output_summary"));
    }

    #[test]
    fn parse_raw_achieved_true() {
        let v = parse_verdict(r#"{"achieved": true, "gaps": []}"#).unwrap();
        assert!(v.achieved);
        assert!(v.gaps.is_empty());
    }

    /// The "worker 谎报完成" case: review judges achieved=false with concrete gaps.
    #[test]
    fn parse_raw_rejected_with_gaps() {
        let v = parse_verdict(
            r#"{"achieved": false, "gaps": ["diff 里没有 login 函数", "测试未添加"]}"#,
        )
        .unwrap();
        assert!(!v.achieved);
        assert_eq!(v.gaps, vec!["diff 里没有 login 函数", "测试未添加"]);
    }

    /// The real `claude -p --output-format json --json-schema` shape: the
    /// verdict is under `structured_output`; `result` holds free text (which
    /// must NOT be parsed as the verdict). Verified against claude 2.1.x.
    #[test]
    fn parse_real_structured_output_envelope() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,
            "result":"Review complete.",
            "structured_output":{"achieved":false,"gaps":["hello.txt 未创建"]}}"#;
        let v = parse_verdict(stdout).unwrap();
        assert!(!v.achieved);
        assert_eq!(v.gaps, vec!["hello.txt 未创建"]);
    }

    #[test]
    fn parse_output_format_json_envelope_with_string_result() {
        // `claude --output-format json` wraps the structured output as a JSON
        // string under `result`.
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,
            "result":"{\"achieved\": false, \"gaps\": [\"空 diff\"]}"}"#;
        let v = parse_verdict(stdout).unwrap();
        assert!(!v.achieved);
        assert_eq!(v.gaps, vec!["空 diff"]);
    }

    #[test]
    fn parse_output_format_json_envelope_with_object_result() {
        let stdout = r#"{"type":"result","result":{"achieved": true, "gaps": []}}"#;
        let v = parse_verdict(stdout).unwrap();
        assert!(v.achieved);
    }

    #[test]
    fn parse_errors_on_garbage_and_missing_fields() {
        assert!(parse_verdict("not json").is_err());
        assert!(parse_verdict("").is_err());
        assert!(parse_verdict(r#"{"gaps": []}"#).is_err(), "missing achieved must error");
    }

    #[test]
    fn review_spawn_spec_wires_fields() {
        let p = pitem(vec![AcceptanceCriterion::Builds], Some("done"));
        let task = Task::drafting("t1".into(), "proj".into(), "demo".into(), 0);
        let spec = review_spawn_spec(&task, &p, "diff", PathBuf::from("/wt"));
        assert_eq!(spec.task_id, "t1");
        assert_eq!(spec.p_item_id, "p1");
        assert_eq!(spec.model, REVIEW_MODEL);
        assert_eq!(spec.cwd, PathBuf::from("/wt"));
        assert!(spec.system_prompt.contains("实现登录接口"));
    }
}
