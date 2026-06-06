//! Independent adversarial Auditor agent (methodology pillar 4).
//!
//! Scaffold created by master before Wave 3 so P10 can fill this without
//! racing P15 on `lib.rs`. P10 implements: independent auditor session,
//! 10 red lines, weak-implementation heuristics, AuditFinding, 2-of-3 voting.
//! See design/tasks-impl-plan.md (P10) and design/tasks-req-registry.yaml
//! (REQ-022, REQ-028, REQ-033, REQ-034, REQ-043).
//!
//! ## Why an independent session
//!
//! The "don't trust the model" methodology forbids the Master from auditing
//! itself — a model marking its own work `Done` is the被审计方 reporting on
//! itself. So the Auditor runs as its **own** Claude session ([REQ-022]) with
//! its own SYSTEM prompt ([`AUDITOR_SYSTEM_PROMPT`], `include_str!`-embedded at
//! compile time so it cannot be tampered with at runtime), spawned by the same
//! supervisor that spawns workers but pointed at the Master's decision log
//! rather than at code. This module is the **pure data + decision layer** for
//! that session, mirroring how [`crate::worker`] is the pure layer behind a
//! worker subprocess: it builds the spawn spec, models the structured findings
//! the Auditor emits, runs the proxy-signal / weak-implementation checks, and
//! tallies the 2-of-3 vote. The actual subprocess spawn and the routing of
//! confirmed findings into the deviation ledger live at the supervisor /
//! `claw-fleet-core` boundary (see the deviation note in the P10 report).

use serde::{Deserialize, Serialize};

use crate::pitem::{AcceptanceCriterion, PItemId};

/// The Auditor's SYSTEM prompt, embedded at compile time so it cannot be
/// swapped out at runtime (same tamper-resistance contract as the Master
/// template's `include_str!`). Lists the 10 red lines and the 4
/// weak-implementation heuristics the Auditor must check.
///
/// [REQ-022] independent system prompt with 10 red lines.
/// [REQ-028] the 4 weak-implementation heuristics.
pub const AUDITOR_SYSTEM_PROMPT: &str = include_str!("system_prompt.md");

/// Model the Auditor session runs on. Opus-class on purpose: an adversarial
/// auditor that is *weaker* than the Master it audits would be easy to fool, so
/// the Auditor gets the same tier as the Master (patch §5 reserves Opus for the
/// master; the auditor shares that tier precisely because it must out-reason the
/// thing it polices).
pub const AUDITOR_MODEL: &str = "claude-opus-4-8";

/// How many independent Auditor sessions vote on each batch of findings.
///
/// [REQ-022][REQ-033] Three independent sessions; a finding is confirmed only
/// when at least [`VOTE_THRESHOLD`] of them report the same problem.
pub const AUDITOR_QUORUM: usize = 3;

/// Votes required (out of [`AUDITOR_QUORUM`]) to confirm a finding. 2-of-3.
///
/// [REQ-022][REQ-033][REQ-034] The 2/3 threshold: a single session's finding is
/// treated as noise; two independent agreeing sessions confirm it.
pub const VOTE_THRESHOLD: usize = 2;

// ── Independent spawn spec (REQ-022) ─────────────────────────────────────────

/// What the supervisor needs to spawn **one** independent Auditor session.
///
/// [REQ-022] The Auditor is independent of the Master: its own spawn spec, its
/// own (compile-time-embedded) SYSTEM prompt, and — critically — it is *not*
/// pointed at the codebase the way a worker is; `master_log_path` is the Master
/// decision log it audits. `session_index` distinguishes the three quorum
/// members so they don't collide on session files. Mirrors
/// [`crate::worker::WorkerSpawnSpec`]'s "pure spec, supervisor does the spawn"
/// shape, with `Serialize`/`Deserialize` so it can cross the supervisor boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditorSpawnSpec {
    pub task_id: String,
    /// 0-based index of this session within the 3-session quorum.
    pub session_index: usize,
    /// The Master decision log this Auditor reads (it never reads worker code
    /// directly — it audits the Master's *decisions*, not the diffs).
    pub master_log_path: String,
    /// The compile-time-embedded Auditor SYSTEM prompt.
    pub system_prompt: String,
    pub model: String,
}

/// Build the 3 independent [`AuditorSpawnSpec`]s for a task's audit quorum.
///
/// [REQ-022][REQ-033] Exactly [`AUDITOR_QUORUM`] specs, each carrying the same
/// embedded SYSTEM prompt but a distinct `session_index` so the three sessions
/// are genuinely separate processes (separate session files, separate model
/// context) — that independence is what makes the 2-of-3 vote meaningful.
pub fn auditor_quorum_specs(task_id: &str, master_log_path: &str) -> Vec<AuditorSpawnSpec> {
    (0..AUDITOR_QUORUM)
        .map(|i| AuditorSpawnSpec {
            task_id: task_id.to_string(),
            session_index: i,
            master_log_path: master_log_path.to_string(),
            system_prompt: AUDITOR_SYSTEM_PROMPT.to_string(),
            model: AUDITOR_MODEL.to_string(),
        })
        .collect()
}

// ── Structured finding (REQ-022 / REQ-034) ───────────────────────────────────

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

/// One structured violation report the Auditor emits.
///
/// [REQ-022] The required JSON shape `{severity, category, finding, evidence,
/// req_affected}`. Serialized as camelCase (`reqAffected`) so it matches the
/// wire shape the supervisor / UI consume, and so it lines up with
/// `claw_fleet_core::audit::AuditEvent`'s camelCase convention.
///
/// [REQ-034] alignment: the runtime *security* audit event
/// (`claw_fleet_core::audit::AuditEvent` with `session_id|timestamp|tool_name`
/// dedup) is a sibling record produced by the guard layer; an `AuditFinding`
/// reuses the same dedup discipline via [`AuditFinding::dedup_key`] so the UI
/// can de-duplicate repeated findings across the quorum's three sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFinding {
    pub severity: Severity,
    /// A red-line tag (`no-proxy-signal`, …) or a weak-pattern tag
    /// (`declared-vs-reality`, …). See [`Category`].
    pub category: String,
    /// One-line statement of what was violated.
    pub finding: String,
    /// Concrete evidence quoted from the Master log / worker output. Never a
    /// proxy signal, never the Auditor's own paraphrase.
    pub evidence: String,
    /// REQ ids this finding implicates, for reverse-lookup against the registry.
    pub req_affected: Vec<String>,
}

impl AuditFinding {
    /// A stable dedup key for collapsing the same finding reported by multiple
    /// quorum sessions, mirroring `claw_fleet_core::audit::AuditEvent::dedup_key`.
    /// Two findings are "the same problem" iff they share category + the sorted
    /// set of affected REQs — exactly the equivalence the 2-of-3 vote uses.
    ///
    /// [REQ-034] dedup discipline so the UI surfaces one notification per real
    /// problem rather than three (one per quorum member).
    pub fn dedup_key(&self) -> String {
        let mut reqs = self.req_affected.clone();
        reqs.sort();
        format!("{}|{}", self.category, reqs.join(","))
    }
}

/// The canonical category tags. Red lines (1..=10) → always Critical; weak
/// patterns (A..=D) → High/Medium. Kept as an enum so the heuristic code and
/// tests can't typo a tag string; [`Category::tag`] is the on-the-wire value
/// that goes into [`AuditFinding::category`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    // ── 10 red lines (REQ-022) — all Critical ──
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
    // ── 4 weak-implementation heuristics (REQ-028) ──
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

    /// The 10 red lines, in red-line order (1..=10). Used to prove the SYSTEM
    /// prompt mentions every one of them.
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

// ── 2-of-3 voting (REQ-022 / REQ-033 / REQ-034) ──────────────────────────────

/// A finding confirmed by the quorum vote: the canonical finding plus how many
/// of the [`AUDITOR_QUORUM`] sessions reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedFinding {
    pub finding: AuditFinding,
    pub votes: usize,
}

/// [REQ-022][REQ-033][REQ-034] Tally findings from the 3 independent Auditor
/// sessions and keep only those that reached the [`VOTE_THRESHOLD`] (2-of-3).
///
/// Two findings are "the same problem" iff they share [`AuditFinding::dedup_key`]
/// (category + sorted affected-REQ set) — the same equivalence the UI uses to
/// de-duplicate. Within a single session, repeated findings of the same problem
/// count as **one** vote (a session can't out-vote itself by repeating). The
/// returned list is sorted by votes desc then dedup_key for determinism, and the
/// representative `AuditFinding` is the first one seen for that key.
pub fn tally_votes(per_session: &[Vec<AuditFinding>]) -> Vec<ConfirmedFinding> {
    use std::collections::BTreeMap;

    // key -> (representative finding, set of session indices that reported it)
    let mut tally: BTreeMap<String, (AuditFinding, std::collections::BTreeSet<usize>)> =
        BTreeMap::new();

    for (session_idx, findings) in per_session.iter().enumerate() {
        for f in findings {
            let key = f.dedup_key();
            tally
                .entry(key)
                .or_insert_with(|| (f.clone(), std::collections::BTreeSet::new()))
                .1
                .insert(session_idx);
        }
    }

    let mut confirmed: Vec<ConfirmedFinding> = tally
        .into_iter()
        .filter(|(_, (_, voters))| voters.len() >= VOTE_THRESHOLD)
        .map(|(_, (finding, voters))| ConfirmedFinding {
            finding,
            votes: voters.len(),
        })
        .collect();

    confirmed.sort_by(|a, b| {
        b.votes
            .cmp(&a.votes)
            .then_with(|| a.finding.dedup_key().cmp(&b.finding.dedup_key()))
    });
    confirmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tests_pass(cmd: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::TestsPass(cmd.to_string())
    }

    // ── [REQ-022] independent session + embedded prompt ──────────────────────

    /// The SYSTEM prompt is embedded at compile time and mentions all 10 red
    /// lines (by tag) and all 4 weak-implementation heuristics — proving the
    /// Auditor's constitution travels with the binary, not a runtime file.
    #[test]
    fn req022_system_prompt_lists_all_red_lines_and_weak_patterns() {
        assert!(
            AUDITOR_SYSTEM_PROMPT.contains("Auditor"),
            "prompt must identify the auditor role"
        );
        for rl in Category::red_lines() {
            assert!(
                AUDITOR_SYSTEM_PROMPT.contains(rl.tag()),
                "system prompt must mention red-line tag `{}`",
                rl.tag()
            );
        }
        for wp in Category::weak_patterns() {
            assert!(
                AUDITOR_SYSTEM_PROMPT.contains(wp.tag()),
                "system prompt must mention weak-pattern tag `{}`",
                wp.tag()
            );
        }
        // The 4 proxy signals must be named so the Auditor knows what to reject.
        for proxy in ["自报", "token", "diff"] {
            assert!(
                AUDITOR_SYSTEM_PROMPT.contains(proxy),
                "system prompt must name proxy signal `{proxy}`"
            );
        }
    }

    /// [REQ-022][REQ-033] The quorum is 3 independent specs, each with its own
    /// session_index but the same embedded prompt — independence is what makes
    /// the 2-of-3 vote meaningful.
    #[test]
    fn req022_quorum_is_three_independent_specs() {
        let specs = auditor_quorum_specs("task-1", "/log/master.jsonl");
        assert_eq!(specs.len(), AUDITOR_QUORUM);
        assert_eq!(AUDITOR_QUORUM, 3);
        let idxs: Vec<usize> = specs.iter().map(|s| s.session_index).collect();
        assert_eq!(idxs, vec![0, 1, 2], "sessions must be distinctly indexed");
        for s in &specs {
            assert_eq!(s.system_prompt, AUDITOR_SYSTEM_PROMPT);
            assert_eq!(s.model, AUDITOR_MODEL);
            assert_eq!(s.master_log_path, "/log/master.jsonl");
        }
        // A spec round-trips through JSON (crosses the supervisor boundary).
        let json = serde_json::to_string(&specs[0]).unwrap();
        let back: AuditorSpawnSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, specs[0]);
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

    // ── [REQ-022][REQ-033][REQ-034] 2-of-3 voting ────────────────────────────

    fn finding(cat: Category, reqs: &[&str]) -> AuditFinding {
        AuditFinding {
            severity: if cat.is_red_line() {
                Severity::Critical
            } else {
                Severity::High
            },
            category: cat.tag().to_string(),
            finding: "f".into(),
            evidence: "e".into(),
            req_affected: reqs.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 2-of-3: a finding reported by 2 sessions is confirmed; one reported by
    /// only 1 is dropped as noise.
    #[test]
    fn req033_two_of_three_confirms_majority_drops_singletons() {
        let s0 = vec![
            finding(Category::NoProxySignal, &["REQ-003", "REQ-043"]),
            finding(Category::TouchesScopeShrink, &["REQ-028"]),
        ];
        let s1 = vec![finding(Category::NoProxySignal, &["REQ-043", "REQ-003"])]; // same key, reordered reqs
        let s2 = vec![finding(Category::DependencyChainBreak, &["REQ-028"])]; // singleton

        let confirmed = tally_votes(&[s0, s1, s2]);
        // no-proxy-signal: 2 votes → confirmed. touches-scope-shrink: 1 → dropped.
        // dependency-chain-break: 1 → dropped.
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].finding.category, Category::NoProxySignal.tag());
        assert_eq!(confirmed[0].votes, 2);
    }

    /// All three agree → confirmed with 3 votes.
    #[test]
    fn req033_unanimous_finding_confirmed_with_three_votes() {
        let f = finding(Category::NoDirectGit, &["REQ-021"]);
        let confirmed = tally_votes(&[vec![f.clone()], vec![f.clone()], vec![f.clone()]]);
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].votes, 3);
    }

    /// A single session repeating the same finding 3× is still only 1 vote — a
    /// session can't out-vote itself.
    #[test]
    fn req033_repeated_finding_in_one_session_counts_once() {
        let f = finding(Category::NoProxySignal, &["REQ-043"]);
        let confirmed = tally_votes(&[vec![f.clone(), f.clone(), f.clone()], vec![], vec![]]);
        assert!(
            confirmed.is_empty(),
            "1 session (even repeating) is below the 2/3 threshold"
        );
    }

    /// Mock master that ONLY reports proxy signals, fed end-to-end through the
    /// quorum: 2 of 3 auditors catch it → confirmed critical. This is the spec's
    /// "mock master 仅代理信号 → Auditor 报 critical" acceptance test.
    #[test]
    fn req034_mock_proxy_only_master_confirmed_critical_by_quorum() {
        let record = MarkDoneRecord {
            p_item_id: "p9".into(),
            declared: vec![AcceptanceCriterion::Builds],
            evidence: vec![MarkDoneEvidence::WorkerSelfReport {
                text: "done!".into(),
            }],
        };
        // Two independent auditor sessions both run the same audit and agree;
        // the third (mock) missed it.
        let a0 = audit_mark_done(&record);
        let a1 = audit_mark_done(&record);
        let a2: Vec<AuditFinding> = vec![];
        assert_eq!(a0[0].severity, Severity::Critical);

        let confirmed = tally_votes(&[a0, a1, a2]);
        assert_eq!(confirmed.len(), 1, "proxy-only mark-done must be confirmed");
        assert_eq!(confirmed[0].finding.severity, Severity::Critical);
        assert_eq!(
            confirmed[0].finding.category,
            Category::NoProxySignal.tag()
        );
        assert_eq!(confirmed[0].votes, 2, "2-of-3 confirms it");
    }

    #[test]
    fn dedup_key_is_category_plus_sorted_reqs() {
        let a = finding(Category::NoProxySignal, &["REQ-043", "REQ-003"]);
        let b = finding(Category::NoProxySignal, &["REQ-003", "REQ-043"]);
        assert_eq!(a.dedup_key(), b.dedup_key(), "req order must not matter");
        let c = finding(Category::DeclaredVsReality, &["REQ-028"]);
        assert_ne!(a.dedup_key(), c.dedup_key());
    }
}
