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

use crate::pitem::AcceptanceCriterion;
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
}
