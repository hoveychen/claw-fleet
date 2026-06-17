//! `fleet.yaml` `verify:` section reader — the project-level commands the task
//! pipeline runs **for real** (P-item build/test gates + the task-level e2e
//! pass), as opposed to letting the review LLM merely eyeball the diff.
//!
//! Lives in `claw-fleet-task` (not the `claw-fleet-core::phase_detector`
//! stub) because the orchestrator and the review gate consume it, and
//! `claw-fleet-task` must not depend back on `claw-fleet-core`.
//!
//! Schema (all fields optional; absent → that gate is skipped):
//! ```yaml
//! # fleet.yaml — at the project root
//! verify:
//!   build: cargo build --workspace
//!   test: cargo test --workspace
//!   e2e: ./scripts/e2e.sh
//! ```
//! Unknown top-level keys (e.g. a future `phases:`) are ignored, so this reader
//! coexists with other `fleet.yaml` content.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Project-level verification commands read from `<workspace>/fleet.yaml`'s
/// `verify:` block. Every field is optional: `None` → that gate is skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyConfig {
    /// Command that satisfies a P-item's `Builds` acceptance criterion.
    #[serde(default)]
    pub build: Option<String>,
    /// Default command for a P-item's `TestsPass` criterion that carries no
    /// command of its own. `TestsPass(cmd)` always prefers its own `cmd`.
    #[serde(default)]
    pub test: Option<String>,
    /// Task-level end-to-end command, run once after every P-item is terminal
    /// and before the task parks in `AwaitingAcceptance`.
    #[serde(default)]
    pub e2e: Option<String>,
}

impl VerifyConfig {
    /// True when no command is configured — the caller can skip all real
    /// execution and fall back to detection / LLM judgment.
    pub fn is_empty(&self) -> bool {
        self.build.is_none() && self.test.is_none() && self.e2e.is_none()
    }
}

/// Only the `verify:` block matters here; everything else in `fleet.yaml` is
/// deliberately ignored (serde drops unknown fields by default).
#[derive(Deserialize)]
struct FleetYamlRoot {
    #[serde(default)]
    verify: Option<VerifyConfig>,
}

/// Read `<workspace>/fleet.yaml` and extract the `verify:` block.
///
/// - File missing → `Ok(VerifyConfig::default())` (no config is not an error).
/// - File present but unparseable → `Err` (surfaced; never silently treated as
///   "no verification").
/// - File present without a `verify:` block → `Ok(VerifyConfig::default())`.
pub fn read_verify_config(workspace: &Path) -> Result<VerifyConfig, String> {
    let path = workspace.join("fleet.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(VerifyConfig::default())
        }
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let root: FleetYamlRoot = serde_yaml::from_str(&text)
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(root.verify.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_yaml(dir: &TempDir, body: &str) {
        fs::write(dir.path().join("fleet.yaml"), body).unwrap();
    }

    #[test]
    fn missing_file_returns_default_not_error() {
        let dir = TempDir::new().unwrap();
        let cfg = read_verify_config(dir.path()).unwrap();
        assert!(cfg.is_empty());
        assert_eq!(cfg, VerifyConfig::default());
    }

    #[test]
    fn full_verify_block_parses_all_three() {
        let dir = TempDir::new().unwrap();
        write_yaml(
            &dir,
            "verify:\n  build: cargo build --workspace\n  test: cargo test --workspace\n  e2e: ./scripts/e2e.sh\n",
        );
        let cfg = read_verify_config(dir.path()).unwrap();
        assert_eq!(cfg.build.as_deref(), Some("cargo build --workspace"));
        assert_eq!(cfg.test.as_deref(), Some("cargo test --workspace"));
        assert_eq!(cfg.e2e.as_deref(), Some("./scripts/e2e.sh"));
        assert!(!cfg.is_empty());
    }

    #[test]
    fn partial_block_leaves_others_none() {
        let dir = TempDir::new().unwrap();
        write_yaml(&dir, "verify:\n  e2e: pnpm test:e2e\n");
        let cfg = read_verify_config(dir.path()).unwrap();
        assert_eq!(cfg.e2e.as_deref(), Some("pnpm test:e2e"));
        assert!(cfg.build.is_none());
        assert!(cfg.test.is_none());
    }

    #[test]
    fn no_verify_section_returns_default() {
        let dir = TempDir::new().unwrap();
        // Unrelated top-level content must not error and must not invent commands.
        write_yaml(&dir, "phases:\n  build:\n    cmd: make\n");
        let cfg = read_verify_config(dir.path()).unwrap();
        assert!(cfg.is_empty());
    }

    #[test]
    fn malformed_yaml_errors_rather_than_silently_skipping() {
        let dir = TempDir::new().unwrap();
        // A scalar where a mapping is expected — not valid for our schema.
        write_yaml(&dir, "verify: \"just a string\"\n");
        assert!(read_verify_config(dir.path()).is_err());
    }

    #[test]
    fn roundtrips_through_serde() {
        let cfg = VerifyConfig {
            build: Some("cargo build".into()),
            test: None,
            e2e: Some("e2e".into()),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VerifyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
