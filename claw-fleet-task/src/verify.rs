//! Map a P-item's `AcceptanceCriterion` to the shell command the review gate
//! should actually run, given the project's [`VerifyConfig`] and workspace.
//!
//! Pure resolution only — no execution lives here (that's the review gate in
//! `fleet-task`). Kept pure so it is unit-testable without spawning anything.
//!
//! Resolution rules:
//! - `Builds` → `verify.build` from fleet.yaml, else a built-in detection
//!   (Cargo.toml → `cargo build`, package.json → `npm run build`, go.mod →
//!   `go build ./...`). `None` when nothing is detected.
//! - `TestsPass(cmd)` → its own `cmd` when non-empty, else `verify.test`.
//! - `Custom(_)` / `HumanReview` → `None` (semantic / human judgment, not a
//!   mechanical command — left to the LLM review or the human gate).

use std::path::Path;
use std::process::Command;

use crate::pitem::{AcceptanceCriterion, PItem};
use crate::verify_config::VerifyConfig;

/// Resolve the executable command for one acceptance criterion, or `None` when
/// the criterion isn't mechanically checkable (Custom / HumanReview) or no
/// build command can be found.
pub fn resolve_pitem_command(
    criterion: &AcceptanceCriterion,
    cfg: &VerifyConfig,
    workspace: &Path,
) -> Option<String> {
    match criterion {
        AcceptanceCriterion::Builds => cfg
            .build
            .clone()
            .filter(|c| !c.trim().is_empty())
            .or_else(|| detect_build_command(workspace)),
        AcceptanceCriterion::TestsPass(cmd) => {
            if cmd.trim().is_empty() {
                cfg.test.clone().filter(|c| !c.trim().is_empty())
            } else {
                Some(cmd.clone())
            }
        }
        AcceptanceCriterion::Custom(_) | AcceptanceCriterion::HumanReview => None,
    }
}

/// Built-in build-command detection from project markers. Mirrors (a minimal
/// subset of) `claw_fleet_core::phase_detector`, re-implemented here because
/// `claw-fleet-task` must not depend back on `claw-fleet-core`.
pub fn detect_build_command(workspace: &Path) -> Option<String> {
    if workspace.join("Cargo.toml").exists() {
        return Some("cargo build".to_string());
    }
    if workspace.join("package.json").exists() {
        return Some("npm run build".to_string());
    }
    if workspace.join("go.mod").exists() {
        return Some("go build ./...".to_string());
    }
    None
}

/// Outcome of running one acceptance command in a worktree.
pub struct CheckRun {
    pub command: String,
    pub ok: bool,
    /// Trailing slice of combined stdout+stderr — populated only on failure so
    /// the gap message carries the real error, not a wall of build noise.
    pub output_tail: String,
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = crate::process_util::command("cmd");
        c.arg("/C").arg(command);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// Run one acceptance command in `cwd`, capturing pass/fail + (on failure) the
/// tail of its combined output. A spawn error counts as a failure.
pub fn run_check(command: &str, cwd: &Path) -> CheckRun {
    match shell_command(command).current_dir(cwd).output() {
        Ok(o) => {
            let ok = o.status.success();
            let tail = if ok {
                String::new()
            } else {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                tail_chars(s.trim(), 1500)
            };
            CheckRun { command: command.to_string(), ok, output_tail: tail }
        }
        Err(e) => CheckRun {
            command: command.to_string(),
            ok: false,
            output_tail: format!("spawn failed: {e}"),
        },
    }
}

/// Keep the last `n` characters of `s` (so the gap message shows where a build
/// / test run actually failed, which is almost always at the end).
fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    let tail: String = s.chars().skip(count - n).collect();
    format!("…{tail}")
}

/// The mechanical gate: run every executable acceptance criterion of `p_item`
/// in `cwd`. `Ok(())` when all pass (or none are mechanically checkable);
/// `Err(gaps)` listing each failed command + its output tail. This runs **before**
/// the LLM review — exit codes are ground truth, not a diff the model eyeballs.
pub fn run_mechanical_gate(
    p_item: &PItem,
    cfg: &VerifyConfig,
    cwd: &Path,
) -> Result<(), Vec<String>> {
    let mut gaps = Vec::new();
    for crit in &p_item.acceptance {
        let Some(cmd) = resolve_pitem_command(crit, cfg, cwd) else {
            continue;
        };
        let run = run_check(&cmd, cwd);
        if !run.ok {
            gaps.push(format!("验收命令 `{}` 未通过（退出码非 0）", run.command));
            if !run.output_tail.is_empty() {
                gaps.push(format!("输出末尾：{}", run.output_tail));
            }
        }
    }
    if gaps.is_empty() {
        Ok(())
    } else {
        Err(gaps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn cfg(build: Option<&str>, test: Option<&str>) -> VerifyConfig {
        VerifyConfig {
            build: build.map(String::from),
            test: test.map(String::from),
            e2e: None,
        }
    }

    #[test]
    fn builds_prefers_config_over_detection() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        let c = cfg(Some("just build"), None);
        assert_eq!(
            resolve_pitem_command(&AcceptanceCriterion::Builds, &c, dir.path()),
            Some("just build".to_string())
        );
    }

    #[test]
    fn builds_falls_back_to_detection_when_config_absent() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            resolve_pitem_command(&AcceptanceCriterion::Builds, &VerifyConfig::default(), dir.path()),
            Some("cargo build".to_string())
        );
    }

    #[test]
    fn builds_none_when_no_config_and_no_markers() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            resolve_pitem_command(&AcceptanceCriterion::Builds, &VerifyConfig::default(), dir.path()),
            None
        );
    }

    #[test]
    fn tests_pass_uses_own_command() {
        let dir = TempDir::new().unwrap();
        let crit = AcceptanceCriterion::TestsPass("cargo test -p foo".into());
        assert_eq!(
            resolve_pitem_command(&crit, &VerifyConfig::default(), dir.path()),
            Some("cargo test -p foo".to_string())
        );
    }

    #[test]
    fn empty_tests_pass_falls_back_to_config_test() {
        let dir = TempDir::new().unwrap();
        let crit = AcceptanceCriterion::TestsPass("   ".into());
        let c = cfg(None, Some("cargo test --workspace"));
        assert_eq!(
            resolve_pitem_command(&crit, &c, dir.path()),
            Some("cargo test --workspace".to_string())
        );
    }

    #[test]
    fn custom_and_human_review_are_not_mechanical() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            resolve_pitem_command(&AcceptanceCriterion::Custom("looks right".into()), &VerifyConfig::default(), dir.path()),
            None
        );
        assert_eq!(
            resolve_pitem_command(&AcceptanceCriterion::HumanReview, &VerifyConfig::default(), dir.path()),
            None
        );
    }

    #[test]
    fn detect_prefers_rust_then_node_then_go() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_build_command(dir.path()), Some("npm run build".to_string()));
        fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(detect_build_command(dir.path()), Some("cargo build".to_string()));
    }

    fn pitem_with(acceptance: Vec<AcceptanceCriterion>) -> PItem {
        PItem {
            id: "p1".into(),
            desc: "x".into(),
            touches: vec![],
            depends_on: vec![],
            acceptance,
            human_gate: false,
            status: crate::pitem::PItemStatus::Reviewing,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
            failure_gaps: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_check_reports_exit_code() {
        let dir = TempDir::new().unwrap();
        assert!(run_check("true", dir.path()).ok);
        let fail = run_check("false", dir.path());
        assert!(!fail.ok);
    }

    #[cfg(unix)]
    #[test]
    fn run_check_captures_output_tail_on_failure() {
        let dir = TempDir::new().unwrap();
        let r = run_check("echo boom-marker >&2; exit 1", dir.path());
        assert!(!r.ok);
        assert!(r.output_tail.contains("boom-marker"), "tail: {}", r.output_tail);
    }

    #[cfg(unix)]
    #[test]
    fn mechanical_gate_passes_when_command_succeeds() {
        let dir = TempDir::new().unwrap();
        let p = pitem_with(vec![AcceptanceCriterion::TestsPass("true".into())]);
        assert!(run_mechanical_gate(&p, &VerifyConfig::default(), dir.path()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn mechanical_gate_fails_with_gaps_when_command_fails() {
        let dir = TempDir::new().unwrap();
        let p = pitem_with(vec![AcceptanceCriterion::TestsPass("false".into())]);
        let res = run_mechanical_gate(&p, &VerifyConfig::default(), dir.path());
        let gaps = res.unwrap_err();
        assert!(gaps.iter().any(|g| g.contains("false") && g.contains("退出码")));
    }

    #[cfg(unix)]
    #[test]
    fn mechanical_gate_skips_non_executable_criteria() {
        let dir = TempDir::new().unwrap();
        // Custom + HumanReview resolve to no command → gate passes (LLM/human decides).
        let p = pitem_with(vec![
            AcceptanceCriterion::Custom("looks good".into()),
            AcceptanceCriterion::HumanReview,
        ]);
        assert!(run_mechanical_gate(&p, &VerifyConfig::default(), dir.path()).is_ok());
    }

    #[test]
    fn tail_chars_keeps_last_n() {
        assert_eq!(tail_chars("abcdef", 3), "…def");
        assert_eq!(tail_chars("ab", 5), "ab");
    }
}
