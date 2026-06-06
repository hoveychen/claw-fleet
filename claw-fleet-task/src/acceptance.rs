//! Acceptance verification engine.
//!
//! The methodology's adversarial-audit pillar (PRD §5.7) forbids the master
//! from flipping a P-item to `Done` on *proxy* signals (worker self-report,
//! token count, diff size, elapsed time). Instead each `AcceptanceCriterion`
//! the planner declared must be **actually checked** by a callable function
//! that produces structured evidence. This module is that engine:
//!
//! - [`check_builds`] — `cargo build` for [`AcceptanceCriterion::Builds`].
//! - [`check_tests_pass`] — runs the declared test command for
//!   [`AcceptanceCriterion::TestsPass`].
//! - [`check_human_review`] — defers [`AcceptanceCriterion::HumanReview`] to a
//!   human; the engine never auto-passes it.
//! - [`eval_custom`] — runs a declared shell rule for
//!   [`AcceptanceCriterion::Custom`].
//!
//! Each returns a structured [`AcceptanceCheckResult`] (criterion + passed +
//! evidence + checked_at) that can be persisted as JSON for the audit trail.
//! [`audit_acceptance`] runs the whole declared list in order and applies the
//! all-must-pass rule: declaring `[Builds, TestsPass]` but only passing
//! `Builds` rejects the P-item (REQ-006/050).
//!
//! The *retry-then-ask* escalation (DEC-007: when acceptance is uncertain,
//! retry the worker once before asking the user) is orchestrated by the master
//! in P9 — this module only surfaces the [`AuditDecision`] so P9 can route it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pitem::AcceptanceCriterion;

/// Structured outcome of checking a single [`AcceptanceCriterion`]. Persisted
/// to disk as JSON so the audit trail records *what was checked, whether it
/// passed, and the concrete evidence* — never a proxy signal.
///
/// [REQ-006] Every declared criterion is checked individually and yields one
/// of these; the master iterates `PItem.acceptance` in declared order and
/// collects the results.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCheckResult {
    /// The criterion that was checked (carries its payload, e.g. the test cmd).
    pub criterion: AcceptanceCriterion,
    /// `true` only when the engine gathered real passing evidence. A criterion
    /// that needs a human ([`AcceptanceCriterion::HumanReview`]) is never
    /// `passed = true` from this engine — it is deferred, not auto-passed.
    pub passed: bool,
    /// Concrete evidence: command exit summary, stderr/stdout tail, or the
    /// reason a check was deferred. Never a proxy signal (token count, elapsed
    /// time, diff size, worker self-report).
    pub evidence: String,
    /// Whether this criterion's verdict requires a human before it can pass.
    /// `true` for [`AcceptanceCriterion::HumanReview`]; the master must route
    /// it through `AskUserQuestion` rather than treating `passed = false` as a
    /// hard failure.
    #[serde(default)]
    pub needs_human: bool,
    /// Unix epoch seconds when the check ran.
    pub checked_at: i64,
}

impl AcceptanceCheckResult {
    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn passed(criterion: AcceptanceCriterion, evidence: impl Into<String>) -> Self {
        Self {
            criterion,
            passed: true,
            evidence: evidence.into(),
            needs_human: false,
            checked_at: Self::now(),
        }
    }

    fn failed(criterion: AcceptanceCriterion, evidence: impl Into<String>) -> Self {
        Self {
            criterion,
            passed: false,
            evidence: evidence.into(),
            needs_human: false,
            checked_at: Self::now(),
        }
    }

    fn deferred_to_human(criterion: AcceptanceCriterion, evidence: impl Into<String>) -> Self {
        Self {
            criterion,
            passed: false,
            evidence: evidence.into(),
            needs_human: true,
            checked_at: Self::now(),
        }
    }
}

// ── individual criterion checks ──────────────────────────────────────────────

/// Tail of combined stdout+stderr, capped to the last `n` lines, so evidence
/// stays bounded for very chatty builds/tests.
fn output_tail(stdout: &[u8], stderr: &[u8], n: usize) -> String {
    let mut combined = String::new();
    let so = String::from_utf8_lossy(stdout);
    let se = String::from_utf8_lossy(stderr);
    if !so.trim().is_empty() {
        combined.push_str(&so);
    }
    if !se.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&se);
    }
    let lines: Vec<&str> = combined.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// [REQ-006] Check [`AcceptanceCriterion::Builds`] by running `cargo build` in
/// `workspace`. Passes iff the build exits 0; evidence is the stderr tail
/// (cargo writes diagnostics to stderr).
pub fn check_builds(workspace: &Path) -> AcceptanceCheckResult {
    let out = crate::process_util::command("cargo")
        .arg("build")
        .current_dir(workspace)
        .output();
    match out {
        Ok(out) => {
            let tail = output_tail(&out.stdout, &out.stderr, 40);
            if out.status.success() {
                AcceptanceCheckResult::passed(
                    AcceptanceCriterion::Builds,
                    format!("`cargo build` exited 0\n{tail}"),
                )
            } else {
                AcceptanceCheckResult::failed(
                    AcceptanceCriterion::Builds,
                    format!("`cargo build` exited {}\n{tail}", out.status),
                )
            }
        }
        Err(e) => AcceptanceCheckResult::failed(
            AcceptanceCriterion::Builds,
            format!("failed to spawn `cargo build`: {e}"),
        ),
    }
}

/// [REQ-006] Check [`AcceptanceCriterion::TestsPass`] by running the declared
/// command line in `workspace` via the platform shell. Passes iff the command
/// exits 0; evidence is the exit status + output tail.
pub fn check_tests_pass(workspace: &Path, cmd: &str) -> AcceptanceCheckResult {
    let criterion = AcceptanceCriterion::TestsPass(cmd.to_string());
    if cmd.trim().is_empty() {
        return AcceptanceCheckResult::failed(
            criterion,
            "TestsPass criterion has an empty command — nothing to run".to_string(),
        );
    }
    match run_shell(workspace, cmd) {
        Ok((status_ok, status_desc, tail)) => {
            if status_ok {
                AcceptanceCheckResult::passed(
                    criterion,
                    format!("`{cmd}` {status_desc}\n{tail}"),
                )
            } else {
                AcceptanceCheckResult::failed(
                    criterion,
                    format!("`{cmd}` {status_desc}\n{tail}"),
                )
            }
        }
        Err(e) => AcceptanceCheckResult::failed(
            criterion,
            format!("failed to spawn `{cmd}`: {e}"),
        ),
    }
}

/// [REQ-011] [`AcceptanceCriterion::HumanReview`] is never auto-passed by the
/// engine. It always returns a *deferred* result (`needs_human = true`,
/// `passed = false`) so the master routes it through `AskUserQuestion`.
pub fn check_human_review() -> AcceptanceCheckResult {
    AcceptanceCheckResult::deferred_to_human(
        AcceptanceCriterion::HumanReview,
        "HumanReview criterion requires explicit human approval — the engine \
         never auto-passes it; master must ask the user"
            .to_string(),
    )
}

/// [REQ-006] Evaluate [`AcceptanceCriterion::Custom`] by running the declared
/// rule as a shell command in `workspace`. Convention: the rule passes iff the
/// command exits 0 (e.g. `cargo clippy -- -D warnings`).
pub fn eval_custom(workspace: &Path, rule: &str) -> AcceptanceCheckResult {
    let criterion = AcceptanceCriterion::Custom(rule.to_string());
    if rule.trim().is_empty() {
        return AcceptanceCheckResult::failed(
            criterion,
            "Custom criterion has an empty rule — nothing to evaluate".to_string(),
        );
    }
    match run_shell(workspace, rule) {
        Ok((status_ok, status_desc, tail)) => {
            if status_ok {
                AcceptanceCheckResult::passed(
                    criterion,
                    format!("custom rule `{rule}` {status_desc}\n{tail}"),
                )
            } else {
                AcceptanceCheckResult::failed(
                    criterion,
                    format!("custom rule `{rule}` {status_desc}\n{tail}"),
                )
            }
        }
        Err(e) => AcceptanceCheckResult::failed(
            criterion,
            format!("failed to spawn custom rule `{rule}`: {e}"),
        ),
    }
}

/// Run `cmd` through the platform shell in `workspace`. Returns
/// `(exited_zero, status_description, output_tail)`.
fn run_shell(
    workspace: &Path,
    cmd: &str,
) -> std::io::Result<(bool, String, String)> {
    #[cfg(windows)]
    let out = crate::process_util::command("cmd")
        .arg("/C")
        .arg(cmd)
        .current_dir(workspace)
        .output()?;
    #[cfg(not(windows))]
    let out = crate::process_util::command("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workspace)
        .output()?;
    let tail = output_tail(&out.stdout, &out.stderr, 40);
    let desc = if out.status.success() {
        "exited 0".to_string()
    } else {
        format!("exited {}", out.status)
    };
    Ok((out.status.success(), desc, tail))
}

/// Dispatch a single criterion to its checker. `Builds` / `TestsPass` /
/// `Custom` run real commands in `workspace`; `HumanReview` is deferred.
///
/// [REQ-006][REQ-050] The master calls this per declared criterion, in order.
pub fn check_one(workspace: &Path, criterion: &AcceptanceCriterion) -> AcceptanceCheckResult {
    match criterion {
        AcceptanceCriterion::Builds => check_builds(workspace),
        AcceptanceCriterion::TestsPass(cmd) => check_tests_pass(workspace, cmd),
        AcceptanceCriterion::HumanReview => check_human_review(),
        AcceptanceCriterion::Custom(rule) => eval_custom(workspace, rule),
    }
}

// ── full-list audit + verdict ────────────────────────────────────────────────

/// The verdict the master acts on after auditing the full criterion list.
///
/// [REQ-050] mark-done logic: all pass → `Done`; any hard fail → `Failed`;
/// some criterion needs a human (or P-item/task gating is on) → `WaitHuman`.
/// [DEC-007] When uncertain, P9 retries the worker once before escalating;
/// this enum only reports the verdict, not the retry orchestration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AuditDecision {
    /// Every declared criterion gathered real passing evidence and no human
    /// gate applies. Safe to flip the P-item to `Done`.
    AllPassed,
    /// At least one criterion produced failing evidence (no human deferral was
    /// the blocker). The P-item must move to `Failed`, not `Done`.
    Rejected,
    /// No hard failure, but a human must weigh in before `Done` — either a
    /// `HumanReview` criterion was declared, or the human gate is on.
    WaitHuman,
}

/// Full audit of a P-item's declared acceptance list, in declared order.
///
/// [REQ-006][REQ-050] Iterates every criterion, checks it, and applies the
/// all-must-pass rule. Declaring `[Builds, TestsPass]` but only `Builds`
/// passing yields `Rejected` — the engine never lets a partial pass through.
///
/// [REQ-011] The `gate` precedence (P-item `human_gate` OR task
/// `manual_review_all`) is folded in: even when every machine-checkable
/// criterion passes, an active gate forces `WaitHuman` so the master cannot
/// unilaterally mark done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport {
    pub results: Vec<AcceptanceCheckResult>,
    pub decision: AuditDecision,
}

/// Run the full acceptance audit. `gate` is the resolved human-gate flag — see
/// [`requires_human_gate`] for how the master computes it from the P-item and
/// the task.
pub fn audit_acceptance(
    workspace: &Path,
    criteria: &[AcceptanceCriterion],
    gate: bool,
) -> AuditReport {
    let results: Vec<AcceptanceCheckResult> = criteria
        .iter()
        .map(|c| check_one(workspace, c))
        .collect();

    let any_hard_fail = results.iter().any(|r| !r.passed && !r.needs_human);
    let any_needs_human = results.iter().any(|r| r.needs_human);

    let decision = if any_hard_fail {
        // [REQ-050] Any failing criterion → reject. A hard failure outranks a
        // pending human review: there's no point asking a human to bless a
        // build that doesn't compile.
        AuditDecision::Rejected
    } else if any_needs_human || gate {
        // [REQ-011] HumanReview criterion or an active human gate → WaitHuman,
        // never straight to Done.
        AuditDecision::WaitHuman
    } else {
        AuditDecision::AllPassed
    };

    AuditReport { results, decision }
}

/// [REQ-011][REQ-013][REQ-050] Human-gate precedence. The P-item is gated when
/// **either** its own `human_gate` flag is set **or** the task-level
/// `manual_review_all` is on. Mirrors the master template wording
/// ("P-item.human_gate=true 或 project.manual_review_all 开启"): a gate at
/// *either* level forces human review, so neither side can be silently
/// bypassed by the other being false.
pub fn requires_human_gate(p_item_human_gate: bool, task_manual_review: bool) -> bool {
    p_item_human_gate || task_manual_review
}

// ── persistence ──────────────────────────────────────────────────────────────

/// Directory under `~/.fleet/` where per-(task, P-item) acceptance check
/// results are persisted as JSON for the audit trail.
fn acceptance_dir() -> Option<PathBuf> {
    crate::paths::get_fleet_dir().map(|d| d.join("acceptance"))
}

/// [REQ-006] Persist a batch of [`AcceptanceCheckResult`] to disk as JSON so
/// the audit trail records what was checked. Written atomically (temp file +
/// rename) to `~/.fleet/acceptance/<task_id>__<p_item_id>.json`. Returns the
/// path written.
pub fn persist_results(
    task_id: &str,
    p_item_id: &str,
    results: &[AcceptanceCheckResult],
) -> std::io::Result<PathBuf> {
    let dir = acceptance_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not resolve ~/.fleet directory",
        )
    })?;
    std::fs::create_dir_all(&dir)?;
    let safe_task = sanitize(task_id);
    let safe_p = sanitize(p_item_id);
    let final_path = dir.join(format!("{safe_task}__{safe_p}.json"));
    let tmp_path = dir.join(format!(".{safe_task}__{safe_p}.json.tmp"));
    let json = serde_json::to_string_pretty(results)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Load a previously persisted batch of results.
pub fn load_results(task_id: &str, p_item_id: &str) -> std::io::Result<Vec<AcceptanceCheckResult>> {
    let dir = acceptance_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not resolve ~/.fleet directory",
        )
    })?;
    let path = dir.join(format!("{}__{}.json", sanitize(task_id), sanitize(p_item_id)));
    let s = std::fs::read_to_string(path)?;
    serde_json::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Replace path-separator and other filesystem-unfriendly characters so an id
/// can't escape the acceptance dir or collide with the `__` separator.
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '.' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A workspace with a minimal Cargo project that compiles and has one
    /// passing + one failing test selectable by name.
    fn build_workspace(compiles: bool) -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"accept_fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"accept_fixture\"\npath = \"main.rs\"\n",
        )
        .unwrap();
        let body = if compiles {
            "fn main() {}\n\
             #[test] fn t_ok() { assert_eq!(1+1, 2); }\n\
             #[test] fn t_bad() { assert_eq!(1+1, 3); }\n"
        } else {
            // Missing semicolon / undefined symbol → does not compile.
            "fn main() { this_symbol_does_not_exist() }\n"
        };
        fs::write(root.join("main.rs"), body).unwrap();
        dir
    }

    // ── [REQ-006] individual checks gather real evidence ─────────────────────

    #[test]
    fn check_builds_passes_on_compiling_crate() {
        let ws = build_workspace(true);
        let r = check_builds(ws.path());
        assert!(r.passed, "evidence: {}", r.evidence);
        assert!(!r.needs_human);
        assert_eq!(r.criterion, AcceptanceCriterion::Builds);
        assert!(r.checked_at > 0);
    }

    #[test]
    fn check_builds_fails_on_broken_crate() {
        let ws = build_workspace(false);
        let r = check_builds(ws.path());
        assert!(!r.passed, "broken crate must fail the build check");
        assert!(!r.needs_human);
        assert!(
            r.evidence.contains("cargo build"),
            "evidence should reference the build: {}",
            r.evidence
        );
    }

    #[test]
    fn check_tests_pass_distinguishes_passing_and_failing_commands() {
        let ws = build_workspace(true);
        // Run only the passing test.
        let ok = check_tests_pass(ws.path(), "cargo test t_ok");
        assert!(ok.passed, "evidence: {}", ok.evidence);
        // Run only the failing test.
        let bad = check_tests_pass(ws.path(), "cargo test t_bad");
        assert!(!bad.passed, "a failing test command must not pass");
        match &bad.criterion {
            AcceptanceCriterion::TestsPass(cmd) => assert_eq!(cmd, "cargo test t_bad"),
            other => panic!("wrong criterion: {other:?}"),
        }
    }

    #[test]
    fn check_tests_pass_empty_command_fails() {
        let ws = build_workspace(true);
        let r = check_tests_pass(ws.path(), "   ");
        assert!(!r.passed);
    }

    #[test]
    fn eval_custom_runs_shell_rule() {
        let ws = build_workspace(true);
        let pass = eval_custom(ws.path(), "true");
        assert!(pass.passed, "evidence: {}", pass.evidence);
        let fail = eval_custom(ws.path(), "false");
        assert!(!fail.passed);
    }

    #[test]
    fn check_human_review_never_auto_passes() {
        let r = check_human_review();
        assert!(!r.passed, "HumanReview must never auto-pass");
        assert!(r.needs_human, "HumanReview must defer to a human");
        assert_eq!(r.criterion, AcceptanceCriterion::HumanReview);
    }

    // ── [REQ-006][REQ-050] the all-must-pass rule ────────────────────────────

    /// Declaring `[Builds, TestsPass]` but only Builds passing → Rejected.
    /// This is the spec's named acceptance test for REQ-006/050.
    #[test]
    fn declared_builds_and_tests_only_builds_passing_is_rejected() {
        let ws = build_workspace(true); // builds OK
        let criteria = vec![
            AcceptanceCriterion::Builds,
            AcceptanceCriterion::TestsPass("cargo test t_bad".into()), // fails
        ];
        let report = audit_acceptance(ws.path(), &criteria, false);
        assert_eq!(report.results.len(), 2);
        assert!(report.results[0].passed, "Builds should pass");
        assert!(!report.results[1].passed, "TestsPass should fail");
        assert_eq!(
            report.decision,
            AuditDecision::Rejected,
            "partial pass must reject the whole P-item"
        );
    }

    #[test]
    fn all_machine_criteria_passing_with_no_gate_is_all_passed() {
        let ws = build_workspace(true);
        let criteria = vec![
            AcceptanceCriterion::Builds,
            AcceptanceCriterion::TestsPass("cargo test t_ok".into()),
            AcceptanceCriterion::Custom("true".into()),
        ];
        let report = audit_acceptance(ws.path(), &criteria, false);
        assert!(report.results.iter().all(|r| r.passed));
        assert_eq!(report.decision, AuditDecision::AllPassed);
    }

    #[test]
    fn hard_failure_outranks_human_review() {
        let ws = build_workspace(true);
        // Builds OK, a failing test, AND a HumanReview. The failing test must
        // win → Rejected, not WaitHuman (don't ask a human to bless a red test).
        let criteria = vec![
            AcceptanceCriterion::TestsPass("cargo test t_bad".into()),
            AcceptanceCriterion::HumanReview,
        ];
        let report = audit_acceptance(ws.path(), &criteria, false);
        assert_eq!(report.decision, AuditDecision::Rejected);
    }

    #[test]
    fn human_review_criterion_with_clean_machine_checks_waits_human() {
        let ws = build_workspace(true);
        let criteria = vec![AcceptanceCriterion::Builds, AcceptanceCriterion::HumanReview];
        let report = audit_acceptance(ws.path(), &criteria, false);
        assert_eq!(report.decision, AuditDecision::WaitHuman);
    }

    // ── [REQ-011] human-gate precedence ──────────────────────────────────────

    #[test]
    fn human_gate_precedence_either_side_gates() {
        // Neither set → no gate.
        assert!(!requires_human_gate(false, false));
        // P-item level set → gate.
        assert!(requires_human_gate(true, false));
        // Task (manual_review_all) level set → gate.
        assert!(requires_human_gate(false, true));
        // Both set → gate.
        assert!(requires_human_gate(true, true));
    }

    /// [REQ-011][REQ-050] Even when every machine-checkable criterion passes,
    /// an active gate (P-item OR task level) forces WaitHuman, never Done.
    #[test]
    fn gate_forces_wait_human_even_when_all_machine_checks_pass() {
        let ws = build_workspace(true);
        let criteria = vec![AcceptanceCriterion::Builds];

        // P-item-level gate.
        let gate_p = requires_human_gate(true, false);
        let report_p = audit_acceptance(ws.path(), &criteria, gate_p);
        assert!(report_p.results[0].passed);
        assert_eq!(
            report_p.decision,
            AuditDecision::WaitHuman,
            "P-item human_gate must force WaitHuman"
        );

        // Task-level (manual_review_all) gate, P-item flag false.
        let gate_t = requires_human_gate(false, true);
        let report_t = audit_acceptance(ws.path(), &criteria, gate_t);
        assert_eq!(
            report_t.decision,
            AuditDecision::WaitHuman,
            "task manual_review_all must force WaitHuman"
        );

        // No gate → AllPassed (proves the gate is what flips it, not noise).
        let report_none = audit_acceptance(ws.path(), &criteria, false);
        assert_eq!(report_none.decision, AuditDecision::AllPassed);
    }

    /// A failing criterion under an active gate still Rejects — the gate does
    /// not paper over a hard failure.
    #[test]
    fn gate_does_not_override_hard_failure() {
        let ws = build_workspace(true);
        let criteria = vec![AcceptanceCriterion::TestsPass("cargo test t_bad".into())];
        let report = audit_acceptance(ws.path(), &criteria, true);
        assert_eq!(report.decision, AuditDecision::Rejected);
    }

    // ── [REQ-006] persistence ────────────────────────────────────────────────

    #[test]
    fn results_roundtrip_to_disk_as_json() {
        let _guard = crate::paths::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", home.path());

        let results = vec![
            AcceptanceCheckResult::passed(AcceptanceCriterion::Builds, "ok"),
            AcceptanceCheckResult::failed(
                AcceptanceCriterion::TestsPass("cargo test".into()),
                "exited 101",
            ),
            AcceptanceCheckResult::deferred_to_human(
                AcceptanceCriterion::HumanReview,
                "ask the user",
            ),
        ];
        let path = persist_results("task/../weird id", "p1", &results).unwrap();
        assert!(path.exists(), "json file should be written");
        // Sanitized id must not have escaped the acceptance dir.
        assert!(path.starts_with(home.path().join(".fleet").join("acceptance")));

        let loaded = load_results("task/../weird id", "p1").unwrap();
        assert_eq!(loaded, results, "persisted results must round-trip");
        // The deferred-human flag survives serialization.
        assert!(loaded[2].needs_human);

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
    }
}
