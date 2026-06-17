//! `RealReviewGate` — the production [`ReviewGate`] (P6). Fills the seam the
//! orchestrator left: after a worker exits, judge whether the P-item's
//! acceptance criteria were actually met by running an isolated, read-only
//! `claude` review session and parsing its structured verdict.
//!
//! Flow: extract the worker's `git diff` from the P-item worktree → build the
//! review prompt (claw_fleet_task::review) → run
//! `claude -p --output-format json --json-schema <schema> --safe-mode
//!  --permission-mode bypassPermissions --no-session-persistence` to completion,
//! capturing stdout → `review::parse_verdict`.

use std::path::PathBuf;
use std::process::Command;

use claw_fleet_task::asset_inject;
use claw_fleet_task::orchestrator::{ReviewGate, ReviewVerdict};
use claw_fleet_task::pitem::PItem;
use claw_fleet_task::{review, verify, verify_config, worktree};
use claw_fleet_task::task::Task;

/// Production review gate. `workspace` is the project root (fallback cwd when
/// the P-item worktree can't be resolved).
pub struct RealReviewGate {
    pub workspace: PathBuf,
}

impl ReviewGate for RealReviewGate {
    fn review(&self, task: &Task, p_item: &PItem) -> Result<ReviewVerdict, String> {
        // The worktree still holds the worker's changes (reaped only after a
        // passing review's merge). Diff it against the task branch to get the
        // real changes the review judges — never trust the worker's summary.
        let wt = worktree::worktree_path(&task.id, &p_item.id)
            .ok()
            .filter(|p| p.exists())
            .unwrap_or_else(|| self.workspace.clone());

        // Mechanical gate FIRST: run the P-item's executable acceptance criteria
        // (Builds / TestsPass) in the worktree for real. Exit codes are ground
        // truth — if any fails we reject without ever asking the LLM, so a
        // worker that "claims done" but doesn't compile/pass can't slip past a
        // model eyeballing the diff. Config from <workspace>/fleet.yaml; an
        // unreadable file degrades to "no mechanical gate" (logged) rather than
        // bricking every review.
        let cfg = verify_config::read_verify_config(&self.workspace).unwrap_or_else(|e| {
            eprintln!("[review] fleet.yaml verify config unreadable, skipping mechanical gate: {e}");
            Default::default()
        });
        if let Err(gaps) = verify::run_mechanical_gate(p_item, &cfg, &wt) {
            return Ok(ReviewVerdict { achieved: false, gaps });
        }

        let task_branch = task.task_branch.as_deref().unwrap_or("HEAD");
        let diff = git_diff(&wt, task_branch).unwrap_or_else(|e| {
            format!("(could not compute diff: {e})")
        });

        let spec = review::review_spawn_spec(task, p_item, &diff, wt);
        // Review is a safe-mode session too — transplant the engineering
        // principles (方案 S) so its judgment shares the user's standards.
        let system_prompt = format!(
            "{}\n\n{}",
            spec.system_prompt,
            asset_inject::compose_engineering_principles()
        );
        let stdout = run_review(&spec.cwd, &system_prompt, &spec.model)?;
        review::parse_verdict(&stdout)
    }
}

/// Run the isolated review `claude` session to completion and return its
/// stdout. Uses the verified structured-output flags (`--output-format json`
/// + `--json-schema`) so stdout carries the `{achieved, gaps}` verdict.
fn run_review(cwd: &std::path::Path, system_prompt: &str, model: &str) -> Result<String, String> {
    let mut cmd = claw_fleet_task::process_util::command("claude");
    cmd.current_dir(cwd)
        .arg("--print")
        .arg("--output-format")
        .arg("json")
        .arg("--json-schema")
        .arg(review::REVIEW_SCHEMA)
        .arg("--safe-mode")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg("--no-session-persistence")
        .arg("--model")
        .arg(model)
        .arg("--append-system-prompt")
        .arg(system_prompt)
        .arg("现在对照验收标准与真实 diff 做出裁决，只输出符合 schema 的 JSON。")
        .env("FLEET_SESSION_KIND", "review");
    let out = cmd.output().map_err(|e| format!("spawn review claude: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "review claude exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// `git -C <worktree> diff <base>` — the worker's changes relative to the task
/// branch (covers both committed and working-tree changes).
fn git_diff(worktree: &std::path::Path, base: &str) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("diff")
        .arg(base)
        .output()
        .map_err(|e| format!("git diff: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
