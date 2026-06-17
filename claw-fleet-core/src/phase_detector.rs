//! Auto-detect "phase" P-items for a project workspace.
//!
//! Phase P-items are the tail of every plan: `build-all`, `test-all`, `e2e`,
//! etc. They're not free-form work — they're meta-steps that run a known
//! command once per project shape. The planner often forgets them; this
//! detector fills them in by sniffing the workspace root.
//!
//! Detection rules (PRD §5.x / TASKS P13):
//! - `Cargo.toml` at root → `build-all` (`cargo build`) + `test-all` (`cargo test`)
//! - `package.json` at root → `frontend-build` (script-aware: `npm`/`pnpm`/`bun`)
//! - `playwright.config.{ts,js,mjs,cjs}` at root → `e2e` (`pnpm playwright test`)
//!
//! Output P-items have empty `touches` (they don't edit files), `dependsOn`
//! all the preceding code-touching P-items in the plan, and proper
//! `acceptance` / `resources`.
//!
//! `fleet.yaml` override reader: schema is parsed and validated here so the
//! V2 reader has a stable shape, but the V1 detector doesn't honor it yet —
//! always uses the built-in detection. (PRD P13 marks the override as
//! "predict schema, defer behavior".)

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::pitem::{AcceptanceCriterion, PItem, PItemId, PItemStatus};

/// What kind of phase P-item to inject. Order matters for `dependsOn`
/// chaining: `Build` runs before `Test` runs before `E2e`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhaseKind {
    /// `cargo build` / `npm run build` / `pnpm build` — workspace compile.
    Build,
    /// `cargo test` / `npm test` — workspace test sweep.
    Test,
    /// `playwright test` — end-to-end smoke.
    E2e,
}

/// One detected phase, ready to splat into a `DagPlan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPhase {
    pub kind: PhaseKind,
    pub id: PItemId,
    pub command: String,
    /// Free-text resource hints retained for display; no longer drives
    /// scheduling (resource scheduling was removed in the rebuild).
    pub resources: Vec<String>,
}

/// Built-in detection over a project root. Pure file-existence checks; no
/// command execution. Returns phases in canonical run order
/// (build → test → e2e). When no markers are found, returns empty.
pub fn detect_phases(workspace: &Path) -> Vec<DetectedPhase> {
    let mut out = Vec::new();

    let cargo = workspace.join("Cargo.toml").is_file();
    let pkg_json = workspace.join("package.json");
    let has_package_json = pkg_json.is_file();
    let pkg_manager = detect_package_manager(workspace);
    let playwright = ["playwright.config.ts", "playwright.config.js", "playwright.config.mjs", "playwright.config.cjs"]
        .iter()
        .any(|f| workspace.join(f).is_file());

    // Build: cargo + frontend build (combined into one phase if both present)
    if cargo {
        out.push(DetectedPhase {
            kind: PhaseKind::Build,
            id: "build-all".into(),
            command: "cargo build --workspace".into(),
            resources: vec!["build".into()],
        });
    }
    if has_package_json {
        let cmd = match pkg_manager {
            PackageManager::Pnpm => "pnpm build".into(),
            PackageManager::Bun => "bun run build".into(),
            PackageManager::Npm => "npm run build".into(),
        };
        out.push(DetectedPhase {
            kind: PhaseKind::Build,
            id: "frontend-build".into(),
            command: cmd,
            resources: vec!["build".into()],
        });
    }

    if cargo {
        out.push(DetectedPhase {
            kind: PhaseKind::Test,
            id: "test-all".into(),
            command: "cargo test --workspace".into(),
            resources: vec!["test".into()],
        });
    }

    if playwright {
        let cmd = match pkg_manager {
            PackageManager::Pnpm => "pnpm playwright test".into(),
            PackageManager::Bun => "bun playwright test".into(),
            PackageManager::Npm => "npx playwright test".into(),
        };
        out.push(DetectedPhase {
            kind: PhaseKind::E2e,
            id: "e2e".into(),
            command: cmd,
            resources: vec!["test".into(), "port:3000".into()],
        });
    }

    out
}

/// Convert detected phases into ready-to-append `PItem`s. `predecessor_ids`
/// is the list of code-touching P-items the build/test/e2e phases should
/// depend on. Each phase also chains after the preceding phase
/// (build → test → e2e) so the DAG runs them serially.
pub fn phases_to_pitems(phases: &[DetectedPhase], predecessor_ids: &[PItemId]) -> Vec<PItem> {
    let mut prev: Option<PItemId> = None;
    let mut out = Vec::with_capacity(phases.len());
    for ph in phases {
        let mut deps: Vec<PItemId> = predecessor_ids.to_vec();
        if let Some(prev_id) = &prev {
            if !deps.contains(prev_id) {
                deps.push(prev_id.clone());
            }
        }
        let acceptance = match ph.kind {
            PhaseKind::Build => vec![AcceptanceCriterion::Builds],
            PhaseKind::Test | PhaseKind::E2e => {
                vec![AcceptanceCriterion::TestsPass(ph.command.clone())]
            }
        };
        out.push(PItem {
            id: ph.id.clone(),
            desc: format!(
                "{} phase: {}",
                match ph.kind {
                    PhaseKind::Build => "Build",
                    PhaseKind::Test => "Test",
                    PhaseKind::E2e => "E2E",
                },
                ph.command
            ),
            touches: vec![],
            depends_on: deps,
            acceptance,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
            failure_gaps: Vec::new(),
        });
        prev = Some(ph.id.clone());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Pnpm,
    Bun,
    Npm,
}

fn detect_package_manager(workspace: &Path) -> PackageManager {
    if workspace.join("pnpm-lock.yaml").is_file() {
        PackageManager::Pnpm
    } else if workspace.join("bun.lockb").is_file() || workspace.join("bun.lock").is_file() {
        PackageManager::Bun
    } else {
        // Default to npm even if there's no package-lock.json — `npm run` is
        // the most universally available command.
        PackageManager::Npm
    }
}

/// V1 stub — parsed but ignored. Lives here so the V2 `fleet.yaml` reader
/// has a stable JSON-Schema-compatible shape from day one.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FleetYamlConfig {
    #[serde(default)]
    pub phases: std::collections::HashMap<String, FleetYamlPhase>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FleetYamlPhase {
    pub cmd: String,
    #[serde(default)]
    pub resources: Vec<String>,
}

/// V1 placeholder. The `FleetYamlConfig` schema is defined so V2 can wire a
/// real YAML reader without touching the detection API. V1 always returns
/// `None` — built-in detection wins.
pub fn try_read_fleet_yaml(_workspace: &Path) -> Option<Result<FleetYamlConfig, String>> {
    None
}

/// Return the directory a `dependsOn` chain of P-items should attach to —
/// helper for callers that just want every existing P-item to be a predecessor.
pub fn all_existing_ids(plan: &crate::plan::DagPlan) -> Vec<PItemId> {
    let mut ids: Vec<PItemId> = plan.items.keys().cloned().collect();
    ids.sort();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmpdir() -> TempDir {
        TempDir::new().expect("tempdir")
    }

    #[test]
    fn detects_rust_project() {
        let dir = tmpdir();
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let phases = detect_phases(dir.path());
        let ids: Vec<&str> = phases.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["build-all", "test-all"]);
        assert!(phases[0].command.contains("cargo build"));
        assert!(phases[1].command.contains("cargo test"));
    }

    #[test]
    fn detects_vite_npm_project() {
        let dir = tmpdir();
        fs::write(dir.path().join("package.json"), r#"{"name":"demo"}"#).unwrap();
        let phases = detect_phases(dir.path());
        let ids: Vec<&str> = phases.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["frontend-build"]);
        assert!(phases[0].command.starts_with("npm "));
    }

    #[test]
    fn picks_pnpm_when_lockfile_present() {
        let dir = tmpdir();
        fs::write(dir.path().join("package.json"), r#"{"name":"demo"}"#).unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: 9\n").unwrap();
        let phases = detect_phases(dir.path());
        assert!(phases[0].command.starts_with("pnpm "));
    }

    #[test]
    fn detects_playwright_e2e() {
        let dir = tmpdir();
        fs::write(dir.path().join("package.json"), r#"{"name":"demo"}"#).unwrap();
        fs::write(
            dir.path().join("playwright.config.ts"),
            "export default { testDir: 'e2e' }\n",
        )
        .unwrap();
        let phases = detect_phases(dir.path());
        let ids: Vec<&str> = phases.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"e2e"));
        let e2e = phases.iter().find(|p| p.id == "e2e").unwrap();
        assert!(e2e.command.contains("playwright"));
        assert!(e2e.resources.iter().any(|r| r.starts_with("port:")));
    }

    #[test]
    fn mixed_rust_plus_vite_plus_playwright() {
        let dir = tmpdir();
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(dir.path().join("package.json"), r#"{"name":"demo"}"#).unwrap();
        fs::write(
            dir.path().join("playwright.config.ts"),
            "export default {}\n",
        )
        .unwrap();
        let phases = detect_phases(dir.path());
        let ids: Vec<&str> = phases.iter().map(|p| p.id.as_str()).collect();
        // Build phase split into build-all (cargo) + frontend-build (npm),
        // then test-all (cargo), then e2e.
        assert_eq!(ids, vec!["build-all", "frontend-build", "test-all", "e2e"]);
    }

    #[test]
    fn empty_project_returns_no_phases() {
        let dir = tmpdir();
        assert!(detect_phases(dir.path()).is_empty());
    }

    #[test]
    fn phases_to_pitems_chains_in_run_order() {
        let phases = vec![
            DetectedPhase {
                kind: PhaseKind::Build,
                id: "build-all".into(),
                command: "cargo build".into(),
                resources: vec!["build".into()],
            },
            DetectedPhase {
                kind: PhaseKind::Test,
                id: "test-all".into(),
                command: "cargo test".into(),
                resources: vec!["test".into()],
            },
        ];
        let predecessors = vec!["p1".to_string(), "p2".to_string()];
        let items = phases_to_pitems(&phases, &predecessors);
        assert_eq!(items.len(), 2);
        // build-all depends on p1, p2
        assert!(items[0].depends_on.contains(&"p1".to_string()));
        assert!(items[0].depends_on.contains(&"p2".to_string()));
        // test-all depends on p1, p2, AND build-all (chained)
        assert!(items[1].depends_on.contains(&"build-all".to_string()));
        assert!(items[1].depends_on.contains(&"p1".to_string()));
    }

    #[test]
    fn fleet_yaml_v1_stub_returns_none() {
        let dir = tmpdir();
        // V1 placeholder — schema is reserved but reader is inert.
        assert!(try_read_fleet_yaml(dir.path()).is_none());
    }

    #[test]
    fn fleet_yaml_config_schema_roundtrips() {
        // The schema types are defined now so V2 can deserialize without
        // touching this file. Verify they round-trip through JSON.
        let mut phases = std::collections::HashMap::new();
        phases.insert(
            "build".to_string(),
            FleetYamlPhase {
                cmd: "cargo build".into(),
                resources: vec!["build".into()],
            },
        );
        let cfg = FleetYamlConfig { phases };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: FleetYamlConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
