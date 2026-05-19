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
pub const MASTER_SYSTEM_TEMPLATE: &str = include_str!("system_template.md");

/// Dev-only override. Reads the path from `FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE`
/// when set; falls back to the embedded constant.
///
/// **Production code should never set this env-var.** If you're tempted to,
/// you're about to bypass the security boundary that makes the master's red
/// lines tamper-resistant — find another way.
pub fn template_source() -> String {
    if let Some(path) = std::env::var_os("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE") {
        let p = PathBuf::from(path);
        if let Ok(content) = std::fs::read_to_string(&p) {
            return content;
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
        let mut task = sample_task();
        task.plan = DagPlan::default();
        let prompt = compose_system_prompt(&task);
        assert!(prompt.contains("no plan yet"));
    }

    #[test]
    fn env_override_swaps_template_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("override.md");
        std::fs::write(&path, "OVERRIDE TEMPLATE for task {{TASK_ID}}").unwrap();
        unsafe { std::env::set_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE", &path) };
        let result = template_source();
        unsafe { std::env::remove_var("FLEET_MASTER_SYSTEM_TEMPLATE_OVERRIDE") };
        assert!(result.starts_with("OVERRIDE TEMPLATE"));
    }
}
