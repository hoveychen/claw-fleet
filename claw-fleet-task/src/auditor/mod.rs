//! Deterministic adversarial-rule layer (methodology pillar 4).
//!
//! Per WA-DEC the Auditor is **no longer an LLM session** that votes on
//! findings — that 3-session 2-of-3 quorum was over-designed. What survives is
//! the part that always carried the value: a set of **deterministic rules** that
//! inspect a mark-done decision and a P-item plan diff and emit structured
//! [`AuditFinding`]s with zero model calls, zero sessions, zero timeouts, and
//! zero voting. The mark-done hard gate (`crate::actions::mark_done`) runs these
//! rules in-process; any red-line / `Critical` finding deterministically rejects
//! the mark-done. The `fleet task audit` CLI runs the same rules as a read-only
//! preview.
//!
//! This module is therefore the **pure data + rule layer**: it models the
//! structured findings, runs the proxy-signal check ([`audit_mark_done`]) and
//! the plan-shape weak-implementation heuristics ([`audit_pitem_diff`]). The
//! 10 red lines and 4 weak patterns are encoded as [`Category`] so the rule
//! code and tests can't typo a tag. See design/tasks-req-registry.yaml
//! (REQ-003, REQ-028, REQ-043).

use serde::{Deserialize, Serialize};

use crate::pitem::{AcceptanceCriterion, PItemId};

// ── Structured finding ───────────────────────────────────────────────────────

/// Severity of an audit finding, aligned with the deviation ledger's
/// `RiskLevel` and `audit-patterns.json`'s tiers. Red-line violations are always
/// [`Severity::Critical`]; weak-implementation heuristics map to High/Medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Medium,
    High,
    Critical,
}

/// One structured violation report a deterministic rule emits.
///
/// The required JSON shape `{severity, category, finding, evidence,
/// req_affected}`. Serialized as camelCase (`reqAffected`) so it matches the
/// wire shape the supervisor / UI consume, and so it lines up with
/// `claw_fleet_core::audit::AuditEvent`'s camelCase convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFinding {
    pub severity: Severity,
    /// A red-line tag (`no-proxy-signal`, …) or a weak-pattern tag
    /// (`declared-vs-reality`, …). See [`Category`].
    pub category: String,
    /// One-line statement of what was violated.
    pub finding: String,
    /// Concrete evidence quoted from the mark-done record / plan diff. Never a
    /// proxy signal, never a paraphrase.
    pub evidence: String,
    /// REQ ids this finding implicates, for reverse-lookup against the registry.
    pub req_affected: Vec<String>,
}

/// The canonical category tags. Red lines (1..=10) → always Critical; weak
/// patterns (A..=D) → High/Medium. Kept as an enum so the rule code and
/// tests can't typo a tag string; [`Category::tag`] is the on-the-wire value
/// that goes into [`AuditFinding::category`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    // ── 10 red lines — all Critical ──
    NoDirectEdit,
    NoSkipAudit,
    NoPauseResumeClear,
    NoProxySignal,
    NoDirectGit,
    NoWorkerSessionRw,
    NoConfigMutation,
    NoNewTask,
    NoWorkerFileWrite,
    MergeOnlyViaPMerge,
    // ── 4 weak-implementation heuristics ──
    DeclaredVsReality,
    AcceptanceStrengthRegression,
    TouchesScopeShrink,
    DependencyChainBreak,
}

impl Category {
    /// The wire tag written into [`AuditFinding::category`].
    pub fn tag(self) -> &'static str {
        match self {
            Category::NoDirectEdit => "no-direct-edit",
            Category::NoSkipAudit => "no-skip-audit",
            Category::NoPauseResumeClear => "no-pause-resume-clear",
            Category::NoProxySignal => "no-proxy-signal",
            Category::NoDirectGit => "no-direct-git",
            Category::NoWorkerSessionRw => "no-worker-session-rw",
            Category::NoConfigMutation => "no-config-mutation",
            Category::NoNewTask => "no-new-task",
            Category::NoWorkerFileWrite => "no-worker-file-write",
            Category::MergeOnlyViaPMerge => "merge-only-via-p-merge",
            Category::DeclaredVsReality => "declared-vs-reality",
            Category::AcceptanceStrengthRegression => "acceptance-strength-regression",
            Category::TouchesScopeShrink => "touches-scope-shrink",
            Category::DependencyChainBreak => "dependency-chain-break",
        }
    }

    /// Whether this category is one of the 10 red lines (always Critical).
    pub fn is_red_line(self) -> bool {
        matches!(
            self,
            Category::NoDirectEdit
                | Category::NoSkipAudit
                | Category::NoPauseResumeClear
                | Category::NoProxySignal
                | Category::NoDirectGit
                | Category::NoWorkerSessionRw
                | Category::NoConfigMutation
                | Category::NoNewTask
                | Category::NoWorkerFileWrite
                | Category::MergeOnlyViaPMerge
        )
    }

    /// The 10 red lines, in red-line order (1..=10).
    pub fn red_lines() -> [Category; 10] {
        [
            Category::NoDirectEdit,
            Category::NoSkipAudit,
            Category::NoPauseResumeClear,
            Category::NoProxySignal,
            Category::NoDirectGit,
            Category::NoWorkerSessionRw,
            Category::NoConfigMutation,
            Category::NoNewTask,
            Category::NoWorkerFileWrite,
            Category::MergeOnlyViaPMerge,
        ]
    }

    /// The 4 weak-implementation heuristics.
    pub fn weak_patterns() -> [Category; 4] {
        [
            Category::DeclaredVsReality,
            Category::AcceptanceStrengthRegression,
            Category::TouchesScopeShrink,
            Category::DependencyChainBreak,
        ]
    }
}

// ── mark-done evidence model + proxy-signal detection (REQ-043) ───────────────

/// One piece of "evidence" the Master attached to a mark-done decision. The
/// Auditor classifies each: a [`Self::is_proxy`] piece (worker self-report,
/// token count, elapsed time, diff size) is NOT real acceptance evidence.
///
/// [REQ-043] This is the data shape the Auditor reconstructs from the Master's
/// decision log to decide whether mark-done rested on real evidence or proxies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkDoneEvidence {
    /// `cargo build`/`check` exit code recorded. Real evidence for `Builds`.
    BuildExit { code: i32, tail: String },
    /// A test command + its exit code. Real evidence for `TestsPass`.
    TestExit { cmd: String, code: i32, tail: String },
    /// A human approval recorded. Real evidence for `HumanReview`.
    HumanApproval { approver: String },
    /// A concrete judgement against a custom rule. Real evidence for `Custom`.
    CustomJudgement { rule: String, note: String },
    // ── proxy signals — NEVER acceptance evidence (red line 4) ──
    /// "the worker said it was done."
    WorkerSelfReport { text: String },
    /// "the worker burned N tokens."
    TokenCount { tokens: u64 },
    /// "the worker ran for N seconds."
    ElapsedTime { secs: u64 },
    /// "the diff was N lines."
    DiffSize { lines: u64 },
}

impl MarkDoneEvidence {
    /// `true` iff this is a forbidden proxy signal (worker self-report, token
    /// count, elapsed time, diff size). [REQ-003][REQ-043]
    pub fn is_proxy(&self) -> bool {
        matches!(
            self,
            MarkDoneEvidence::WorkerSelfReport { .. }
                | MarkDoneEvidence::TokenCount { .. }
                | MarkDoneEvidence::ElapsedTime { .. }
                | MarkDoneEvidence::DiffSize { .. }
        )
    }

    /// Which `AcceptanceCriterion` (if any) this piece of evidence substantiates
    /// — only when the underlying check actually *passed*. A non-zero exit code
    /// substantiates nothing (a red test is not evidence the criterion is met).
    fn substantiates(&self, criterion: &AcceptanceCriterion) -> bool {
        match (self, criterion) {
            (MarkDoneEvidence::BuildExit { code, .. }, AcceptanceCriterion::Builds) => *code == 0,
            (
                MarkDoneEvidence::TestExit { cmd, code, .. },
                AcceptanceCriterion::TestsPass(want),
            ) => *code == 0 && cmd == want,
            (MarkDoneEvidence::HumanApproval { .. }, AcceptanceCriterion::HumanReview) => true,
            (
                MarkDoneEvidence::CustomJudgement { rule, .. },
                AcceptanceCriterion::Custom(want),
            ) => rule == want,
            _ => false,
        }
    }
}

/// A mark-done decision the Master made, as the Auditor reconstructs it from the
/// decision log: which P-item, what it declared, and the evidence the Master
/// attached. [REQ-043]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkDoneRecord {
    pub p_item_id: PItemId,
    pub declared: Vec<AcceptanceCriterion>,
    pub evidence: Vec<MarkDoneEvidence>,
}

/// [REQ-043] Audit a single mark-done decision. Returns the findings the Auditor
/// would emit (empty when the decision is clean).
///
/// Two failure modes are detected:
/// 1. **Proxy-only mark-done** — the Master attached *only* proxy signals (or no
///    real evidence at all). This is a red-line-4 violation → one `Critical`
///    `no-proxy-signal` finding (REQ-003/REQ-043).
/// 2. **Declared-but-unverified** — a declared criterion has no real
///    substantiating evidence even though *some* real evidence exists for other
///    criteria. This is the weak-implementation "declared vs reality" pattern →
///    one `High` `declared-vs-reality` finding per missing criterion (REQ-028).
///
/// The two are kept distinct on purpose: "the Master leaned entirely on proxies"
/// is a constitutional red line, while "one of several criteria slipped through
/// unverified" is a weakness report. When *no* real evidence exists at all the
/// proxy-only red line fires (it dominates), and per-criterion declared-vs-reality
/// findings are suppressed to avoid double-counting the same rot.
pub fn audit_mark_done(record: &MarkDoneRecord) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    let has_any_real = record.evidence.iter().any(|e| !e.is_proxy());
    let has_any_proxy = record.evidence.iter().any(|e| e.is_proxy());

    if !has_any_real {
        // Red line 4: mark-done rested on nothing but proxy signals (or nothing
        // at all). [REQ-003][REQ-043]
        let cited: Vec<String> = record
            .evidence
            .iter()
            .filter(|e| e.is_proxy())
            .map(describe_proxy)
            .collect();
        let evidence = if cited.is_empty() {
            format!(
                "mark-done on P-item `{}` attached NO acceptance evidence at all — \
                 declared {:?} but the Master log carries no real verification",
                record.p_item_id, record.declared
            )
        } else {
            format!(
                "mark-done on P-item `{}` rested ONLY on proxy signals: [{}]. \
                 Proxy signals are not acceptance evidence (worker is the被审计方; \
                 tokens/elapsed/diff-size are uncorrelated with correctness).",
                record.p_item_id,
                cited.join("; ")
            )
        };
        findings.push(AuditFinding {
            severity: Severity::Critical,
            category: Category::NoProxySignal.tag().to_string(),
            finding: format!(
                "Master marked P-item `{}` Done using proxy signals instead of real \
                 acceptance evidence",
                record.p_item_id
            ),
            evidence,
            req_affected: vec!["REQ-003".to_string(), "REQ-043".to_string()],
        });
        return findings; // red line dominates; don't double-report per criterion
    }

    // Some real evidence exists. Now check each declared criterion is actually
    // substantiated; any that isn't is a declared-vs-reality weakness. [REQ-028]
    for criterion in &record.declared {
        let substantiated = record.evidence.iter().any(|e| e.substantiates(criterion));
        if !substantiated {
            findings.push(AuditFinding {
                severity: Severity::High,
                category: Category::DeclaredVsReality.tag().to_string(),
                finding: format!(
                    "declared acceptance criterion {:?} on P-item `{}` has no real \
                     substantiating evidence in the mark-done record",
                    criterion, record.p_item_id
                ),
                evidence: format!(
                    "declared {:?}; mark-done evidence present: {:?} — no entry \
                     substantiates {:?}",
                    record.declared,
                    record.evidence.iter().map(short_evidence).collect::<Vec<_>>(),
                    criterion
                ),
                req_affected: vec![
                    "REQ-004".to_string(),
                    "REQ-006".to_string(),
                    "REQ-028".to_string(),
                ],
            });
        }
    }

    // A leftover proxy alongside real evidence isn't itself a violation (the
    // Master may log tokens for telemetry); we only flag proxies when they are
    // *standing in for* missing real evidence, handled above.
    let _ = has_any_proxy;

    findings
}

fn describe_proxy(e: &MarkDoneEvidence) -> String {
    match e {
        MarkDoneEvidence::WorkerSelfReport { text } => format!("worker self-report: {text:?}"),
        MarkDoneEvidence::TokenCount { tokens } => format!("token count = {tokens}"),
        MarkDoneEvidence::ElapsedTime { secs } => format!("elapsed = {secs}s"),
        MarkDoneEvidence::DiffSize { lines } => format!("diff size = {lines} lines"),
        _ => "real evidence".to_string(),
    }
}

fn short_evidence(e: &MarkDoneEvidence) -> String {
    match e {
        MarkDoneEvidence::BuildExit { code, .. } => format!("BuildExit({code})"),
        MarkDoneEvidence::TestExit { cmd, code, .. } => format!("TestExit({cmd:?},{code})"),
        MarkDoneEvidence::HumanApproval { approver } => format!("HumanApproval({approver:?})"),
        MarkDoneEvidence::CustomJudgement { rule, .. } => format!("CustomJudgement({rule:?})"),
        other => describe_proxy(other),
    }
}

// ── weak-implementation heuristics (REQ-028) ─────────────────────────────────

/// Before/after view of a P-item's plan-relevant shape, so the Auditor can spot
/// a P-item being silently weakened across an `update-plan`. [REQ-028]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PItemDiff {
    pub p_item_id: PItemId,
    /// `desc` text after the update — used to spot touches that no longer cover
    /// the files the desc says must change.
    pub desc: String,
    pub acceptance_before: Vec<AcceptanceCriterion>,
    pub acceptance_after: Vec<AcceptanceCriterion>,
    pub touches_before: Vec<String>,
    pub touches_after: Vec<String>,
    pub depends_on_before: Vec<PItemId>,
    pub depends_on_after: Vec<PItemId>,
    /// Files the `desc` explicitly says this P-item must produce/edit. The
    /// supervisor extracts these (e.g. backtick-quoted paths in the desc); the
    /// Auditor checks they remain covered by `touches_after`.
    pub desc_required_files: Vec<String>,
    /// `true` when the supervisor already recorded a deviation-ledger entry
    /// covering this weakening — an *approved* weakening is not a finding.
    pub has_deviation_entry: bool,
}

/// [REQ-028] Run the 3 plan-shape weak-implementation heuristics over a P-item
/// update (the 4th, declared-vs-reality, is folded into [`audit_mark_done`]).
/// Reports — never fixes:
///
/// - **acceptance-strength-regression** (High): fewer acceptance criteria after
///   the update (and no deviation entry blessing it).
/// - **touches-scope-shrink** (Medium): a file the `desc` requires is no longer
///   in `touches_after`.
/// - **dependency-chain-break** (High): a `depends_on` edge was dropped.
pub fn audit_pitem_diff(diff: &PItemDiff) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    // B. acceptance-strength-regression — acceptance got shorter without an
    //    approved deviation. [REQ-028]
    if diff.acceptance_after.len() < diff.acceptance_before.len() && !diff.has_deviation_entry {
        findings.push(AuditFinding {
            severity: Severity::High,
            category: Category::AcceptanceStrengthRegression.tag().to_string(),
            finding: format!(
                "acceptance for P-item `{}` was weakened ({} → {} criteria) with no \
                 deviation-ledger entry",
                diff.p_item_id,
                diff.acceptance_before.len(),
                diff.acceptance_after.len()
            ),
            evidence: format!(
                "before: {:?}; after: {:?}",
                diff.acceptance_before, diff.acceptance_after
            ),
            req_affected: vec!["REQ-028".to_string()],
        });
    }

    // C. touches-scope-shrink — a desc-required file fell out of touches. [REQ-028]
    for required in &diff.desc_required_files {
        let covered = diff
            .touches_after
            .iter()
            .any(|t| required == t || required.starts_with(t.as_str()) || t.starts_with(required.as_str()));
        if !covered {
            findings.push(AuditFinding {
                severity: Severity::Medium,
                category: Category::TouchesScopeShrink.tag().to_string(),
                finding: format!(
                    "P-item `{}` desc requires editing `{}` but it is not covered by \
                     touches",
                    diff.p_item_id, required
                ),
                evidence: format!(
                    "desc requires {:?}; touches after update: {:?}",
                    diff.desc_required_files, diff.touches_after
                ),
                req_affected: vec!["REQ-028".to_string()],
            });
        }
    }

    // D. dependency-chain-break — a depends_on edge was dropped. [REQ-028]
    for dep in &diff.depends_on_before {
        if !diff.depends_on_after.contains(dep) {
            findings.push(AuditFinding {
                severity: Severity::High,
                category: Category::DependencyChainBreak.tag().to_string(),
                finding: format!(
                    "P-item `{}` lost dependency edge `{}` — its upstream output_summary \
                     will not reach this P-item's Layer-3 context",
                    diff.p_item_id, dep
                ),
                evidence: format!(
                    "depends_on before: {:?}; after: {:?}",
                    diff.depends_on_before, diff.depends_on_after
                ),
                req_affected: vec!["REQ-028".to_string()],
            });
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tests_pass(cmd: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::TestsPass(cmd.to_string())
    }

    // ── red lines + weak patterns are the canonical 10 + 4 ───────────────────

    /// The two canonical category sets stay 10 + 4, all red lines classify as
    /// red lines, and weak patterns do not.
    #[test]
    fn red_lines_and_weak_patterns_are_canonical_sets() {
        assert_eq!(Category::red_lines().len(), 10);
        assert_eq!(Category::weak_patterns().len(), 4);
        for rl in Category::red_lines() {
            assert!(rl.is_red_line(), "red line `{}` must classify as red", rl.tag());
        }
        for wp in Category::weak_patterns() {
            assert!(!wp.is_red_line(), "weak pattern `{}` must not be a red line", wp.tag());
        }
    }

    // ── [REQ-043] proxy-signal detection at mark-done ────────────────────────

    /// The spec's named acceptance test: a mock master mark-done carrying ONLY a
    /// proxy signal (token count) → the Auditor reports `critical`.
    #[test]
    fn req043_mark_done_with_only_proxy_evidence_is_critical() {
        let record = MarkDoneRecord {
            p_item_id: "p1".into(),
            declared: vec![AcceptanceCriterion::Builds, tests_pass("cargo test")],
            evidence: vec![
                MarkDoneEvidence::TokenCount { tokens: 50_000 },
                MarkDoneEvidence::ElapsedTime { secs: 120 },
                MarkDoneEvidence::WorkerSelfReport {
                    text: "I finished it".into(),
                },
            ],
        };
        let findings = audit_mark_done(&record);
        assert_eq!(findings.len(), 1, "proxy-only must yield exactly one finding");
        let f = &findings[0];
        assert_eq!(f.severity, Severity::Critical);
        assert_eq!(f.category, Category::NoProxySignal.tag());
        assert!(f.req_affected.contains(&"REQ-003".to_string()));
        assert!(f.req_affected.contains(&"REQ-043".to_string()));
        // Evidence quotes the actual proxy signals, not a paraphrase.
        assert!(f.evidence.contains("token count = 50000"));
        assert!(f.evidence.contains("elapsed = 120s"));
    }

    /// No evidence at all is also a proxy-style red line (nothing real backs it).
    #[test]
    fn req043_mark_done_with_no_evidence_is_critical() {
        let record = MarkDoneRecord {
            p_item_id: "p2".into(),
            declared: vec![AcceptanceCriterion::Builds],
            evidence: vec![],
        };
        let findings = audit_mark_done(&record);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[0].category, Category::NoProxySignal.tag());
        assert!(findings[0].evidence.contains("NO acceptance evidence"));
    }

    /// Real, passing evidence for every declared criterion → clean, no findings.
    /// Telemetry proxies riding alongside real evidence don't trip the auditor.
    #[test]
    fn req043_mark_done_with_real_passing_evidence_is_clean() {
        let record = MarkDoneRecord {
            p_item_id: "p3".into(),
            declared: vec![AcceptanceCriterion::Builds, tests_pass("cargo test")],
            evidence: vec![
                MarkDoneEvidence::BuildExit {
                    code: 0,
                    tail: "Finished".into(),
                },
                MarkDoneEvidence::TestExit {
                    cmd: "cargo test".into(),
                    code: 0,
                    tail: "test result: ok".into(),
                },
                // telemetry proxy alongside real evidence — allowed.
                MarkDoneEvidence::TokenCount { tokens: 1234 },
            ],
        };
        assert!(audit_mark_done(&record).is_empty(), "clean mark-done");
    }

    /// [REQ-028] declared-vs-reality: real evidence exists for Builds, but the
    /// declared TestsPass has no substantiating evidence → one High finding.
    #[test]
    fn req028_declared_criterion_without_evidence_is_declared_vs_reality() {
        let record = MarkDoneRecord {
            p_item_id: "p4".into(),
            declared: vec![AcceptanceCriterion::Builds, tests_pass("cargo test")],
            evidence: vec![MarkDoneEvidence::BuildExit {
                code: 0,
                tail: "ok".into(),
            }],
        };
        let findings = audit_mark_done(&record);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].category, Category::DeclaredVsReality.tag());
        assert!(findings[0].req_affected.contains(&"REQ-028".to_string()));
    }

    /// A failing test (exit != 0) does NOT substantiate TestsPass — a red test
    /// is not evidence the criterion is met.
    #[test]
    fn req043_failing_test_does_not_substantiate_tests_pass() {
        let record = MarkDoneRecord {
            p_item_id: "p5".into(),
            declared: vec![tests_pass("cargo test")],
            evidence: vec![
                MarkDoneEvidence::BuildExit { code: 0, tail: "".into() },
                MarkDoneEvidence::TestExit {
                    cmd: "cargo test".into(),
                    code: 101,
                    tail: "FAILED".into(),
                },
            ],
        };
        let findings = audit_mark_done(&record);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, Category::DeclaredVsReality.tag());
    }

    // ── [REQ-028] plan-shape weak heuristics ─────────────────────────────────

    fn base_diff() -> PItemDiff {
        PItemDiff {
            p_item_id: "p1".into(),
            desc: "implement X".into(),
            acceptance_before: vec![AcceptanceCriterion::Builds, tests_pass("cargo test")],
            acceptance_after: vec![AcceptanceCriterion::Builds, tests_pass("cargo test")],
            touches_before: vec!["src/a.rs".into()],
            touches_after: vec!["src/a.rs".into()],
            depends_on_before: vec!["p0".into()],
            depends_on_after: vec!["p0".into()],
            desc_required_files: vec![],
            has_deviation_entry: false,
        }
    }

    #[test]
    fn req028_acceptance_strength_regression_detected() {
        let mut d = base_diff();
        d.acceptance_after = vec![AcceptanceCriterion::Builds]; // dropped TestsPass
        let findings = audit_pitem_diff(&d);
        assert!(findings
            .iter()
            .any(|f| f.category == Category::AcceptanceStrengthRegression.tag()
                && f.severity == Severity::High));
    }

    #[test]
    fn req028_acceptance_regression_with_approved_deviation_is_silent() {
        let mut d = base_diff();
        d.acceptance_after = vec![AcceptanceCriterion::Builds];
        d.has_deviation_entry = true; // master recorded + approved the weakening
        let findings = audit_pitem_diff(&d);
        assert!(
            !findings
                .iter()
                .any(|f| f.category == Category::AcceptanceStrengthRegression.tag()),
            "an approved deviation must suppress the regression finding"
        );
    }

    #[test]
    fn req028_touches_scope_shrink_detected() {
        let mut d = base_diff();
        d.desc_required_files = vec!["src/a.rs".into(), "src/b.rs".into()];
        d.touches_after = vec!["src/a.rs".into()]; // b.rs no longer covered
        let findings = audit_pitem_diff(&d);
        let f = findings
            .iter()
            .find(|f| f.category == Category::TouchesScopeShrink.tag())
            .expect("must flag the uncovered desc-required file");
        assert_eq!(f.severity, Severity::Medium);
        assert!(f.evidence.contains("src/b.rs") || f.finding.contains("src/b.rs"));
    }

    #[test]
    fn req028_dependency_chain_break_detected() {
        let mut d = base_diff();
        d.depends_on_after = vec![]; // dropped the edge to p0
        let findings = audit_pitem_diff(&d);
        assert!(findings
            .iter()
            .any(|f| f.category == Category::DependencyChainBreak.tag()
                && f.severity == Severity::High));
    }

    #[test]
    fn req028_clean_diff_yields_no_findings() {
        assert!(audit_pitem_diff(&base_diff()).is_empty());
    }
}
