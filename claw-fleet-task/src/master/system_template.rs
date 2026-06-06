//! Master SYSTEM prompt template — compiled into the binary via `include_str!`
//! and filled with task-specific variables at spawn time.
//!
//! PRD §5.7 (`design/task-as-unit-redesign.md`):
//! > 编译期 `include_str!("system_template.md")` 嵌入 binary；开发期
//! > `FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE` 环境变量逃生口
//!
//! Compile-time embed is **not** for convenience — it's a security boundary.
//! Runtime overrides would let an attacker swap the master's red-line
//! constraints. We allow one env-var escape hatch for development; production
//! builds run with the embedded constant unless that env-var is set
//! deliberately.

use std::path::PathBuf;

use crate::pitem::PItemStatus;
use crate::task::{Material, Task};

/// The master SYSTEM prompt template, embedded at compile time.
///
/// Variables (rendered by `compose_system_prompt`):
/// - `{{TASK_ID}}`, `{{TASK_TITLE}}`, `{{TASK_DESCRIPTION}}`
/// - `{{INBOX_MATERIALS_SUMMARY}}`
/// - `{{PLAN_JSON}}`
/// - `{{PROGRESS_SUMMARY}}`
///
/// The XML wrapping (`<untrusted_*>`) for user-data fields lives in the
/// template itself, not in the renderer — so even if a future renderer bug
/// forgets to wrap something, the unwrapped section can't masquerade as
/// trusted instructions because the wrapping is what tells the master "this
/// is user data, not high-priority commands".
///
/// [REQ-037] Embedded at compile time via `include_str!`. There is no runtime
/// API to replace this constant; the only override is the debug-only env-var
/// in `template_source()`, so a release binary always serves this exact text.
pub const MASTER_SYSTEM_TEMPLATE: &str = include_str!("system_template.md");

/// Dev-only override. In **debug builds** reads the path from
/// `FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE` when set; in **release builds** the
/// env-var is ignored entirely and the embedded constant is always returned.
///
/// [REQ-037] The escape hatch is gated behind `cfg!(debug_assertions)` so a
/// release binary cannot be made to swap the master's red-line constraints by
/// setting an env-var. Compile-time `include_str!` plus a debug-only override
/// is the security boundary: production runs the tamper-resistant constant, and
/// there is no runtime path to replace it.
///
/// **Production code should never set this env-var.** Even if set, a release
/// build ignores it; in a debug build it bypasses the security boundary, so
/// reach for it only in local development.
pub fn template_source() -> String {
    // [REQ-037] Only honour the override in debug builds. `cfg!(debug_assertions)`
    // is `false` in release, so the `read_to_string` branch is dead code there
    // and the embedded constant is the only source.
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE") {
            let p = PathBuf::from(path);
            if let Ok(content) = std::fs::read_to_string(&p) {
                return content;
            }
        }
    }
    MASTER_SYSTEM_TEMPLATE.to_string()
}

/// Compose the master's SYSTEM prompt for a specific task. Pure function —
/// no IO beyond serializing the plan; same task always yields same output.
pub fn compose_system_prompt(task: &Task) -> String {
    let template = template_source();
    let plan_json = serde_json::to_string_pretty(&task.plan).unwrap_or_else(|_| "{}".into());
    let progress = render_progress_summary(task);
    let materials = render_materials_summary(&task.inbox_materials);

    template
        .replace("{{TASK_ID}}", &task.id)
        .replace("{{TASK_TITLE}}", &task.title)
        .replace(
            "{{TASK_DESCRIPTION}}",
            if task.description.is_empty() {
                "(none)"
            } else {
                &task.description
            },
        )
        .replace("{{INBOX_MATERIALS_SUMMARY}}", &materials)
        .replace("{{PLAN_JSON}}", &plan_json)
        .replace("{{PROGRESS_SUMMARY}}", &progress)
}

fn render_progress_summary(task: &Task) -> String {
    if task.plan.is_empty() {
        return "(no plan yet — call `fleet task get-plan` after planner runs)".into();
    }
    let total = task.plan.items.len();
    let mut counts = [0usize; 7]; // WaitDeps, WaitResource, Running, WaitHumanGate, Done, Failed, Skipped
    for p in task.plan.items.values() {
        match p.status {
            PItemStatus::WaitDeps => counts[0] += 1,
            PItemStatus::WaitResource => counts[1] += 1,
            PItemStatus::Running => counts[2] += 1,
            PItemStatus::WaitHumanGate => counts[3] += 1,
            PItemStatus::Done => counts[4] += 1,
            PItemStatus::Failed(_) => counts[5] += 1,
            PItemStatus::Skipped => counts[6] += 1,
        }
    }
    format!(
        "total={total}  done={d}  failed={f}  skipped={s}  running={r}  waitGate={g}  waitDeps={wd}  waitResource={wr}",
        d = counts[4],
        f = counts[5],
        s = counts[6],
        r = counts[2],
        g = counts[3],
        wd = counts[0],
        wr = counts[1]
    )
}

fn render_materials_summary(materials: &[Material]) -> String {
    if materials.is_empty() {
        return "(no materials attached)".into();
    }
    let mut lines = Vec::with_capacity(materials.len());
    for m in materials {
        match m {
            Material::File { path, media, .. } => {
                lines.push(format!("- file ({media:?}): {}", path.display()));
            }
            Material::Text { content, .. } => {
                let preview: String = content.chars().take(120).collect();
                let suffix = if content.chars().count() > 120 { " …" } else { "" };
                lines.push(format!("- text: {preview}{suffix}"));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{PItem, PItemStatus};
    use crate::plan::DagPlan;
    use crate::task::{MediaKind, TaskStatus};
    use std::path::PathBuf;

    fn sample_task() -> Task {
        let plan = DagPlan::from_items(vec![PItem {
            id: "p1".into(),
            desc: "first".into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }]);
        Task {
            id: "t-demo".into(),
            project_id: "proj".into(),
            title: "Add bookmarks UI".into(),
            description: "Implement the Bookmarks list / edit / delete flows.".into(),
            inbox_materials: vec![
                Material::File {
                    path: PathBuf::from("/tmp/screenshot.png"),
                    media: MediaKind::Screenshot,
                    added_at: 0,
                },
                Material::Text {
                    content: "user requested feature".into(),
                    added_at: 0,
                },
            ],
            plan,
            status: TaskStatus::Running,
            created_at: 1_700_000_000,
            started_at: Some(1_700_000_100),
            completed_at: None,
            task_branch: Some("fleet/add-bookmarks-ui".into()),
            workspace: None,
            master_session_id: None,
            title_auto: false,
        }
    }

    #[test]
    fn template_has_required_sections() {
        // The embedded template must include the load-bearing sections per
        // PRD §5.7. If anyone strips one of these, this test catches it
        // before it reaches a real run.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("Acceptance Audit Protocol"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("不信模型"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("untrusted_task_title"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("untrusted_task_description"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("untrusted_inbox_materials"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("mark-done"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("mark-failed"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("update-plan"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("[event]"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("[user]"));
    }

    #[test]
    fn template_does_not_expose_pause_resume_clear_as_master_tools() {
        // These are user-only per PRD §5.7. The template must not list them
        // in the master's tool section.
        let tools_section = MASTER_SYSTEM_TEMPLATE
            .split("你的工具集")
            .nth(1)
            .unwrap_or("")
            .split("═══")
            .next()
            .unwrap_or("");
        assert!(
            !tools_section.contains("fleet task pause"),
            "master must NOT have `fleet task pause` in its tool list"
        );
        assert!(
            !tools_section.contains("fleet task resume"),
            "master must NOT have `fleet task resume` in its tool list"
        );
        assert!(
            !tools_section.contains("fleet task clear"),
            "master must NOT have `fleet task clear` in its tool list"
        );
    }

    #[test]
    fn compose_substitutes_task_vars() {
        // Serialise against `env_override_swaps_template_when_set`, which mutates
        // the process-global `FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE` env var read
        // by `template_source()`. Without this lock a concurrent override leaks a
        // stub template lacking `{{PLAN_JSON}}` into our `compose_system_prompt`.
        let _g = crate::paths::fleet_home_lock();
        let task = sample_task();
        let prompt = compose_system_prompt(&task);
        assert!(prompt.contains("t-demo"));
        assert!(prompt.contains("Add bookmarks UI"));
        assert!(prompt.contains("Implement the Bookmarks"));
        assert!(prompt.contains("/tmp/screenshot.png"));
        assert!(prompt.contains("user requested feature"));
        // Plan JSON included
        assert!(prompt.contains("\"p1\""));
        // Progress summary computed
        assert!(prompt.contains("total=1"));
        assert!(prompt.contains("waitDeps=1"));
        // No leftover placeholders
        assert!(!prompt.contains("{{TASK_ID}}"));
        assert!(!prompt.contains("{{TASK_TITLE}}"));
        assert!(!prompt.contains("{{PLAN_JSON}}"));
    }

    #[test]
    fn compose_keeps_untrusted_wrapping_around_user_data() {
        // The XML wrapping is a security boundary — must survive substitution.
        let _g = crate::paths::fleet_home_lock();
        let task = sample_task();
        let prompt = compose_system_prompt(&task);
        assert!(prompt.contains("<untrusted_task_title>"));
        assert!(prompt.contains("</untrusted_task_title>"));
        assert!(prompt.contains("<untrusted_task_description>"));
        assert!(prompt.contains("<untrusted_inbox_materials>"));
        // Substituted content is inside the wrapper, not bare.
        let title_block = prompt
            .split("<untrusted_task_title>")
            .nth(1)
            .unwrap()
            .split("</untrusted_task_title>")
            .next()
            .unwrap();
        assert!(title_block.contains("Add bookmarks UI"));
    }

    #[test]
    fn compose_handles_missing_description_and_materials() {
        let _g = crate::paths::fleet_home_lock();
        let mut task = sample_task();
        task.description = String::new();
        task.inbox_materials = Vec::new();
        let prompt = compose_system_prompt(&task);
        assert!(prompt.contains("(none)"), "empty description should render as `(none)`");
        assert!(
            prompt.contains("(no materials attached)"),
            "empty materials should render the fallback line"
        );
    }

    #[test]
    fn compose_handles_empty_plan() {
        let _g = crate::paths::fleet_home_lock();
        let mut task = sample_task();
        task.plan = DagPlan::default();
        let prompt = compose_system_prompt(&task);
        assert!(prompt.contains("no plan yet"));
    }

    // [REQ-037] In debug builds the env-var escape hatch is honoured.
    #[cfg(debug_assertions)]
    #[test]
    fn env_override_swaps_template_when_set() {
        // Hold the global test lock across the whole set→read→remove window so
        // the override env var never leaks into a concurrent reader's
        // `compose_system_prompt` (which would drop `{{PLAN_JSON}}` and fail
        // assertions in the runtime/compose tests).
        let _g = crate::paths::fleet_home_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("override.md");
        std::fs::write(&path, "OVERRIDE TEMPLATE for task {{TASK_ID}}").unwrap();
        unsafe { std::env::set_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE", &path) };
        let result = template_source();
        unsafe { std::env::remove_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE") };
        assert!(result.starts_with("OVERRIDE TEMPLATE"));
    }

    // [REQ-037] In release builds the env-var must be IGNORED — the embedded
    // constant is the only source, so the security boundary can't be bypassed
    // at runtime. This test only compiles into release builds; debug runs skip
    // it (the debug counterpart above covers the override path).
    #[cfg(not(debug_assertions))]
    #[test]
    fn env_override_ignored_in_release_build() {
        let _g = crate::paths::fleet_home_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("override.md");
        std::fs::write(&path, "MALICIOUS OVERRIDE that strips red lines").unwrap();
        unsafe { std::env::set_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE", &path) };
        let result = template_source();
        unsafe { std::env::remove_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE") };
        // Release build must serve the embedded constant verbatim, ignoring env.
        assert_eq!(result, MASTER_SYSTEM_TEMPLATE);
        assert!(!result.contains("MALICIOUS OVERRIDE"));
    }

    // [REQ-037] Regardless of build profile, with NO env-var set the source is
    // exactly the embedded constant. This proves the `cfg!` gate doesn't alter
    // the no-override default.
    #[test]
    fn template_source_is_embedded_constant_without_override() {
        let _g = crate::paths::fleet_home_lock();
        // Ensure no stray override leaks from another test.
        unsafe { std::env::remove_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE") };
        assert_eq!(template_source(), MASTER_SYSTEM_TEMPLATE);
    }

    // [REQ-037] The template is embedded via `include_str!`, so the constant is
    // non-empty at compile time and carries the load-bearing red-line section.
    #[test]
    fn template_is_compile_time_embedded_and_nonempty() {
        assert!(!MASTER_SYSTEM_TEMPLATE.is_empty());
        assert!(MASTER_SYSTEM_TEMPLATE.contains("红线"));
    }

    // ───────── [REQ-010] 10 red lines, one assertion per line ─────────
    // Each red line must be mentioned in the embedded template. If anyone
    // strips one, the matching test fails before it reaches a real run. The
    // sanction for each red line (Auditor critical report / acceptance reject)
    // is documented in the template's red-line section.

    #[test]
    fn redline_01_no_direct_code_edit() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无直接代码编辑"));
    }

    #[test]
    fn redline_02_no_skip_audit() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无跳过审计"));
    }

    #[test]
    fn redline_03_no_pause_resume_clear() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无 pause/resume/clear"));
    }

    #[test]
    fn redline_04_no_proxy_signals() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无代理信号"));
    }

    #[test]
    fn redline_05_no_direct_git() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无直接 git"));
    }

    #[test]
    fn redline_06_no_worker_session_rw() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无 worker 会话读写"));
    }

    #[test]
    fn redline_07_no_fleet_config_edit() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无改 fleet 配置"));
    }

    #[test]
    fn redline_08_no_new_task() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无创建新 task"));
    }

    #[test]
    fn redline_09_no_worker_file_edit() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("无改 worker 文件"));
    }

    #[test]
    fn redline_10_merge_only_via_pmerge() {
        // [REQ-010]
        assert!(MASTER_SYSTEM_TEMPLATE.contains("merge 仅通过 P-merge"));
    }

    #[test]
    fn redline_section_declares_exactly_ten() {
        // [REQ-010] The template must claim "共 10 条" and the numbered list
        // must run 1.→10. so the count is unambiguous and auditable.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("红线（共 10 条"));
        // Extract the red-line section and count the numbered items 1.–10.
        let section = MASTER_SYSTEM_TEMPLATE
            .split("红线（共 10 条")
            .nth(1)
            .expect("red-line section present")
            .split("你的工具集")
            .next()
            .unwrap_or("");
        for n in 1..=10 {
            let marker = format!("{n}. **");
            assert!(
                section.contains(&marker),
                "red-line section is missing numbered item `{marker}`"
            );
        }
        // There must be no 11th numbered item in the section.
        assert!(
            !section.contains("11. **"),
            "red-line section must declare exactly 10 items, found an 11th"
        );
    }

    // ───────── [REQ-003] 4 forbidden proxy signals ─────────
    #[test]
    fn audit_protocol_forbids_four_proxy_signals() {
        // [REQ-003] The four proxy signals (worker self-report, token spend,
        // diff size, elapsed time) must each be named as forbidden evidence.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("worker 自报说完了"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("token / 时间消耗大"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("diff 看着量足够"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("耗时长"));
        // The forbid step must be explicitly framed as proxy-signal rejection.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("不能用代理信号充证据"));
    }

    // ───────── [REQ-004] 4-step audit protocol ─────────
    #[test]
    fn audit_protocol_has_four_named_steps() {
        // [REQ-004] unpack → find evidence → forbid proxies → escalate.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("Acceptance Audit Protocol"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("**unpack**"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("**find evidence**"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("**forbid proxies**"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("**escalate**"));
    }

    #[test]
    fn audit_protocol_maps_each_acceptance_kind_to_evidence() {
        // [REQ-004] Each acceptance variant must map to a concrete evidence
        // gathering step in the find-evidence stage.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("`builds`"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("cargo check --package"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("`testsPass(cmd)`"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("`humanReview`"));
        assert!(MASTER_SYSTEM_TEMPLATE.contains("`custom(rule)`"));
    }

    #[test]
    fn audit_protocol_escalate_retries_worker_before_user() {
        // [REQ-004][DEC-007] When uncertain, retry the worker once before
        // escalating to the user via AskUserQuestion.
        let section = MASTER_SYSTEM_TEMPLATE
            .split("**escalate**")
            .nth(1)
            .expect("escalate step present")
            .split("═══")
            .next()
            .unwrap_or("");
        assert!(section.contains("retry worker"));
        assert!(section.contains("AskUserQuestion"));
    }

    // ───────── [REQ-021] 7 allowed tools, git/edit/write/pause/resume/clear forbidden ─────────
    /// Body of the tool section: text between the tool-section header line
    /// (which itself ends in `═══`) and the next section header `═══ 事件协议`.
    fn tool_section_body() -> &'static str {
        MASTER_SYSTEM_TEMPLATE
            .split("仅这 8 个能动 task） ═══")
            .nth(1)
            .expect("tool section header present")
            .split("═══ 事件协议")
            .next()
            .expect("event-protocol header present after tools")
    }

    #[test]
    fn tool_section_lists_eight_allowed_tools() {
        // [REQ-021][WA3] Exactly the 8 task tools (the adversarial `audit` tool
        // was added in WA3 so the master can run the independent quorum audit
        // before mark-done), no more.
        let section = tool_section_body();
        for tool in [
            "get-plan",
            "get-dispatchable",
            "dispatch",
            "read-output",
            "audit",
            "mark-done",
            "mark-failed",
            "update-plan",
        ] {
            assert!(
                section.contains(tool),
                "tool section missing allowed tool `{tool}`"
            );
        }
        // Numbered 1.–8. with no 9th item.
        for n in 1..=8 {
            assert!(
                section.contains(&format!("{n}. `fleet task")),
                "tool section missing numbered tool {n}"
            );
        }
        assert!(
            !section.contains("9. `fleet task"),
            "tool section must list exactly 8 tools, found a 9th"
        );
        // The header declares the count of 8.
        assert!(MASTER_SYSTEM_TEMPLATE.contains("共 8 个专用工具"));
    }

    // ───────── [WA3] mandatory adversarial-audit step before mark-done ─────────

    #[test]
    fn audit_protocol_has_mandatory_audit_step_before_mark_done() {
        // [WA3] The Acceptance Audit Protocol must carry an explicit step that
        // requires `fleet task audit` before mark-done — skipping it is framed
        // as a red-line-2 violation.
        assert!(
            MASTER_SYSTEM_TEMPLATE.contains("**adversarial audit**"),
            "protocol must name the adversarial-audit step"
        );
        assert!(
            MASTER_SYSTEM_TEMPLATE.contains("fleet task audit"),
            "protocol must instruct calling `fleet task audit`"
        );
        // The audit step must precede / gate mark-done explicitly.
        assert!(
            MASTER_SYSTEM_TEMPLATE.contains("之前**必须**先跑独立对抗审计"),
            "protocol must make audit mandatory before mark-done"
        );
    }

    #[test]
    fn audit_protocol_critical_confirmed_blocks_mark_done() {
        // [WA3][DEC-007] A CRITICAL_CONFIRMED verdict must forbid mark-done and
        // route to retry-worker-once then AskUserQuestion (never self-release).
        // Pull the audit step body and assert the blocking + escalation wording.
        let step = MASTER_SYSTEM_TEMPLATE
            .split("**adversarial audit**")
            .nth(1)
            .expect("audit step present")
            .split("═══ Human Gate")
            .next()
            .unwrap_or("");
        assert!(step.contains("CRITICAL_CONFIRMED"));
        assert!(step.contains("CLEAN"));
        assert!(
            step.contains("绝对不许 mark-done"),
            "a confirmed critical must hard-block mark-done"
        );
        assert!(step.contains("DEC-007"));
        assert!(step.contains("retry worker") || step.contains("retry"));
        assert!(step.contains("AskUserQuestion"));
    }

    #[test]
    fn tool_section_explicitly_forbids_git_edit_write_and_user_only_tools() {
        // [REQ-021] git / Edit / Write / pause / resume / clear must be named
        // as forbidden, not merely absent.
        let section = tool_section_body();
        assert!(section.contains("禁止使用"));
        assert!(section.contains("`git`"));
        assert!(section.contains("`Edit`"));
        assert!(section.contains("`Write`"));
        assert!(section.contains("pause"));
        assert!(section.contains("resume"));
        assert!(section.contains("clear"));
    }
}
