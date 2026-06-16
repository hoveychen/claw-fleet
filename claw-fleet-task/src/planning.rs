//! Planning session (P5) — the single interactive front of the pipeline.
//!
//! When a task has no plan yet, `fleet-task` spawns ONE interactive planner
//! session (the only non-isolated session — see `spawn_specs::PlannerSpawnSpec`
//! and `ProcessLauncher::launch_planner`). The planner clarifies the request
//! with the user via AskUserQuestion / fleet__ask, decomposes it into a lean
//! dependency graph, and writes it back with `fleet task update-plan`. The
//! deterministic orchestrator idles on the empty plan until that write lands,
//! then dispatches workers — no further user interaction.
//!
//! This module owns only the spec/prompt construction; the launch + the
//! "wait for the plan" coordination live in the `fleet-task` runtime.

use std::path::PathBuf;

use crate::spawn_specs::PlannerSpawnSpec;
use crate::task::{Material, Task};

/// Planning is the upfront reasoning step; give it a strong model.
pub const PLANNER_MODEL: &str = "claude-opus-4-8";

/// Build the planner spawn spec for `task`, running in `cwd` (the project
/// workspace).
pub fn planner_spawn_spec(task: &Task, cwd: PathBuf) -> PlannerSpawnSpec {
    PlannerSpawnSpec {
        task_id: task.id.clone(),
        cwd,
        system_prompt: compose_planner_system_prompt(task),
        // Per-task model override (composer selector) wins; otherwise the
        // strong default planning model.
        model: task
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| PLANNER_MODEL.to_string()),
    }
}

/// The planner's appended system prompt: its role, the task seed (title +
/// description + inbox materials), and the contract for clarifying with the
/// user and emitting the DAG.
fn compose_planner_system_prompt(task: &Task) -> String {
    let materials = render_materials(&task.inbox_materials);
    format!(
        r#"# 你是 Fleet 任务的「规划会话」

你是这个任务流水线里**唯一**会跟用户交互的环节。你的职责：跟用户对清需求，然后把任务拆解成一张精简的依赖图(DAG)写回去。拆完之后由确定性 orchestrator 自主执行，不再打扰用户。

## 任务种子
- task_id: {task_id}
- 标题: {title}
- 描述:
{description}
- 附带材料:
{materials}

## 你要做的
1. **对清需求**：描述里有歧义、缺约束、或有多种合理做法时，用 AskUserQuestion / fleet__ask 弹决策卡问用户。问到能动手为止——不要凭空假设。
2. **拆解成精简 DAG**：把任务拆成若干 P-item，每个是一个能被一个隔离 worker 独立完成的原子工作。**宁可粗，不要过度拆分**——这是单用户桌面场景，不是给一支团队排期。每个 P-item：
   - `id`：短 kebab-case
   - `desc`：做什么
   - `touches`：预计会改的文件路径(用于隔离与依赖推断)
   - `dependsOn`：依赖的 P-item id 列表(只连真实依赖)
   - `acceptance`：达成标准(`builds` / `{{ testsPass: "<cmd>" }}` / `{{ custom: "<自然语言>" }}`)，review 会拿它判断 worker 是否真做完
   - `humanGate`：true 表示该项即使 review 通过也要停下等用户确认(默认 false)
3. **写回计划**：把 DAG 组成 DagPlan 的 YAML（顶层 `items:` 是 id→P-item 的映射），通过
   `fleet task update-plan {task_id} --from-stdin`
   从 stdin 传入。该命令会校验依赖完整性与无环；报错就修正后重试。
4. 写成功后**结束本会话**（停止 agent）。orchestrator 会接手开始跑。

## 纪律
- 不要自己去实现任何 P-item——你只规划，不执行。
- 不要 `git commit` / 改分支。
- DAG 要可执行、依赖真实、acceptance 可判定。
"#,
        task_id = task.id,
        title = task.title,
        description = indent(&task.description),
        materials = materials,
    )
}

fn indent(s: &str) -> String {
    if s.trim().is_empty() {
        "  (空)".to_string()
    } else {
        s.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
    }
}

fn render_materials(materials: &[Material]) -> String {
    if materials.is_empty() {
        return "  (无)".to_string();
    }
    materials
        .iter()
        .map(|m| match m {
            Material::File { path, media, .. } => {
                format!("  - 文件 ({media:?}): {}", path.display())
            }
            Material::Text { content, .. } => {
                let preview: String = content.chars().take(160).collect();
                format!("  - 文本: {preview}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{MediaKind, Task};
    use std::path::PathBuf;

    fn task_with(desc: &str) -> Task {
        let mut t = Task::drafting("t-1".into(), "proj".into(), "添加书签 UI".into(), 0);
        t.description = desc.into();
        t
    }

    #[test]
    fn prompt_carries_seed_and_contract() {
        let mut task = task_with("实现书签列表、编辑、删除");
        task.inbox_materials.push(Material::Text {
            content: "用户希望支持拖拽排序".into(),
            added_at: 0,
        });
        let spec = planner_spawn_spec(&task, PathBuf::from("/ws"));
        let p = &spec.system_prompt;
        // seed
        assert!(p.contains("t-1"), "task id");
        assert!(p.contains("添加书签 UI"), "title");
        assert!(p.contains("实现书签列表"), "description seed");
        assert!(p.contains("拖拽排序"), "material seed");
        // contract
        assert!(p.contains("AskUserQuestion"), "must instruct interactive clarify");
        assert!(p.contains("fleet task update-plan t-1 --from-stdin"), "must give the real write command");
        assert!(p.contains("dependsOn") && p.contains("acceptance") && p.contains("touches"));
        // discipline: planner must not execute
        assert!(p.contains("只规划，不执行"));
        assert_eq!(spec.model, PLANNER_MODEL);
        assert_eq!(spec.cwd, PathBuf::from("/ws"));
        assert_eq!(spec.task_id, "t-1");
    }

    #[test]
    fn empty_description_and_no_materials_render_placeholders() {
        let task = task_with("");
        let spec = planner_spawn_spec(&task, PathBuf::from("/ws"));
        assert!(spec.system_prompt.contains("(空)"), "empty desc placeholder");
        assert!(spec.system_prompt.contains("(无)"), "no-materials placeholder");
    }

    #[test]
    fn per_task_model_override_wins_over_default() {
        let mut task = task_with("x");
        // No override → default planner model.
        assert_eq!(planner_spawn_spec(&task, PathBuf::from("/ws")).model, PLANNER_MODEL);
        // Override set → used verbatim.
        task.model = Some("claude-sonnet-4-6".into());
        assert_eq!(planner_spawn_spec(&task, PathBuf::from("/ws")).model, "claude-sonnet-4-6");
        // Blank override → ignored, falls back to default.
        task.model = Some("   ".into());
        assert_eq!(planner_spawn_spec(&task, PathBuf::from("/ws")).model, PLANNER_MODEL);
    }

    #[test]
    fn file_material_is_summarised() {
        let mut task = task_with("x");
        task.inbox_materials.push(Material::File {
            path: PathBuf::from("/tmp/shot.png"),
            media: MediaKind::Screenshot,
            added_at: 0,
        });
        let spec = planner_spawn_spec(&task, PathBuf::from("/ws"));
        assert!(spec.system_prompt.contains("shot.png"));
        assert!(spec.system_prompt.contains("Screenshot"));
    }
}
