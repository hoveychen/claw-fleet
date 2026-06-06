//! fleet.yaml configuration carrier (REQ-047, design §6.2).
//!
//! Scaffold created by master before Wave 3 so P15 can fill this without
//! racing P10 on `lib.rs`. P15 implements: parse `phases: [{name, cmd,
//! resources}]` and `resources: { custom_X: { concurrency: N } }`, validate,
//! and feed custom locks to the scheduler. See design/tasks-impl-plan.md (P15).
//!
//! # Why a hand-written parser (deviation)
//!
//! `claw-fleet-task`'s `Cargo.toml` does not depend on a YAML backend and
//! `Cargo.toml` is outside this P-item's `touches` list, so we cannot pull in
//! `serde_yaml`. The fleet.yaml schema is small and fully specified by REQ-047
//! / design §6.2, so this module ships a focused line-oriented parser that
//! covers exactly that schema (two-space indentation, the `phases` /
//! `resources.custom_*` keys, inline `[a, b]` resource lists). It is NOT a
//! general YAML parser — see the deviation note returned by the P-item. The
//! parser rejects shapes it does not understand rather than silently dropping
//! them, so a malformed file surfaces as a `FleetConfigError` instead of a
//! quietly-empty config.

use std::collections::BTreeMap;
use std::path::Path;

use crate::pitem::ResourceName;
use crate::plan::DagPlan;

/// The file name we look for at a project root.
pub const FLEET_CONFIG_FILENAME: &str = "fleet.yaml";

// One declared phase: a named build/test/etc. command plus the
// resources it holds while running. The Planner uses `cmd` to synthesise phase
// P-items; the Scheduler uses `resources` to serialise contending phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseConfig {
    pub name: String,
    pub cmd: String,
    pub resources: Vec<ResourceName>,
}

// A declared custom resource lock with a concurrency cap. `custom:db`
// with `concurrency: 1` is a mutex; `concurrency: 2` lets two holders run at
// once. The Scheduler consults this to decide whether contending P-items may
// run concurrently (see `runner::dispatchable_with_config`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomResource {
    /// Canonical resource name as P-items reference it, e.g. `custom:db`.
    pub name: ResourceName,
    /// Max number of P-items that may hold this resource at once (>= 1).
    pub concurrency: u32,
}

// Parsed fleet.yaml. Carries the declared phases and the custom
// resource concurrency map. Feeds both the Planner (phases) and the Scheduler
// (custom resource caps).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FleetConfig {
    pub phases: Vec<PhaseConfig>,
    /// Custom resource name -> concurrency cap.
    pub custom_resources: BTreeMap<ResourceName, CustomResource>,
}

/// Errors `load_fleet_config` / `parse_fleet_config` can raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetConfigError {
    /// The file could not be read.
    Io(String),
    /// A structural / semantic validation failure (bad resource name,
    /// duplicate phase, malformed concurrency, unknown shape).
    Validation(String),
}

impl std::fmt::Display for FleetConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FleetConfigError::Io(m) => write!(f, "fleet.yaml IO error: {m}"),
            FleetConfigError::Validation(m) => write!(f, "fleet.yaml invalid: {m}"),
        }
    }
}

impl std::error::Error for FleetConfigError {}

impl FleetConfig {
    /// The full set of resource names the Scheduler should be aware of:
    /// every custom resource declared here. Built-in `build` / `test` / `git`
    /// are implicit (concurrency 1) and need no declaration.
    // These are the "custom locks fed to the scheduler".
    pub fn custom_lock_names(&self) -> Vec<ResourceName> {
        self.custom_resources.keys().cloned().collect()
    }

    /// Concurrency cap for a resource. Custom resources use their declared cap;
    /// any non-declared resource (built-ins like `build`/`test`/`git`, or an
    /// undeclared `custom:*`) is treated as a mutex (cap 1) so the Scheduler
    /// never over-parallelises an unknown lock.
    // The Scheduler reads this to decide how many holders a resource
    // may have concurrently.
    pub fn concurrency_for(&self, resource: &str) -> u32 {
        self.custom_resources
            .get(resource)
            .map(|c| c.concurrency)
            .unwrap_or(1)
    }

    /// Validate that every `custom:*` resource referenced by a plan's P-items
    /// (or by the declared phases) is actually declared in `resources.custom`.
    /// Built-in resources (`build` / `test` / `git`) and non-custom scoped
    /// locks (`port:*`, `global:*`, …) are allowed without declaration.
    // Guards against a P-item referencing an undeclared custom lock,
    // which would otherwise be silently treated as a mutex with no operator
    // intent behind it.
    pub fn validate_plan_resources(&self, plan: &DagPlan) -> Result<(), FleetConfigError> {
        for item in plan.items.values() {
            for r in &item.resources {
                if r.starts_with("custom:") && !self.custom_resources.contains_key(r) {
                    return Err(FleetConfigError::Validation(format!(
                        "P-item {} references undeclared custom resource {:?}; \
                         declare it under resources.custom in fleet.yaml",
                        item.id, r
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Load and validate fleet.yaml from a project root directory. Returns
/// `Ok(None)` when no fleet.yaml exists (config is optional — projects fall
/// back to auto-detected phases). Returns `Ok(Some(_))` on a valid file and
/// `Err` on read / validation failure.
// Entry point named in the acceptance criterion.
pub fn load_fleet_config(project_root: &Path) -> Result<Option<FleetConfig>, FleetConfigError> {
    let path = project_root.join(FLEET_CONFIG_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| FleetConfigError::Io(format!("{}: {e}", path.display())))?;
    parse_fleet_config(&raw).map(Some)
}

/// Parse + validate a fleet.yaml document from its text. Split out from
/// `load_fleet_config` so tests can exercise the parser without touching the
/// filesystem.
// Parsing + validation core. Recognised shape (design §6.2,
// REQ-047 acceptance):
//
// ```yaml
// phases:
//   - name: build
//     cmd: cargo build
//     resources: [build]
// resources:
//   custom:db:
//     concurrency: 1
// ```
pub fn parse_fleet_config(text: &str) -> Result<FleetConfig, FleetConfigError> {
    let mut cfg = FleetConfig::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut seen_phase_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    while i < lines.len() {
        let line = lines[i];
        if is_blank_or_comment(line) {
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        let trimmed = line.trim();
        if indent != 0 {
            return Err(FleetConfigError::Validation(format!(
                "unexpected indentation at top level: {line:?}"
            )));
        }
        if trimmed == "phases:" {
            i = parse_phases(&lines, i + 1, &mut cfg, &mut seen_phase_names)?;
        } else if trimmed == "resources:" {
            i = parse_resources(&lines, i + 1, &mut cfg)?;
        } else {
            return Err(FleetConfigError::Validation(format!(
                "unknown top-level key (expected 'phases:' or 'resources:'): {line:?}"
            )));
        }
    }

    // Cross-check: any custom:* resource a phase declares must be declared too.
    for ph in &cfg.phases {
        for r in &ph.resources {
            if r.starts_with("custom:") && !cfg.custom_resources.contains_key(r) {
                return Err(FleetConfigError::Validation(format!(
                    "phase {:?} references undeclared custom resource {:?}",
                    ph.name, r
                )));
            }
        }
    }
    Ok(cfg)
}

/// Parse the `phases:` block (a YAML list of `{name, cmd, resources}` maps).
/// Returns the index of the next unconsumed line.
fn parse_phases(
    lines: &[&str],
    start: usize,
    cfg: &mut FleetConfig,
    seen: &mut std::collections::HashSet<String>,
) -> Result<usize, FleetConfigError> {
    let mut i = start;
    let mut current: Option<PhaseConfig> = None;

    let flush = |cur: &mut Option<PhaseConfig>,
                 cfg: &mut FleetConfig,
                 seen: &mut std::collections::HashSet<String>|
     -> Result<(), FleetConfigError> {
        if let Some(ph) = cur.take() {
            if ph.name.is_empty() {
                return Err(FleetConfigError::Validation(
                    "phase entry is missing a 'name'".into(),
                ));
            }
            if ph.cmd.is_empty() {
                return Err(FleetConfigError::Validation(format!(
                    "phase {:?} is missing a 'cmd'",
                    ph.name
                )));
            }
            // duplicate phase name -> error
            if !seen.insert(ph.name.clone()) {
                return Err(FleetConfigError::Validation(format!(
                    "duplicate phase name {:?}",
                    ph.name
                )));
            }
            cfg.phases.push(ph);
        }
        Ok(())
    };

    while i < lines.len() {
        let line = lines[i];
        if is_blank_or_comment(line) {
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        if indent == 0 {
            // Back to a top-level key: end of phases block.
            break;
        }
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("- ") {
            // Start of a new phase entry.
            flush(&mut current, cfg, seen)?;
            current = Some(PhaseConfig {
                name: String::new(),
                cmd: String::new(),
                resources: Vec::new(),
            });
            apply_phase_field(current.as_mut().unwrap(), rest.trim())?;
        } else if trimmed == "-" {
            flush(&mut current, cfg, seen)?;
            current = Some(PhaseConfig {
                name: String::new(),
                cmd: String::new(),
                resources: Vec::new(),
            });
        } else {
            let ph = current.as_mut().ok_or_else(|| {
                FleetConfigError::Validation(format!(
                    "phase field outside a list entry (missing leading '- '): {line:?}"
                ))
            })?;
            apply_phase_field(ph, trimmed)?;
        }
        i += 1;
    }
    flush(&mut current, cfg, seen)?;
    Ok(i)
}

/// Apply a `key: value` line to a phase entry.
fn apply_phase_field(ph: &mut PhaseConfig, field: &str) -> Result<(), FleetConfigError> {
    let (key, value) = split_kv(field)?;
    match key {
        "name" => ph.name = unquote(value).to_string(),
        "cmd" => ph.cmd = unquote(value).to_string(),
        "resources" => ph.resources = parse_inline_list(value)?,
        other => {
            return Err(FleetConfigError::Validation(format!(
                "unknown phase field {other:?} (expected name/cmd/resources)"
            )))
        }
    }
    Ok(())
}

/// Parse the `resources:` block. We only recognise the `custom:*` resource
/// declarations of the form:
///
/// ```yaml
/// resources:
///   custom:db:
///     concurrency: 1
/// ```
fn parse_resources(
    lines: &[&str],
    start: usize,
    cfg: &mut FleetConfig,
) -> Result<usize, FleetConfigError> {
    let mut i = start;
    let mut current_name: Option<String> = None;

    while i < lines.len() {
        let line = lines[i];
        if is_blank_or_comment(line) {
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        if indent == 0 {
            break; // next top-level key
        }
        let trimmed = line.trim();
        // The resource name itself contains a colon (`custom:db`), so we must
        // NOT classify lines by splitting on the first colon. Distinguish:
        //   - a *field* line — `concurrency: 2` — has a non-empty value after
        //     the last ": " separator;
        //   - a *header* line — `custom:db:` — ends in ':' with no trailing
        //     value.
        let header_name = trimmed.strip_suffix(':').map(str::trim);
        let field_kv = trimmed
            .rsplit_once(": ")
            .map(|(k, v)| (k.trim(), v.trim()))
            .filter(|(_, v)| !v.is_empty());

        if let Some((key, value)) = field_kv {
            // `key: value` field line.
            if key == "concurrency" {
                let name = current_name.clone().ok_or_else(|| {
                    FleetConfigError::Validation(
                        "'concurrency' outside a custom resource block".into(),
                    )
                })?;
                let n: u32 = value.parse().map_err(|_| {
                    FleetConfigError::Validation(format!(
                        "concurrency for {name:?} must be a non-negative integer, got {value:?}"
                    ))
                })?;
                if n < 1 {
                    return Err(FleetConfigError::Validation(format!(
                        "concurrency for {name:?} must be >= 1, got {n}"
                    )));
                }
                if let Some(cr) = cfg.custom_resources.get_mut(&name) {
                    cr.concurrency = n;
                }
            } else {
                return Err(FleetConfigError::Validation(format!(
                    "unknown field under custom resource: {trimmed:?} (expected 'concurrency')"
                )));
            }
        } else if let Some(name) = header_name.filter(|n| !n.is_empty()) {
            // Resource header line, e.g. `custom:db:`.
            let name = name.to_string();
            if !name.starts_with("custom:") {
                return Err(FleetConfigError::Validation(format!(
                    "only custom:* resource declarations are supported under \
                     resources:, got {name:?}"
                )));
            }
            // duplicate resource declaration -> error
            if cfg.custom_resources.contains_key(&name) {
                return Err(FleetConfigError::Validation(format!(
                    "duplicate custom resource declaration {name:?}"
                )));
            }
            cfg.custom_resources.insert(
                name.clone(),
                CustomResource {
                    name: name.clone(),
                    concurrency: 1,
                },
            );
            current_name = Some(name);
        } else {
            return Err(FleetConfigError::Validation(format!(
                "malformed resources entry: {line:?}"
            )));
        }
        i += 1;
    }
    Ok(i)
}

// ---- small parsing helpers -------------------------------------------------

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Split `key: value` on the first colon. The fleet.yaml keys we accept here
/// (name/cmd/resources) contain no colons, so the first colon is the separator.
fn split_kv(field: &str) -> Result<(&str, &str), FleetConfigError> {
    field
        .split_once(':')
        .map(|(k, v)| (k.trim(), v.trim()))
        .ok_or_else(|| FleetConfigError::Validation(format!("expected 'key: value', got {field:?}")))
}

/// Strip matching single/double quotes from a scalar value.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Parse an inline YAML flow list: `[a, b, c]` -> `["a","b","c"]`. An empty
/// `[]` yields an empty vec.
fn parse_inline_list(value: &str) -> Result<Vec<String>, FleetConfigError> {
    let v = value.trim();
    let inner = v
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| {
            FleetConfigError::Validation(format!(
                "resources must be an inline list like [build, test], got {value:?}"
            ))
        })?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner
        .split(',')
        .map(|item| unquote(item.trim()).to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{PItem, PItemStatus};

    fn pitem(id: &str, resources: &[&str]) -> PItem {
        PItem {
            id: id.into(),
            desc: id.into(),
            touches: vec![],
            depends_on: vec![],
            resources: resources.iter().map(|s| (*s).to_string()).collect(),
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
        }
    }

    // Parses the canonical fleet.yaml shape: phases list + custom
    // resource concurrency map.
    #[test]
    fn parses_phases_and_custom_resources() {
        let yaml = r#"
phases:
  - name: build
    cmd: cargo build
    resources: [build]
  - name: test
    cmd: cargo test
    resources: [test, custom:db]
resources:
  custom:db:
    concurrency: 1
"#;
        let cfg = parse_fleet_config(yaml).expect("parse");
        assert_eq!(cfg.phases.len(), 2);
        assert_eq!(cfg.phases[0].name, "build");
        assert_eq!(cfg.phases[0].cmd, "cargo build");
        assert_eq!(cfg.phases[0].resources, vec!["build".to_string()]);
        assert_eq!(cfg.phases[1].name, "test");
        assert_eq!(
            cfg.phases[1].resources,
            vec!["test".to_string(), "custom:db".to_string()]
        );
        assert_eq!(cfg.custom_resources.len(), 1);
        assert_eq!(cfg.concurrency_for("custom:db"), 1);
        assert_eq!(cfg.custom_lock_names(), vec!["custom:db".to_string()]);
    }

    // custom resource with concurrency > 1 is parsed as a counting
    // semaphore, not a mutex.
    #[test]
    fn custom_resource_concurrency_two() {
        let yaml = r#"
resources:
  custom:db:
    concurrency: 2
"#;
        let cfg = parse_fleet_config(yaml).expect("parse");
        assert_eq!(cfg.concurrency_for("custom:db"), 2);
        // built-ins / undeclared default to mutex
        assert_eq!(cfg.concurrency_for("build"), 1);
        assert_eq!(cfg.concurrency_for("custom:other"), 1);
    }

    // validation: duplicate phase name is rejected.
    #[test]
    fn rejects_duplicate_phase_name() {
        let yaml = r#"
phases:
  - name: build
    cmd: cargo build
    resources: [build]
  - name: build
    cmd: cargo build --release
    resources: [build]
"#;
        let err = parse_fleet_config(yaml).unwrap_err();
        assert!(
            matches!(&err, FleetConfigError::Validation(m) if m.contains("duplicate phase")),
            "got {err:?}"
        );
    }

    // validation: unknown top-level key rejected (illegal shape).
    #[test]
    fn rejects_unknown_top_level_key() {
        let yaml = "garbage:\n  foo: bar\n";
        let err = parse_fleet_config(yaml).unwrap_err();
        assert!(matches!(err, FleetConfigError::Validation(_)));
    }

    // validation: a phase referencing an undeclared custom resource
    // is rejected (illegal resource name relative to declarations).
    #[test]
    fn rejects_phase_with_undeclared_custom_resource() {
        let yaml = r#"
phases:
  - name: e2e
    cmd: pnpm e2e
    resources: [custom:redis]
"#;
        let err = parse_fleet_config(yaml).unwrap_err();
        assert!(
            matches!(&err, FleetConfigError::Validation(m) if m.contains("undeclared custom resource")),
            "got {err:?}"
        );
    }

    // validation: concurrency must be a positive integer.
    #[test]
    fn rejects_zero_concurrency() {
        let yaml = "resources:\n  custom:db:\n    concurrency: 0\n";
        let err = parse_fleet_config(yaml).unwrap_err();
        assert!(matches!(err, FleetConfigError::Validation(_)));
    }

    // validate_plan_resources catches a P-item referencing an
    // undeclared custom lock.
    #[test]
    fn validate_plan_resources_flags_undeclared() {
        let cfg = parse_fleet_config("resources:\n  custom:db:\n    concurrency: 1\n").unwrap();
        let mut plan = DagPlan::default();
        plan.items
            .insert("P1".into(), pitem("P1", &["custom:db"])); // declared, ok
        assert!(cfg.validate_plan_resources(&plan).is_ok());
        plan.items
            .insert("P2".into(), pitem("P2", &["custom:cache"])); // undeclared
        let err = cfg.validate_plan_resources(&plan).unwrap_err();
        assert!(matches!(err, FleetConfigError::Validation(_)));
    }

    // missing file -> Ok(None) (config optional).
    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let got = load_fleet_config(dir.path()).unwrap();
        assert!(got.is_none());
    }

    // load_fleet_config reads + validates from disk.
    #[test]
    fn loads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FLEET_CONFIG_FILENAME),
            "resources:\n  custom:db:\n    concurrency: 1\n",
        )
        .unwrap();
        let cfg = load_fleet_config(dir.path()).unwrap().unwrap();
        assert_eq!(cfg.concurrency_for("custom:db"), 1);
    }
}
