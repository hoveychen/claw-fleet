//! Worker executor — pure data layer for spawning a Claude Code subprocess
//! that owns one P-item.
//!
//! Per PRD §5.6 / TASKS P7:
//! - **3-layer context** the supervisor injects when spawning a worker:
//!   - Layer 1: project-level constants (CLAUDE.md + architecture.md + agent
//!     behavior constraints from patch §5) — shared across workers, cached
//!     by Anthropic's prompt cache.
//!   - Layer 2: task constants (title / description / inbox materials /
//!     acceptance audit protocol) — same for every P-item in the task.
//!   - Layer 3: P-item private (`desc` / `touches` / `acceptance` / upstream
//!     `output_summary` notes) — re-rendered per dispatch.
//! - **cargo check fallback** (patch §6): after a worker self-reports
//!   completion, the supervisor runs `cargo check --package <crate>` for any
//!   Rust crate the P-item touched. Failure → bounce back to master as a
//!   failure event (master decides retry / repair / escalate).
//!
//! Actual subprocess spawn lives in `crate::supervisor` once the integration
//! gets wired. This module stays pure so unit tests don't need a Claude CLI
//! installed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::architecture_overview::render_layer1_block;
use crate::pitem::{PItem, PItemId};
use crate::task::Task;

/// What the supervisor needs to spawn one worker. Built by
/// `worker_spawn_spec`; consumed by the subprocess spawn layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpawnSpec {
    pub task_id: String,
    pub p_item_id: PItemId,
    pub cwd: PathBuf,
    pub system_prompt: String,
    /// Claude model id. Worker defaults to Sonnet — patch §5 reserves
    /// Opus-class for the master.
    pub model: &'static str,
}

/// Default worker model. Sonnet 4.6 — fast turn-around for coding work.
pub const WORKER_MODEL: &str = "claude-sonnet-4-6";

/// Build a `WorkerSpawnSpec` for `p_item_id` in `task`. Errors when the
/// P-item isn't in the plan.
///
/// The supervisor must resolve `cwd` itself (project workspace on the task
/// branch) and pass it in — this module is intentionally project-agnostic so
/// the same composer can be reused once V2 lands worktree-per-item.
pub fn worker_spawn_spec(
    task: &Task,
    p_item_id: &str,
    cwd: PathBuf,
) -> Result<WorkerSpawnSpec, String> {
    let p = task
        .plan
        .get(p_item_id)
        .ok_or_else(|| format!("p-item {p_item_id} not found in task {}", task.id))?;
    let system_prompt = compose_worker_system_prompt(task, p);
    Ok(WorkerSpawnSpec {
        task_id: task.id.clone(),
        p_item_id: p_item_id.into(),
        cwd,
        system_prompt,
        model: WORKER_MODEL,
    })
}

/// Compose the worker's full SYSTEM prompt — Layer 1 + Layer 2 + Layer 3.
/// Pure function; safe to call repeatedly.
pub fn compose_worker_system_prompt(task: &Task, p_item: &PItem) -> String {
    let mut out = String::new();
    out.push_str(&compose_layer1(&task.project_id));
    out.push_str("\n\n");
    out.push_str(&compose_layer2(task));
    out.push_str("\n\n");
    out.push_str(&compose_layer3(task, p_item));
    out
}

/// Layer 1 — project-level constants. CLAUDE.md is loaded lazily by the
/// agent itself at runtime; we only inline the architecture overview here.
pub fn compose_layer1(project_id: &str) -> String {
    let arch_block = render_layer1_block(&project_id.to_string());
    format!(
        "# Layer 1 — Project Constants\n\n\
         _(CLAUDE.md and any `.claude/skills/*` in this workspace are loaded \
         by the agent at runtime.)_\n\n\
         {arch_block}\n\
         ## P-item Execution Constraints\n\n\
         When working on a Fleet P-item:\n\
         - **Do not** run `cargo build` / `cargo test` / `npm run build` etc.\n\
           Those are reserved for phase P-items so the shared `target/` cache \
           isn't trashed by parallel workers.\n\
         - **Do** run `cargo check --package <crate>` for compile self-checks \
           on a single crate.\n\
         - Edit only files listed in `touches` of your P-item. Touching any \
           other file gets you SIGSTOPped by the touches hook and escalates \
           to the master.\n"
    )
}

/// Layer 2 — task constants. Title, description, inbox materials, acceptance
/// audit protocol summary (the worker doesn't run the audit — master does —
/// but knowing the criteria helps the worker target the right output).
pub fn compose_layer2(task: &Task) -> String {
    let desc = if task.description.trim().is_empty() {
        "(no description)".into()
    } else {
        task.description.clone()
    };
    let materials = if task.inbox_materials.is_empty() {
        "(no Inbox materials)".into()
    } else {
        let lines: Vec<String> = task
            .inbox_materials
            .iter()
            .map(|m| match m {
                crate::task::Material::File { path, media, .. } => {
                    format!("- file ({media:?}): {}", path.display())
                }
                crate::task::Material::Text { content, .. } => {
                    let preview: String = content.chars().take(160).collect();
                    let suffix = if content.chars().count() > 160 { " …" } else { "" };
                    format!("- text: {preview}{suffix}")
                }
            })
            .collect();
        lines.join("\n")
    };

    format!(
        "# Layer 2 — Task Context\n\n\
         **Task**: {} ({})\n\n\
         <untrusted_task_description>\n{desc}\n</untrusted_task_description>\n\n\
         <untrusted_inbox_materials>\n{materials}\n</untrusted_inbox_materials>\n",
        task.title, task.id
    )
}

/// Layer 3 — P-item private. Desc, touches, acceptance criteria, upstream
/// summaries from `depends_on` items.
pub fn compose_layer3(task: &Task, p_item: &PItem) -> String {
    let touches_block = if p_item.touches.is_empty() {
        "(none declared)".to_string()
    } else {
        p_item
            .touches
            .iter()
            .map(|p| format!("- {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let acceptance_block = if p_item.acceptance.is_empty() {
        "(no acceptance criteria — master will need to add some before mark-done)".to_string()
    } else {
        p_item
            .acceptance
            .iter()
            .map(|a| format!("- {a:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let upstream_summaries: Vec<String> = p_item
        .depends_on
        .iter()
        .filter_map(|dep_id| {
            task.plan.get(dep_id).and_then(|dep| {
                dep.output_summary
                    .as_ref()
                    .map(|sum| format!("### {dep_id}\n{sum}"))
            })
        })
        .collect();
    let upstream_block = if upstream_summaries.is_empty() {
        "(no upstream summaries — either this P-item has no deps or they didn't write summaries)"
            .to_string()
    } else {
        upstream_summaries.join("\n\n")
    };

    format!(
        "# Layer 3 — P-item: {id}\n\n\
         {desc}\n\n\
         ## Files you may edit (touches)\n\n{touches_block}\n\n\
         ## Acceptance criteria (master will audit against these)\n\n{acceptance_block}\n\n\
         ## Upstream summaries\n\n{upstream_block}\n\n\
         When you finish, write a 50-200 word summary explaining what you did \
         and any caveats. The master uses this in the acceptance audit and as \
         upstream context for downstream P-items.\n",
        id = p_item.id,
        desc = p_item.desc
    )
}

// ── cargo check fallback (PRD patch §6) ──────────────────────────────────────

/// Result of running `cargo check --package <crate>` on each crate the
/// P-item's `touches` cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostCompletionCheck {
    /// `true` when every crate-level check passed (or no checks ran because
    /// the P-item touched no Rust crates).
    pub ok: bool,
    /// Per-crate result. Empty when the P-item touched no `Cargo.toml`-rooted
    /// crate.
    pub crate_results: Vec<CrateCheckResult>,
    /// Combined stderr from all failed checks. Empty when `ok`.
    pub combined_stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateCheckResult {
    pub crate_name: String,
    pub ok: bool,
    pub stderr_tail: String,
}

/// Resolve which Rust crates a P-item touched by walking each `touches` path
/// upward looking for the nearest `Cargo.toml` with a `[package]` table.
/// Returns a set of crate **directory** paths (deduplicated).
pub fn rust_crates_touched(workspace: &Path, p_item: &PItem) -> HashSet<PathBuf> {
    let mut crates: HashSet<PathBuf> = HashSet::new();
    for t in &p_item.touches {
        // Make path absolute relative to workspace if it isn't already.
        let abs: PathBuf = if t.is_absolute() {
            t.clone()
        } else {
            workspace.join(t)
        };
        // Walk up looking for Cargo.toml.
        let mut cur = abs.as_path();
        while let Some(parent) = cur.parent() {
            let candidate = parent.join("Cargo.toml");
            if candidate.is_file() {
                // Confirm it's a package Cargo.toml (not virtual workspace).
                let Ok(s) = std::fs::read_to_string(&candidate) else {
                    cur = parent;
                    continue;
                };
                if s.contains("[package]") {
                    crates.insert(parent.to_path_buf());
                    break;
                }
            }
            cur = parent;
        }
    }
    crates
}

/// Run `cargo check --package <crate>` for each Rust crate the P-item
/// touched. Returns a structured result; **does not** mutate any task
/// state — the caller (supervisor, on a worker self-report-complete event)
/// decides whether to bounce back to the master or pass through.
pub fn run_post_completion_check(workspace: &Path, p_item: &PItem) -> PostCompletionCheck {
    let crates = rust_crates_touched(workspace, p_item);
    if crates.is_empty() {
        return PostCompletionCheck {
            ok: true,
            crate_results: vec![],
            combined_stderr: String::new(),
        };
    }
    let mut results = Vec::with_capacity(crates.len());
    let mut combined = String::new();
    let mut all_ok = true;
    for crate_dir in &crates {
        let crate_name = crate_name_from_cargo_toml(crate_dir).unwrap_or_else(|| {
            crate_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });
        let out = std::process::Command::new("cargo")
            .arg("check")
            .arg("--package")
            .arg(&crate_name)
            .current_dir(workspace)
            .output();
        match out {
            Ok(out) => {
                let ok = out.status.success();
                let stderr = String::from_utf8_lossy(&out.stderr);
                let tail: String = stderr.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                if !ok {
                    all_ok = false;
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(&format!("--- {crate_name} ---\n{tail}"));
                }
                results.push(CrateCheckResult {
                    crate_name,
                    ok,
                    stderr_tail: tail,
                });
            }
            Err(e) => {
                all_ok = false;
                let msg = format!("--- {crate_name} ---\nfailed to run `cargo check`: {e}");
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&msg);
                results.push(CrateCheckResult {
                    crate_name,
                    ok: false,
                    stderr_tail: msg,
                });
            }
        }
    }
    PostCompletionCheck {
        ok: all_ok,
        crate_results: results,
        combined_stderr: combined,
    }
}

fn crate_name_from_cargo_toml(crate_dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(crate_dir.join("Cargo.toml")).ok()?;
    // Naive parse: find `name = "..."` in the file.
    for line in s.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start_matches([' ', '=']);
            if let Some(start) = rest.find('"') {
                if let Some(end) = rest[start + 1..].find('"') {
                    return Some(rest[start + 1..start + 1 + end].to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{AcceptanceCriterion, PItemStatus};
    use crate::plan::DagPlan;
    use crate::task::{Task, TaskStatus};
    use std::fs;
    use tempfile::TempDir;

    fn pitem_with(id: &str, touches: &[&str], deps: &[&str], acceptance: Vec<AcceptanceCriterion>) -> PItem {
        PItem {
            id: id.into(),
            desc: format!("do the {id} thing"),
            touches: touches.iter().map(PathBuf::from).collect(),
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            resources: vec![],
            estimate_secs: None,
            acceptance,
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }
    }

    fn sample_task(p_items: Vec<PItem>) -> Task {
        Task {
            id: "t-demo".into(),
            project_id: "demo-project".into(),
            title: "Add bookmarks".into(),
            description: "Implement bookmarks CRUD".into(),
            inbox_materials: vec![],
            plan: DagPlan::from_items(p_items),
            status: TaskStatus::Running,
            created_at: 0,
            started_at: None,
            completed_at: None,
            task_branch: Some("fleet/add-bookmarks".into()),
            master_session_id: None,
        }
    }

    #[test]
    fn compose_worker_system_prompt_includes_all_three_layers() {
        let p = pitem_with(
            "p1",
            &["src/foo.rs"],
            &[],
            vec![AcceptanceCriterion::Builds],
        );
        let task = sample_task(vec![p.clone()]);
        let prompt = compose_worker_system_prompt(&task, &p);
        assert!(prompt.contains("Layer 1"));
        assert!(prompt.contains("Layer 2"));
        assert!(prompt.contains("Layer 3"));
        assert!(prompt.contains("p1"));
        assert!(prompt.contains("Add bookmarks"));
        assert!(prompt.contains("src/foo.rs"));
        assert!(prompt.contains("Builds"));
    }

    #[test]
    fn layer1_warns_about_phase_commands() {
        let l1 = compose_layer1("any-project");
        assert!(l1.contains("Do not"));
        assert!(l1.contains("cargo build"));
        assert!(l1.contains("cargo check"));
        assert!(l1.contains("touches"));
    }

    #[test]
    fn layer2_wraps_user_data_in_untrusted_tags() {
        let task = sample_task(vec![]);
        let l2 = compose_layer2(&task);
        assert!(l2.contains("<untrusted_task_description>"));
        assert!(l2.contains("</untrusted_task_description>"));
        assert!(l2.contains("Implement bookmarks CRUD"));
    }

    #[test]
    fn layer3_includes_upstream_summaries_from_done_deps() {
        let mut upstream = pitem_with("upstream", &[], &[], vec![]);
        upstream.status = PItemStatus::Done;
        upstream.output_summary = Some("upstream did X and Y".into());
        let p = pitem_with("downstream", &["src/bar.rs"], &["upstream"], vec![]);
        let task = sample_task(vec![upstream, p.clone()]);
        let l3 = compose_layer3(&task, &p);
        assert!(l3.contains("Upstream summaries"));
        assert!(l3.contains("upstream did X and Y"));
        assert!(l3.contains("src/bar.rs"));
    }

    #[test]
    fn layer3_handles_no_upstream_gracefully() {
        let p = pitem_with("solo", &[], &[], vec![]);
        let task = sample_task(vec![p.clone()]);
        let l3 = compose_layer3(&task, &p);
        assert!(l3.contains("no upstream summaries"));
    }

    #[test]
    fn worker_spawn_spec_errors_for_unknown_pitem() {
        let task = sample_task(vec![]);
        let err = worker_spawn_spec(&task, "missing", PathBuf::from("/tmp/ws")).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn rust_crates_touched_walks_to_nearest_cargo_toml() {
        let dir = TempDir::new().unwrap();
        let crate_root = dir.path().join("crates").join("foo");
        fs::create_dir_all(crate_root.join("src")).unwrap();
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(crate_root.join("src").join("lib.rs"), "// lib\n").unwrap();
        let p = pitem_with(
            "p1",
            &["crates/foo/src/lib.rs", "crates/foo/src/other.rs"],
            &[],
            vec![],
        );
        let crates = rust_crates_touched(dir.path(), &p);
        assert_eq!(crates.len(), 1);
        assert!(crates.contains(&crate_root));
    }

    #[test]
    fn rust_crates_touched_returns_empty_when_no_cargo_toml() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src-ui")).unwrap();
        fs::write(dir.path().join("src-ui").join("App.tsx"), "export {}").unwrap();
        let p = pitem_with("p1", &["src-ui/App.tsx"], &[], vec![]);
        let crates = rust_crates_touched(dir.path(), &p);
        assert!(crates.is_empty());
    }

    #[test]
    fn rust_crates_touched_skips_virtual_workspace_root() {
        let dir = TempDir::new().unwrap();
        // Workspace root with no [package].
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/foo\"]\n",
        )
        .unwrap();
        let crate_root = dir.path().join("crates").join("foo");
        fs::create_dir_all(crate_root.join("src")).unwrap();
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(crate_root.join("src").join("lib.rs"), "// lib").unwrap();
        let p = pitem_with("p1", &["crates/foo/src/lib.rs"], &[], vec![]);
        let crates = rust_crates_touched(dir.path(), &p);
        assert_eq!(crates.len(), 1);
        assert!(crates.contains(&crate_root));
    }

    #[test]
    fn run_post_completion_check_returns_ok_with_no_rust_crates() {
        let dir = TempDir::new().unwrap();
        let p = pitem_with("p1", &["README.md"], &[], vec![]);
        let r = run_post_completion_check(dir.path(), &p);
        assert!(r.ok);
        assert!(r.crate_results.is_empty());
    }
}
