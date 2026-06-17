//! Asset injection for the isolated worker / review sessions (方案 S).
//!
//! Safe-mode sessions (`--safe-mode`) keep the user's OAuth login but lose the
//! fleet CLAUDE.md injection, hooks, *and* the user's principles + project
//! memory. The deterministic orchestrator spawns workers/reviews in safe-mode
//! for isolation, then explicitly carries the two assets the lost context
//! actually mattered for back in via `--append-system-prompt`:
//!
//! 1. [`compose_engineering_principles`] — a fixed, embedded subset of the
//!    user's engineering-execution discipline. It is embedded as a constant
//!    (NOT read from the global `~/.claude/CLAUDE.md`) so the fleet-managed
//!    `@import` lines and the interaction / PRD-orchestration meta-rules — which
//!    are the *master*'s concern, never a worker's — can never leak into a
//!    worker prompt.
//! 2. [`relevant_memory`] — the project memory entries most relevant to the
//!    P-item, found by plain keyword / path matching (no LLM) against
//!    `~/.claude/projects/<key>/memory/*.md`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::real_home_dir;
use crate::pitem::PItem;
use crate::task::Task;

/// The engineering-execution subset of the user's principles, embedded
/// verbatim. Scope is deliberately narrow: rules a *worker* executing one
/// P-item in an isolated worktree must follow. It intentionally omits the
/// interaction mode (decision cards / AskUserQuestion), the PRD orchestration
/// discipline (commit cadence / worktree workflow — the orchestrator owns
/// those), commit-attribution rules, and the meta-rules about how to record
/// rules. Those belong to the master / interactive surface, not a worker.
pub const ENGINEERING_PRINCIPLES: &str = r#"# 工程执行纪律（隔离会话注入）

你在一个隔离的 git worktree 里执行一个明确的子任务。请严格遵守以下工程纪律：

1. 改 bug 必须测试先行：先写一个能复现该 bug 的失败测试，运行并确认它"真的进入测试体、跑到断言才失败"（编译错误/缺符号/夹具 panic 不算红灯），然后再改代码，最后重跑确认通过。绝不先写修复再补测试。
   - 例外（跳过测试但仍以其它方式验证）：纯构建/配置文件、无测试基建的纯 UI 改动、为这点改动新建测试基建不成比例。跳过时说明用了哪种验证（构建通过 / lint 通过）。

2. 写完代码、在声称完成之前，必须运行相应的构建/类型检查（cargo build / cargo test / tsc --noEmit / pnpm build 等）确认能编过，修掉所有错误再报告完成。

3. 绝不使用 git stash / git checkout -- / git reset 等任何会丢弃、移动或覆盖工作区未提交改动的命令。要对比干净状态请阅读报错并推理，不要污染工作区。

4. 工作区边界：只编辑分配给你的文件（P-item 的 touches 列表）。需要改 touches 之外的文件时停下来说明，不要绕过 hook。

5. 消费外部 API 的数值字段时绝不臆测单位/量纲（0-1 比例 vs 0-100 百分比等），先真正调用 API 看原始返回再写逻辑。

6. 不得谎称使用了实测值：凡声称"根据实际测量/实际时长/实际返回"，就必须真正执行测量并把结果用于后续逻辑，不得用硬编码估算冒充实测。

7. 在没有收集到证明问题确实发生的证据之前，不得以"如果/可能/也许"为由实施修复。先用真实证据定位根因，再动手。
"#;

/// Return the engineering-discipline subset to append to a worker / review
/// session's system prompt.
pub fn compose_engineering_principles() -> String {
    ENGINEERING_PRINCIPLES.to_string()
}

/// How many memory entries `relevant_memory` injects at most.
const MAX_MEMORY_ENTRIES: usize = 5;
/// Cap on the injected body length per entry (chars) so one fat memory file
/// can't blow up the prompt.
const MAX_ENTRY_CHARS: usize = 1_200;

/// `~/.claude/projects` (honours the `FLEET_HOME` test override via
/// [`real_home_dir`]).
fn claude_projects_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Encode a workspace path into the Claude Code project-directory key: every
/// `/`, `.`, and `_` becomes `-` (matching Claude's own scheme — see the
/// `decode_project_key` in `claw_fleet_core::memory` for the inverse).
fn encode_project_key(workspace: &Path) -> String {
    workspace
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' || c == '_' { '-' } else { c })
        .collect()
}

/// Lowercase alphanumeric tokens of length ≥ 3, de-duplicated, drawn from a
/// blob of text. Used to turn a P-item's desc / touches into match keywords.
fn keywords(blob: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in blob.chars() {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else {
            if cur.len() >= 3 && !out.contains(&cur) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    if cur.len() >= 3 && !out.contains(&cur) {
        out.push(cur);
    }
    out
}

/// Build the keyword set for a P-item: its description, its `touches` paths
/// (full + each component), and the task title / description for context.
fn pitem_keywords(task: &Task, p_item: &PItem) -> Vec<String> {
    let mut blob = String::new();
    blob.push_str(&p_item.desc);
    blob.push(' ');
    for t in &p_item.touches {
        blob.push_str(&t.to_string_lossy());
        blob.push(' ');
    }
    blob.push_str(&task.title);
    blob.push(' ');
    blob.push_str(&task.description);
    keywords(&blob)
}

/// Score a memory file's text against the keyword set: number of distinct
/// keywords that appear anywhere in the (lowercased) text.
fn score(text: &str, kws: &[String]) -> usize {
    let lower = text.to_lowercase();
    kws.iter().filter(|k| lower.contains(k.as_str())).count()
}

/// Retrieve the project-memory entries most relevant to this P-item, formatted
/// for `--append-system-prompt`. Plain keyword / path matching, no LLM. Returns
/// an empty string when the task has no resolvable workspace, the project has
/// no memory directory, or nothing matches.
pub fn relevant_memory(task: &Task, p_item: &PItem) -> String {
    let Some(workspace) = task.workspace.as_ref() else {
        return String::new();
    };
    let Some(projects) = claude_projects_dir() else {
        return String::new();
    };
    let memory_dir = projects.join(encode_project_key(workspace)).join("memory");
    let Ok(entries) = fs::read_dir(&memory_dir) else {
        return String::new();
    };

    let kws = pitem_keywords(task, p_item);
    if kws.is_empty() {
        return String::new();
    }

    // (score, name, body) for each scoring memory file.
    let mut scored: Vec<(usize, String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        // MEMORY.md is the index, not a fact — skip it.
        if name.eq_ignore_ascii_case("MEMORY.md") {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        // Score against filename + body so a well-named file matching the
        // touches path still surfaces even if its prose doesn't repeat the term.
        let s = score(&format!("{name}\n{body}"), &kws);
        if s > 0 {
            scored.push((s, name, body));
        }
    }

    if scored.is_empty() {
        return String::new();
    }

    // Highest score first; ties broken by name for determinism.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(MAX_MEMORY_ENTRIES);

    let mut out = String::from("# 相关项目记忆（按本子任务相关性检索）\n");
    for (_, name, body) in scored {
        let trimmed: String = if body.chars().count() > MAX_ENTRY_CHARS {
            body.chars().take(MAX_ENTRY_CHARS).collect::<String>() + "\n…(truncated)"
        } else {
            body
        };
        out.push_str(&format!("\n## {name}\n{}\n", trimmed.trim()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::fleet_home_lock;
    use crate::pitem::PItemStatus;
    use crate::task::{Task, TaskStatus};
    use std::path::PathBuf;

    /// Claims the process-wide FLEET_HOME mutex and points it at `tmp` for the
    /// guard's lifetime so memory lookups resolve under a temp `~/.claude`.
    struct HomeOverride {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl HomeOverride {
        fn new(tmp: &Path) -> Self {
            let lock = fleet_home_lock();
            let prev = std::env::var_os("FLEET_HOME");
            std::env::set_var("FLEET_HOME", tmp);
            HomeOverride { prev, _lock: lock }
        }
    }
    impl Drop for HomeOverride {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }

    fn task_with_workspace(ws: &str) -> Task {
        let mut t = Task::drafting("t1".into(), "proj".into(), "demo task".into(), 0);
        t.workspace = Some(PathBuf::from(ws));
        t.status = TaskStatus::Running;
        t
    }

    fn pitem(desc: &str, touches: &[&str]) -> PItem {
        PItem {
            id: "p1".into(),
            desc: desc.into(),
            touches: touches.iter().map(PathBuf::from).collect(),
            depends_on: vec![],
            acceptance: vec![],
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
            failure_gaps: Vec::new(),
        }
    }

    #[test]
    fn principles_cover_engineering_rules() {
        let p = compose_engineering_principles();
        // Load-bearing engineering rules must be present.
        assert!(p.contains("测试先行"), "must teach test-first");
        assert!(p.contains("git stash"), "must forbid destructive git");
        assert!(p.contains("touches"), "must state the workspace/touches boundary");
        assert!(p.contains("单位"), "must warn about external API units");
        assert!(p.contains("实测"), "must forbid faking measured values");
    }

    #[test]
    fn principles_exclude_interaction_and_orchestration_meta_rules() {
        let p = compose_engineering_principles();
        // Interaction-mode / orchestration / attribution meta-rules are the
        // master's concern and must never leak into a worker prompt.
        assert!(!p.contains("AskUserQuestion"), "no interaction-mode rules");
        assert!(!p.contains("fleet__ask"), "no decision-card rules");
        assert!(!p.contains("决策卡"), "no decision-card rules");
        assert!(!p.contains("Co-Authored-By"), "no commit-attribution rules");
        assert!(!p.contains("worktree --no-ff"), "no PRD merge-discipline rules");
        assert!(!p.contains("git merge --no-ff"), "no PRD merge-discipline rules");
    }

    #[test]
    fn relevant_memory_hits_by_touches_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());

        // A task whose workspace encodes to a project key, with a memory file
        // whose content names a token from the P-item's touches.
        let ws = "/Users/demo/work/myapp";
        let key = encode_project_key(Path::new(ws));
        let memory_dir = tmp.path().join(".claude").join("projects").join(&key).join("memory");
        fs::create_dir_all(&memory_dir).unwrap();
        fs::write(
            memory_dir.join("feedback_runner.md"),
            "---\nname: runner-quirk\ndescription: the runner module has a subtle lock ordering\n---\n\nThe runner.rs dispatch loop must hold the task write lock before mutating.\n",
        )
        .unwrap();
        // An unrelated memory file that should NOT be returned.
        fs::write(
            memory_dir.join("reference_unrelated.md"),
            "---\nname: payments\n---\n\nStripe webhook signature verification notes.\n",
        )
        .unwrap();

        let task = task_with_workspace(ws);
        let p = pitem("fix the dispatch loop", &["claw-fleet-task/src/runner.rs"]);
        let injected = relevant_memory(&task, &p);

        assert!(injected.contains("feedback_runner.md"), "expected runner memory: {injected}");
        assert!(injected.contains("lock ordering"), "body must be included: {injected}");
        assert!(!injected.contains("Stripe"), "unrelated memory must be excluded: {injected}");
    }

    #[test]
    fn relevant_memory_empty_when_no_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        let mut task = task_with_workspace("/x");
        task.workspace = None;
        let p = pitem("anything", &["a.rs"]);
        assert!(relevant_memory(&task, &p).is_empty());
    }

    #[test]
    fn relevant_memory_empty_when_no_memory_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        // Workspace resolves but there's no projects/<key>/memory directory.
        let task = task_with_workspace("/Users/demo/work/empty");
        let p = pitem("do work on runner", &["runner.rs"]);
        assert!(relevant_memory(&task, &p).is_empty());
    }

    #[test]
    fn encode_project_key_matches_claude_scheme() {
        assert_eq!(
            encode_project_key(Path::new("/Users/hoveychen/workspace/claude-fleet")),
            "-Users-hoveychen-workspace-claude-fleet"
        );
        // dots and underscores also collapse to dashes
        assert_eq!(encode_project_key(Path::new("/a/b.c_d")), "-a-b-c-d");
    }
}
