//! Contract test: every example YAML shipped with the `atomic-plan-tasks`
//! skill must deserialize cleanly into `Vec<PItem>` and form a valid `DagPlan`.
//!
//! This is the automated half of P5's acceptance criterion. If the schema in
//! `claw-fleet-core/src/pitem.rs` drifts, these will fail and the skill prompt
//! needs to be updated to match.

use serde::Deserialize;

use claw_fleet_core::pitem::PItem;
use claw_fleet_core::plan::DagPlan;

#[derive(Debug, Deserialize)]
struct ExamplePlan {
    plan: Vec<PItem>,
}

const LINEAR: &str = include_str!("../../skills/atomic-plan-tasks/examples/linear.yaml");
const FAN_OUT: &str = include_str!("../../skills/atomic-plan-tasks/examples/fan-out.yaml");
const COMPLEX: &str = include_str!("../../skills/atomic-plan-tasks/examples/complex.yaml");

/// Parse skill-output YAML into a typed plan.
///
/// `serde_yaml` rejects single-key-map syntax for externally-tagged enum
/// variants (it expects YAML tags like `!testsPass`), so we deserialize via
/// `serde_yaml::Value` → `serde_json::Value` → `serde_json::from_value`.
/// The YAML→JSON conversion produces the same shape `serde_json` would emit
/// for our schema, so `{ testsPass: "cargo test" }` round-trips cleanly.
fn load(name: &str, yaml: &str) -> Vec<PItem> {
    let yv: serde_yaml::Value = serde_yaml::from_str(yaml)
        .unwrap_or_else(|e| panic!("{name}: yaml lex failed: {e}"));
    let jv: serde_json::Value = serde_json::to_value(&yv)
        .unwrap_or_else(|e| panic!("{name}: yaml → json convert failed: {e}"));
    let parsed: ExamplePlan = serde_json::from_value(jv)
        .unwrap_or_else(|e| panic!("{name}: deserialize failed: {e}"));
    assert!(!parsed.plan.is_empty(), "{name}: plan is empty");
    parsed.plan
}

fn assert_plan_is_valid(name: &str, items: Vec<PItem>) {
    let dag = DagPlan::from_items(items);
    if let Err(e) = dag.validate() {
        panic!("{name}: DagPlan validation failed: {e:?}");
    }
    assert!(
        dag.topo_sort().is_ok(),
        "{name}: topo_sort failed despite passing validate"
    );
}

#[test]
fn linear_example_deserializes() {
    let items = load("linear", LINEAR);
    assert_eq!(items.len(), 4);
    assert_plan_is_valid("linear", items);
}

#[test]
fn fan_out_example_deserializes() {
    let items = load("fan-out", FAN_OUT);
    assert!(items.len() >= 5, "fan-out should have ≥5 items");
    assert_plan_is_valid("fan-out", items);
}

#[test]
fn complex_example_deserializes() {
    let items = load("complex", COMPLEX);
    assert!(items.len() >= 8, "complex should have ≥8 items");
    assert_plan_is_valid("complex", items);
}

#[test]
fn examples_default_status_to_wait_deps() {
    // Planner output omits `status`. Schema must default it to WaitDeps so the
    // scheduler sees a sensible starting state.
    for (name, yaml) in [("linear", LINEAR), ("fan-out", FAN_OUT), ("complex", COMPLEX)] {
        for item in load(name, yaml) {
            assert!(
                matches!(item.status, claw_fleet_core::pitem::PItemStatus::WaitDeps),
                "{name}: P-item {} should default to WaitDeps, got {:?}",
                item.id,
                item.status
            );
        }
    }
}

#[test]
fn complex_example_exercises_full_schema_surface() {
    // Spot-check that the "complex" example uses every PItem patch field —
    // acceptance, artifacts, skippable, humanGate, resources — so the
    // skill examples teach the planner the full surface.
    let items = load("complex", COMPLEX);
    let any_human_gate = items.iter().any(|p| p.human_gate);
    let any_skippable = items.iter().any(|p| p.skippable.is_some());
    let any_artifact = items.iter().any(|p| !p.artifacts.is_empty());
    let any_resource = items.iter().any(|p| !p.resources.is_empty());
    let any_acceptance = items.iter().any(|p| !p.acceptance.is_empty());
    assert!(
        any_human_gate && any_skippable && any_artifact && any_resource && any_acceptance,
        "complex.yaml must demonstrate the full schema: \
         humanGate={any_human_gate} skippable={any_skippable} artifacts={any_artifact} \
         resources={any_resource} acceptance={any_acceptance}"
    );
}
