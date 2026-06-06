//! Business actions on `Task` — the side-effectful operations that
//! coordinate master/worker spawn, worktree merge, and the host-provided
//! subprocess + LLM hooks. Pure data + persistence stays in `crate::task`.
//!
//! Migrated from `claw_fleet_core::task_actions` so the standalone
//! `fleet-task` binary can reuse the exact same lifecycle logic. The
//! callbacks the actions used to make into `crate::project` /
//! `crate::supervisor` / `crate::merge_mediator` are now expressed via the
//! `TaskHost` trait (`TaskLifecycleHost + LlmMediator`).
//!
//! `claw_fleet_core::task_actions` keeps thin wrappers around these so the
//! existing `claw_fleet_core::task::start_task(&id)` import path continues
//! to work — those wrappers supply a `SupervisorHost` host transparently.
//!
//! `reconcile_task_completion` did **not** migrate: it's tied to the legacy
//! `fleet-sessions.json` table that fleet-task explicitly does not use.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::acceptance::{self, AuditDecision};
use crate::auditor::{
    auditor_quorum_specs, tally_votes, AuditFinding, AuditorSpawnSpec, ConfirmedFinding, Severity,
};
use crate::master::spawn_spec_from_task;
use crate::pitem::{ArtifactKind, FailReason, PItemId, PItemStatus};
use crate::runner::TaskHost;
use crate::task::{
    get_task, git_create_branch, pick_unique_branch, slugify_title, task_json_path,
    task_write_lock, tasks_dir, write_task_atomic, TaskStatus,
};
use crate::worker::worker_spawn_spec;
use crate::worktree::{self, MergeOutcome};

// ── output_summary validation (REQ-013, DEC-012) ─────────────────────────────

/// [REQ-013][DEC-012] The PRD requires every worker `output_summary` to be
/// 50–200 *characters* (`字`) and tagged with an `artifact_kind`. P6 added the
/// `output_summary` field + `ArtifactKind` enum; DEC-012 explicitly deferred
/// this mark-done-side *validation* to P9 (here): the acceptance engine checks
/// code, this checks the summary metadata the master will inject into
/// downstream Layer-3 contexts.
///
/// Length policy (registry REQ-013 acceptance): `< 50` → warn (accept, the
/// summary is just thin); `> 200` → truncate (keep the first 200 chars so the
/// downstream context stays bounded); in-range → accept verbatim. We measure
/// Unicode scalar values (`chars().count()`) because the spec says "字"
/// (characters), and a Chinese-language summary's word count is meaningless.
pub const SUMMARY_MIN_CHARS: usize = 50;
pub const SUMMARY_MAX_CHARS: usize = 200;

/// Outcome of validating a worker `output_summary` against REQ-013.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryValidation {
    /// The summary actually stored — truncated to [`SUMMARY_MAX_CHARS`] when
    /// the input was over-long, otherwise the input verbatim.
    pub stored: String,
    /// Number of Unicode scalar chars in the *original* input.
    pub original_chars: usize,
    /// `true` when the original was shorter than [`SUMMARY_MIN_CHARS`] (warn,
    /// not reject).
    pub too_short: bool,
    /// `true` when the original exceeded [`SUMMARY_MAX_CHARS`] and was
    /// truncated.
    pub truncated: bool,
}

/// [REQ-013] Validate + normalise a worker `output_summary`. Never rejects on
/// length alone (short → warn, long → truncate); the only hard requirement
/// (`artifact_kind` 必选) is enforced separately against the P-item's
/// `artifacts` field by the caller, because the mark-done signature carries no
/// artifact-kind parameter.
pub fn validate_output_summary(summary: &str) -> SummaryValidation {
    let original_chars = summary.chars().count();
    let too_short = original_chars < SUMMARY_MIN_CHARS;
    let truncated = original_chars > SUMMARY_MAX_CHARS;
    let stored = if truncated {
        summary.chars().take(SUMMARY_MAX_CHARS).collect()
    } else {
        summary.to_string()
    };
    SummaryValidation {
        stored,
        original_chars,
        too_short,
        truncated,
    }
}

/// [REQ-013] artifact_kind is mandatory on a Done P-item. The mark-done call
/// site has no artifact-kind parameter (the signature is shared with the core
/// wrapper + fleet-cli, both outside P9's touches), so the kind is read from
/// the P-item's own `artifacts` field, which the planner declares. An empty
/// `artifacts` list means the worker never tagged an artifact kind → reject.
fn require_artifact_kind(artifacts: &[ArtifactKind]) -> Result<(), String> {
    if artifacts.is_empty() {
        Err("REQ-013: output_summary must declare an artifact_kind \
             (FileList|GitDiff|TestOutput|ManualNote); the P-item's `artifacts` \
             list is empty"
            .to_string())
    } else {
        Ok(())
    }
}

// ── deviation-marker parsing (REQ-042, REQ-026) ──────────────────────────────

/// A simplification/deviation the worker self-declared in its `output_summary`.
/// [REQ-026] When a worker can't fully satisfy a requirement it must say so in
/// its summary; [REQ-042] the master parses those markers and turns each into a
/// deviation-ledger entry carrying the `affected_req` ids so the Auditor can
/// trace which REQ was weakened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDeviation {
    /// The decision text after the marker (what was simplified / deferred).
    pub decision: String,
    /// REQ ids named in the marker line, e.g. `["REQ-013", "REQ-026"]`.
    pub affected_req: Vec<String>,
}

/// [REQ-042][REQ-026] Parse `[DEVIATION]` / 简化声明 markers out of a worker
/// `output_summary`. Recognised marker tokens (case-insensitive for the ASCII
/// one) at the start of a line: `[DEVIATION]`, `简化声明`, `简化:`. Everything
/// after the marker on that line is the decision text; any `REQ-NNN` tokens
/// found anywhere on the line become `affected_req`. Multiple markers (one per
/// line) yield multiple [`ParsedDeviation`]s, in source order.
pub fn parse_deviation_markers(summary: &str) -> Vec<ParsedDeviation> {
    let mut out = Vec::new();
    for line in summary.lines() {
        let trimmed = line.trim();
        let rest = strip_deviation_marker(trimmed);
        let Some(rest) = rest else { continue };
        let affected_req = extract_req_ids(trimmed);
        out.push(ParsedDeviation {
            decision: rest.trim().to_string(),
            affected_req,
        });
    }
    out
}

/// Return the text after a recognised deviation marker, or `None` if the line
/// is not a deviation declaration.
fn strip_deviation_marker(line: &str) -> Option<&str> {
    // ASCII `[DEVIATION]` is matched case-insensitively.
    let lower = line.to_ascii_lowercase();
    if let Some(idx) = lower.find("[deviation]") {
        return Some(&line[idx + "[deviation]".len()..]);
    }
    for marker in ["简化声明", "简化:", "简化："] {
        if let Some(idx) = line.find(marker) {
            return Some(&line[idx + marker.len()..]);
        }
    }
    None
}

/// Extract every `REQ-NNN` token from a line. Matches `REQ-` followed by one or
/// more ASCII digits.
fn extract_req_ids(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = line.as_bytes();
    let needle = b"REQ-";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + needle.len() {
                ids.push(String::from_utf8_lossy(&bytes[i..j]).into_owned());
                i = j;
                continue;
            }
        }
        i += 1;
    }
    ids
}

// ── deviation ledger (mirrored — see crate-cycle note) ───────────────────────

/// Approval state of a deviation ledger entry. Mirror of
/// `claw_fleet_core::deviation_ledger::DeviationStatus` — see the
/// module-level note for why P9 cannot import the core type directly.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeviationStatus {
    Pending,
    Approved,
    Rejected,
}

/// One deviation-ledger entry. Field-for-field schema-compatible (same
/// `camelCase` keys) with `claw_fleet_core::deviation_ledger::DeviationEntry`,
/// so an entry P9 appends here is read back losslessly by core's
/// `read_deviations()` and vice-versa — both target the same
/// `~/.fleet/deviations.jsonl`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviationEntry {
    pub id: String,
    pub date: String,
    pub affected_req: Vec<String>,
    pub decision: String,
    pub rationale: String,
    pub risk_level: String,
    pub mitigation: String,
    pub approved_by: Option<String>,
    pub signed_off_at: Option<i64>,
    pub status: DeviationStatus,
}

/// `~/.fleet/deviations.jsonl` — same path core's deviation_ledger uses.
fn deviations_path() -> Result<PathBuf, String> {
    crate::paths::get_fleet_dir()
        .map(|d| d.join("deviations.jsonl"))
        .ok_or_else(|| "could not resolve fleet home dir".to_string())
}

/// [REQ-026][REQ-042] Append one deviation entry as a single JSONL line.
/// Append-only: never truncates/rewrites, so the audit trail is preserved.
fn append_deviation(entry: &DeviationEntry) -> Result<PathBuf, String> {
    let path = deviations_path()?;
    append_jsonl_line(&path, entry)?;
    Ok(path)
}

/// [REQ-027] Read every deviation entry in append (chronological) order.
/// Malformed lines are skipped so one corrupt append can't hide the rest.
fn read_deviations() -> Result<Vec<DeviationEntry>, String> {
    read_jsonl_lines::<DeviationEntry>(&deviations_path()?)
}

/// [REQ-027] Pending deviations whose `affected_req` intersects this P-item's
/// covered REQ ids. A non-empty result means mark-done must not proceed
/// straight to Done (see [`mark_done`] / DEC-007).
fn pending_deviations_for_reqs(covered_reqs: &[String]) -> Result<Vec<DeviationEntry>, String> {
    Ok(read_deviations()?
        .into_iter()
        .filter(|d| {
            d.status == DeviationStatus::Pending
                && d.affected_req.iter().any(|r| covered_reqs.contains(r))
        })
        .collect())
}

/// Build a fresh `Pending` deviation entry for a worker-declared simplification.
fn pending_entry(parsed: &ParsedDeviation, task_id: &str, p_item_id: &str) -> DeviationEntry {
    let now = chrono::Utc::now();
    DeviationEntry {
        id: format!("DEV-{}-{}-{}", task_id, p_item_id, now.timestamp()),
        date: now.format("%Y-%m-%d").to_string(),
        affected_req: parsed.affected_req.clone(),
        decision: parsed.decision.clone(),
        rationale: format!(
            "worker self-declared simplification on P-item {p_item_id} (task {task_id})"
        ),
        // Worker self-declared, not yet human-reviewed: medium until a human
        // weighs the blast radius. Conservative default; never auto-low.
        risk_level: "medium".to_string(),
        mitigation: "pending human review at mark-done gate (REQ-027)".to_string(),
        approved_by: None,
        signed_off_at: None,
        status: DeviationStatus::Pending,
    }
}

// ── master decision log (REQ-042) ────────────────────────────────────────────

/// [REQ-042] One structured master-decision record. The methodology's
/// traceability pillar requires the master to log every `dispatch` / `mark-done`
/// decision with the acceptance verdict and whether a deviation was recorded,
/// so the Auditor can replay the audit. Written to `~/.fleet/master-decisions.jsonl`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MasterDecision {
    pub task_id: String,
    pub p_item_id: String,
    /// `dispatch` | `mark-done` | `mark-failed`.
    pub kind: String,
    pub timestamp: i64,
    /// Per-criterion verdicts, e.g. `["Builds=pass", "TestsPass=fail"]`. Empty
    /// for dispatch / mark-failed.
    pub acceptance: Vec<String>,
    /// The audit verdict (`allPassed` | `rejected` | `waitHuman`) when one ran.
    pub decision: Option<String>,
    /// Deviation entry ids recorded as part of this decision.
    pub deviations_recorded: Vec<String>,
    /// Free-text note (e.g. which pending deviation blocked, retry hint).
    pub note: String,
}

fn decisions_path() -> Result<PathBuf, String> {
    crate::paths::get_fleet_dir()
        .map(|d| d.join("master-decisions.jsonl"))
        .ok_or_else(|| "could not resolve fleet home dir".to_string())
}

/// [REQ-042] Append one master decision as a JSONL line (append-only).
/// Best-effort: logging failure must not abort the decision itself, so callers
/// ignore the result.
fn log_master_decision(d: &MasterDecision) -> Result<PathBuf, String> {
    let path = decisions_path()?;
    append_jsonl_line(&path, d)?;
    Ok(path)
}

/// Read every master decision in append order (for tests / the Auditor).
pub fn read_master_decisions() -> Result<Vec<MasterDecision>, String> {
    read_jsonl_lines::<MasterDecision>(&decisions_path()?)
}

// ── shared append-only JSONL primitives (mirror of deviation_ledger) ─────────

fn append_jsonl_line<T: Serialize>(path: &PathBuf, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir ledger dir: {e}"))?;
    }
    let mut line =
        serde_json::to_string(value).map_err(|e| format!("serialize ledger entry: {e}"))?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open ledger {}: {e}", path.display()))?;
    f.write_all(line.as_bytes())
        .map_err(|e| format!("append ledger line: {e}"))?;
    Ok(())
}

fn read_jsonl_lines<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Result<Vec<T>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("read ledger: {e}"))?;
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<T>(line) {
            out.push(v);
        }
    }
    Ok(out)
}

/// [REQ-026] The REQ ids a P-item's worker work is expected to cover. The
/// planner does not (yet) attach an explicit REQ list to each P-item, so we
/// derive the set the mark-done gate must guard from two sources: REQ ids the
/// worker named in its own deviation markers (those are exactly the REQs the
/// worker says it weakened) plus any REQ id mentioned in the P-item `desc`.
/// This is the set [`pending_deviations_for_reqs`] intersects against.
fn covered_reqs(item_desc: &str, parsed: &[ParsedDeviation]) -> Vec<String> {
    let mut set: Vec<String> = extract_req_ids(item_desc);
    for p in parsed {
        for r in &p.affected_req {
            if !set.contains(r) {
                set.push(r.clone());
            }
        }
    }
    set
}

// ── WA2: live adversarial-audit orchestration (methodology pillar 4) ─────────

/// Spawns ONE independent Auditor session. The audit orchestrator
/// ([`audit_pitem`]) drives three of these and 2-of-3 votes on their findings.
///
/// Abstracted as a trait for the same reason `dispatch_pitem` takes a
/// `TaskHost`: the real launcher shells out to `claude` (it lives in
/// `fleet-task`'s `ProcessLauncher`/`LocalHost`, which `claw-fleet-task` cannot
/// reference without a dependency cycle), while tests supply a fake that writes
/// canned `AuditFinding[]` JSON to the output path so the polling / tally /
/// timeout logic can be exercised without a real Claude process.
///
/// `session_index` selects which of the 3 quorum specs to launch; `output_path`
/// is where that session must deposit its findings JSON ([`audit_pitem`] polls
/// it). Returns the spawned pid (unused by the orchestrator beyond liveness,
/// but kept so a real launcher can be reaped/observed).
pub trait AuditorLauncher {
    fn launch_auditor(
        &self,
        session_index: usize,
        spec: &AuditorSpawnSpec,
        output_path: &std::path::Path,
    ) -> Result<u32, String>;
}

/// The verdict line `audit_pitem` returns (and the CLI prints). `CriticalConfirmed`
/// means at least one Critical finding cleared the 2-of-3 quorum — per WA3 the
/// master MUST NOT mark-done. `Clean` means no Critical finding was confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditVerdict {
    /// `n` = number of Critical confirmed findings.
    CriticalConfirmed(usize),
    Clean,
}

impl AuditVerdict {
    /// The stdout verdict line the CLI prints and the master parses.
    /// `AUDIT_VERDICT: CRITICAL_CONFIRMED <n>` or `AUDIT_VERDICT: CLEAN`.
    pub fn verdict_line(&self) -> String {
        match self {
            AuditVerdict::CriticalConfirmed(n) => {
                format!("AUDIT_VERDICT: CRITICAL_CONFIRMED {n}")
            }
            AuditVerdict::Clean => "AUDIT_VERDICT: CLEAN".to_string(),
        }
    }
}

/// Result of running the full quorum audit on one P-item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditOutcome {
    pub verdict: AuditVerdict,
    /// The confirmed (2-of-3) findings, written to the audits ledger.
    pub confirmed: Vec<ConfirmedFinding>,
    /// Path the confirmed-findings JSON was written to.
    pub written_to: PathBuf,
}

/// Default hard timeout for waiting on the 3 auditor sessions to deposit their
/// findings. Generous enough for an Opus session to read the log and emit JSON;
/// past it, a missing session is treated as a FAILURE (never a silent pass).
pub const AUDITOR_HARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Per-poll sleep while waiting for auditor output files to appear.
const AUDITOR_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

fn audits_dir() -> Result<PathBuf, String> {
    crate::paths::get_fleet_dir()
        .map(|d| d.join("audits"))
        .ok_or_else(|| "could not resolve fleet home dir".to_string())
}

/// A synthetic Critical finding standing in for an auditor session that never
/// delivered (timed out / wrote unparseable output). [WA2] The spec is
/// explicit: "超时按失败处理不能假装通过" — a non-responding auditor must NOT be
/// read as a clean vote. We inject a distinct Critical per missing session so
/// that if a majority of the quorum fails to report, the 2-of-3 tally surfaces
/// a confirmed `auditor-timeout` Critical rather than a false CLEAN.
fn timeout_finding(session_index: usize, output_path: &std::path::Path) -> AuditFinding {
    AuditFinding {
        severity: Severity::Critical,
        category: "auditor-timeout".to_string(),
        finding: format!(
            "auditor session #{session_index} did not deliver findings before the hard timeout"
        ),
        evidence: format!(
            "expected findings JSON at {} was missing or unparseable after {}s",
            output_path.display(),
            AUDITOR_HARD_TIMEOUT.as_secs()
        ),
        // Tie all timeout findings to the same REQ set so they share a dedup_key
        // and a *quorum* of timeouts (>=2) confirms a single auditor-timeout
        // Critical instead of N separate singletons.
        req_affected: vec!["REQ-022".to_string(), "REQ-033".to_string()],
    }
}

/// [WA2] Run the live adversarial audit for one P-item: spawn 3 independent
/// Auditor sessions, wait (with a hard timeout) for each to deposit its
/// `AuditFinding[]` JSON, tally the 2-of-3 vote, persist the confirmed findings,
/// and return the verdict.
///
/// Steps mirror the methodology's pillar 4:
/// 1. Resolve the Master decision log (`~/.fleet/master-decisions.jsonl`) the
///    auditors read, and build the 3 quorum specs via [`auditor_quorum_specs`].
/// 2. Spawn each session through the [`AuditorLauncher`], handing it a distinct
///    output path under `~/.fleet/audits/<task>_<p>/session_<idx>.json`.
/// 3. Poll those files until all 3 are present+parseable, or [`AUDITOR_HARD_TIMEOUT`]
///    elapses. A session that hasn't delivered by the deadline is recorded as a
///    Critical `auditor-timeout` finding — NOT a silent clean vote.
/// 4. [`tally_votes`] the per-session findings (2-of-3). Any confirmed Critical
///    → `CriticalConfirmed`; otherwise `Clean`.
/// 5. Write the confirmed findings to `~/.fleet/audits/<task>_<p>_<ts>.json`.
///
/// `timeout` lets tests shorten the wait; production passes
/// [`AUDITOR_HARD_TIMEOUT`].
pub fn audit_pitem(
    task_id: &str,
    p_item_id: &str,
    launcher: &dyn AuditorLauncher,
    timeout: std::time::Duration,
) -> Result<AuditOutcome, String> {
    let master_log = decisions_path()?;
    let specs = auditor_quorum_specs(task_id, &master_log.to_string_lossy());

    // Per-session output drop directory: ~/.fleet/audits/<task>_<p>/
    let session_dir = audits_dir()?.join(format!("{task_id}_{p_item_id}"));
    std::fs::create_dir_all(&session_dir)
        .map_err(|e| format!("mkdir audit session dir {}: {e}", session_dir.display()))?;

    // 2. Spawn each independent session, telling it where to write.
    let output_paths: Vec<PathBuf> = specs
        .iter()
        .map(|s| session_dir.join(format!("session_{}.json", s.session_index)))
        .collect();
    // Remove any stale output from a prior audit so we never read old findings.
    for p in &output_paths {
        let _ = std::fs::remove_file(p);
    }
    for (idx, spec) in specs.iter().enumerate() {
        // Spawn failure of one session is itself a failure to audit — record it
        // as a timeout-style miss rather than aborting the whole audit (so the
        // remaining sessions still vote and we never fall through to a fake pass).
        let _ = launcher.launch_auditor(idx, spec, &output_paths[idx]);
    }

    // 3. Poll for each session's findings until all present or hard timeout.
    let deadline = std::time::Instant::now() + timeout;
    let mut per_session: Vec<Option<Vec<AuditFinding>>> = vec![None; specs.len()];
    loop {
        for (idx, path) in output_paths.iter().enumerate() {
            if per_session[idx].is_some() {
                continue;
            }
            if let Some(findings) = try_read_findings(path) {
                per_session[idx] = Some(findings);
            }
        }
        if per_session.iter().all(Option::is_some) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break; // hard timeout — remaining None sessions become timeouts below
        }
        std::thread::sleep(AUDITOR_POLL_INTERVAL);
    }

    // 4. Any session that never delivered counts as a Critical timeout (never a
    //    silent clean vote). Then 2-of-3 tally.
    let session_findings: Vec<Vec<AuditFinding>> = per_session
        .into_iter()
        .enumerate()
        .map(|(idx, maybe)| maybe.unwrap_or_else(|| vec![timeout_finding(idx, &output_paths[idx])]))
        .collect();
    let confirmed = tally_votes(&session_findings);

    let critical_count = confirmed
        .iter()
        .filter(|c| c.finding.severity == Severity::Critical)
        .count();
    let verdict = if critical_count > 0 {
        AuditVerdict::CriticalConfirmed(critical_count)
    } else {
        AuditVerdict::Clean
    };

    // 5. Persist the confirmed findings to the audits ledger.
    let ts = chrono::Utc::now().timestamp();
    let written_to = audits_dir()?.join(format!("{task_id}_{p_item_id}_{ts}.json"));
    if let Some(parent) = written_to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir audits dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&confirmed)
        .map_err(|e| format!("serialize confirmed findings: {e}"))?;
    std::fs::write(&written_to, json)
        .map_err(|e| format!("write audit ledger {}: {e}", written_to.display()))?;

    Ok(AuditOutcome {
        verdict,
        confirmed,
        written_to,
    })
}

/// Try to read + parse one auditor session's `AuditFinding[]` JSON. Returns
/// `None` when the file is absent, empty (still being written), or unparseable —
/// the caller retries until the hard timeout. A successful parse of `[]` is a
/// real "clean" vote and returns `Some(vec![])`.
fn try_read_findings(path: &std::path::Path) -> Option<Vec<AuditFinding>> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None; // file created but not yet flushed
    }
    serde_json::from_str::<Vec<AuditFinding>>(&content).ok()
}

/// Optional knobs for `start_task`. Today only carries `force_human_gate_all`
/// (project-wide manual-review policy in claw-fleet-core); fleet-task's
/// binary always passes `Default::default()`.
#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    /// When true, mark every P-item as `human_gate=true` before spawning the
    /// master. Used by `claw_fleet_core` to honour `Project.manual_review_all`.
    pub force_human_gate_all: bool,
}

/// Transition a Drafting / Planning / Ready task to Running. Creates a fresh
/// `fleet/<slug>` git branch in the workspace returned by
/// `host.workspace_for_task`, spawns the Master session via
/// `host.enqueue_master`, stamps `started_at` + `master_session_id`, and
/// persists `task_branch`. Idempotent: returns `Ok` immediately if the task
/// is already running.
pub fn start_task(task_id: &str, host: &dyn TaskHost, opts: StartOpts) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Running) {
        return Ok(());
    }
    let workspace = host.workspace_for_task(&task)?;
    let slug = slugify_title(&task.title);
    let branch = pick_unique_branch(&workspace, &slug)?;
    git_create_branch(&workspace, &branch)?;
    task.task_branch = Some(branch);
    // Phase 3: persist workspace so out-of-process tools (fleet-cli, master
    // tool calls) can find the working tree without a project table lookup.
    task.workspace = Some(workspace.clone());
    task.status = TaskStatus::Running;
    task.started_at = Some(chrono::Utc::now().timestamp());
    if opts.force_human_gate_all {
        for item in task.plan.items.values_mut() {
            item.human_gate = true;
        }
    }
    write_task_atomic(&task)?;
    let spec = spawn_spec_from_task(&task, workspace.clone())?;
    match host.enqueue_master(&spec) {
        Ok(master_sid) => {
            task.master_session_id = Some(master_sid);
            write_task_atomic(&task)?;
            Ok(())
        }
        Err(e) => Err(format!("master enqueue failed: {e}")),
    }
}

/// Mark a P-item Done — the full mark-done acceptance gate (P9).
///
/// This is the methodology's enforcement point. It runs, in order:
///
/// 1. **[REQ-013][DEC-012] output_summary validation** — 50–200 chars (short →
///    warn, long → truncate) and a mandatory `artifact_kind` (read from the
///    P-item's `artifacts`, since the call signature carries no kind param).
/// 2. **[REQ-042][REQ-026] deviation-marker parsing** — `[DEVIATION]` / 简化声明
///    lines in the summary become `Pending` deviation-ledger entries carrying
///    their `affected_req`.
/// 3. **[REQ-027][DEC-007] pending-deviation gate** — if any `Pending`
///    deviation touches a REQ this P-item covers, mark-done refuses (the master
///    retries the worker once, then asks the user) — never straight to Done.
/// 4. **[REQ-026][REQ-050] acceptance audit** — `acceptance::audit_acceptance`
///    runs every declared `AcceptanceCriterion` *in the worker's worktree*
///    (real build/test evidence, never a proxy signal). `Rejected` → refuse;
///    `WaitHuman` → park in `WaitHumanGate`; `AllPassed` → proceed.
/// 5. **merge + flip Done** — only on `AllPassed` is the worker branch merged
///    back (3-way conflict → host's `LlmMediator`) and the P-item flipped Done.
///
/// Every branch logs a structured [`MasterDecision`] (REQ-042). The signature
/// is unchanged from the migrated version (the core wrapper + fleet-cli call it
/// and are outside P9's touches); refusals are surfaced as `Err(String)` with a
/// machine-readable prefix the master can route on.
pub fn mark_done(
    task_id: &str,
    p_item_id: &str,
    summary: &str,
    host: &dyn TaskHost,
) -> Result<MergeOutcome, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let workspace = host.workspace_for_task(&task)?;
    let task_branch = task
        .task_branch
        .clone()
        .ok_or_else(|| format!("task {task_id} has no task_branch"))?;

    // Snapshot the P-item shape we need before any mutation.
    let (item_desc, item_acceptance, item_artifacts, human_gate) = {
        let item = task
            .plan
            .get(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        (
            item.desc.clone(),
            item.acceptance.clone(),
            item.artifacts.clone(),
            item.human_gate,
        )
    };

    // 1. [REQ-013][DEC-012] output_summary validation + mandatory artifact_kind.
    require_artifact_kind(&item_artifacts)?;
    let validated = validate_output_summary(summary);

    // 2. [REQ-042][REQ-026] parse worker-declared simplifications → Pending
    // deviation ledger entries (with affected_req).
    let parsed = parse_deviation_markers(summary);
    let mut recorded_ids = Vec::new();
    for p in &parsed {
        let entry = pending_entry(p, task_id, p_item_id);
        recorded_ids.push(entry.id.clone());
        append_deviation(&entry)?;
    }

    // 3. [REQ-027][DEC-007] refuse Done while a relevant deviation is Pending.
    let covered = covered_reqs(&item_desc, &parsed);
    let blocking = pending_deviations_for_reqs(&covered)?;
    if !blocking.is_empty() {
        let ids: Vec<&str> = blocking.iter().map(|d| d.id.as_str()).collect();
        log_master_decision(&MasterDecision {
            task_id: task_id.to_string(),
            p_item_id: p_item_id.to_string(),
            kind: "mark-done".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            acceptance: vec![],
            decision: None,
            deviations_recorded: recorded_ids.clone(),
            note: format!("blocked by pending deviation(s): {}", ids.join(", ")),
        })
        .ok();
        // DEC-007: retry the worker once before escalating to the user.
        return Err(format!(
            "PENDING_DEVIATION: P-item {p_item_id} has unapproved deviation(s) [{}] \
             touching its REQ(s) {covered:?}; per DEC-007 retry the worker once, \
             then AskUserQuestion before marking done",
            ids.join(", ")
        ));
    }

    // 4. [REQ-026][REQ-050] real acceptance audit in the worker's worktree —
    // never a proxy signal. The worktree still holds the worker's changes
    // (it is reaped only after a successful merge below).
    let audit_cwd =
        worktree::worktree_path(task_id, p_item_id).unwrap_or_else(|_| workspace.clone());
    let audit_cwd = if audit_cwd.exists() { audit_cwd } else { workspace.clone() };
    // [REQ-011] gate = P-item human_gate OR task manual_review (already folded
    // into each item's human_gate by start_task; no task-level field exists at
    // mark-done time, so the second arg is the persisted per-item flag).
    let gate = acceptance::requires_human_gate(human_gate, false);

    // [REQ-003][REQ-004] No declared acceptance criteria means there is NO real
    // evidence to check — the only thing left to flip Done on would be a proxy
    // signal (worker self-report / token count / elapsed time / diff size),
    // which the adversarial-audit pillar forbids. Refuse to mark Done unless a
    // human gate is in force (in which case the user, not a proxy, decides).
    if item_acceptance.is_empty() && !gate {
        let now = chrono::Utc::now().timestamp();
        log_master_decision(&MasterDecision {
            task_id: task_id.to_string(),
            p_item_id: p_item_id.to_string(),
            kind: "mark-done".to_string(),
            timestamp: now,
            acceptance: vec![],
            decision: Some("rejected".to_string()),
            deviations_recorded: recorded_ids,
            note: "no declared acceptance criteria — refusing to mark Done on proxy signals only \
                   (REQ-003/REQ-004)"
                .to_string(),
        })
        .ok();
        return Err(format!(
            "ACCEPTANCE_REJECTED: P-item {p_item_id} declared no acceptance criteria; \
             marking Done would rely on proxy signals only — forbidden (REQ-003/REQ-004). \
             Declare a Builds/TestsPass/Custom criterion or set human_gate"
        ));
    }

    let report = acceptance::audit_acceptance(&audit_cwd, &item_acceptance, gate);
    let _ = acceptance::persist_results(task_id, p_item_id, &report.results);
    let acceptance_log: Vec<String> = report
        .results
        .iter()
        .map(|r| {
            let verdict = if r.passed {
                "pass"
            } else if r.needs_human {
                "needs-human"
            } else {
                "fail"
            };
            format!("{:?}={verdict}", r.criterion)
        })
        .collect();

    match report.decision {
        AuditDecision::Rejected => {
            // [REQ-003][REQ-004][REQ-050] A declared criterion produced failing
            // evidence (or was missing/empty). Do NOT mark Done.
            let now = chrono::Utc::now().timestamp();
            if let Some(item) = task.plan.get_mut(p_item_id) {
                item.status = PItemStatus::Failed(FailReason::AcceptanceRejected);
                item.completed_at = Some(now);
                item.output_summary = Some(validated.stored.clone());
            }
            write_task_atomic(&task)?;
            log_master_decision(&MasterDecision {
                task_id: task_id.to_string(),
                p_item_id: p_item_id.to_string(),
                kind: "mark-done".to_string(),
                timestamp: now,
                acceptance: acceptance_log.clone(),
                decision: Some("rejected".to_string()),
                deviations_recorded: recorded_ids,
                note: "acceptance audit rejected — at least one criterion failed".to_string(),
            })
            .ok();
            Err(format!(
                "ACCEPTANCE_REJECTED: P-item {p_item_id} failed acceptance audit [{}] — \
                 not marked Done (REQ-050)",
                acceptance_log.join(", ")
            ))
        }
        AuditDecision::WaitHuman => {
            // [REQ-011] Machine checks clean but a HumanReview criterion or an
            // active human gate applies → park in WaitHumanGate, ask the user;
            // never unilaterally Done. No merge yet.
            let now = chrono::Utc::now().timestamp();
            if let Some(item) = task.plan.get_mut(p_item_id) {
                item.status = PItemStatus::WaitHumanGate;
                item.output_summary = Some(validated.stored.clone());
            }
            write_task_atomic(&task)?;
            log_master_decision(&MasterDecision {
                task_id: task_id.to_string(),
                p_item_id: p_item_id.to_string(),
                kind: "mark-done".to_string(),
                timestamp: now,
                acceptance: acceptance_log.clone(),
                decision: Some("waitHuman".to_string()),
                deviations_recorded: recorded_ids,
                note: "human gate / HumanReview criterion → AskUserQuestion".to_string(),
            })
            .ok();
            Err(format!(
                "WAIT_HUMAN: P-item {p_item_id} passed machine checks [{}] but needs human \
                 approval — call AskUserQuestion, then mark-done again after approval",
                acceptance_log.join(", ")
            ))
        }
        AuditDecision::AllPassed => {
            // Every declared criterion gathered real passing evidence and no
            // gate applies → safe to merge + flip Done.
            let outcome = worktree::merge_back(&workspace, &task_branch, task_id, p_item_id)?;
            let outcome = if let MergeOutcome::Conflict { files, reason } = outcome {
                let mediations = host.resolve_conflicts(&files).map_err(|e| {
                    format!(
                        "mediator failed for {n} conflicted file(s) ({reason}): {e}",
                        n = files.len(),
                    )
                })?;
                let resolutions: Vec<(PathBuf, String)> = mediations
                    .into_iter()
                    .map(|m| (m.path, m.resolved_content))
                    .collect();
                worktree::apply_resolutions(&workspace, task_id, p_item_id, &resolutions)?
            } else {
                outcome
            };
            let now = chrono::Utc::now().timestamp();
            {
                let item = task
                    .plan
                    .get_mut(p_item_id)
                    .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
                item.status = PItemStatus::Done;
                item.completed_at = Some(now);
                item.output_summary = Some(validated.stored.clone());
            }
            write_task_atomic(&task)?;
            let _ = worktree::reap(&workspace, task_id, p_item_id);
            log_master_decision(&MasterDecision {
                task_id: task_id.to_string(),
                p_item_id: p_item_id.to_string(),
                kind: "mark-done".to_string(),
                timestamp: now,
                acceptance: acceptance_log,
                decision: Some("allPassed".to_string()),
                deviations_recorded: recorded_ids,
                note: format!(
                    "marked Done (summary {} chars{}{})",
                    validated.original_chars,
                    if validated.too_short { ", warn:short" } else { "" },
                    if validated.truncated { ", truncated" } else { "" },
                ),
            })
            .ok();
            Ok(outcome)
        }
    }
}

/// Mark a P-item Failed. Releases the worktree, flips the status, propagates
/// skip on poisoned downstream items.
pub fn mark_failed(
    task_id: &str,
    p_item_id: &str,
    reason: FailReason,
    host: &dyn TaskHost,
) -> Result<Vec<PItemId>, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        item.status = PItemStatus::Failed(reason);
        item.completed_at = Some(now);
    }
    let newly_skipped = task.plan.propagate_skip();
    write_task_atomic(&task)?;
    if let Ok(workspace) = host.workspace_for_task(&task) {
        let _ = worktree::reap(&workspace, task_id, p_item_id);
    }
    // [REQ-042] Structured master decision log for the mark-failed transition,
    // including the downstream P-items auto-skipped as a consequence.
    log_master_decision(&MasterDecision {
        task_id: task_id.to_string(),
        p_item_id: p_item_id.to_string(),
        kind: "mark-failed".to_string(),
        timestamp: now,
        acceptance: vec![],
        decision: Some("failed".to_string()),
        deviations_recorded: vec![],
        note: format!("propagated skip to: {newly_skipped:?}"),
    })
    .ok();
    Ok(newly_skipped)
}

/// Master tool — dispatch worker for `p_item_id`. Flips its status to
/// Running, provisions a worktree under the task branch, and asks the host
/// to spawn a Worker session via `enqueue_worker`.
pub fn dispatch_pitem(
    task_id: &str,
    p_item_id: &str,
    host: &dyn TaskHost,
) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        match &item.status {
            PItemStatus::WaitDeps | PItemStatus::WaitResource => {
                item.status = PItemStatus::Running;
                item.started_at = Some(now);
            }
            PItemStatus::Running => {
                return Ok(());
            }
            other => {
                return Err(format!(
                    "cannot dispatch p-item {p_item_id} in status {other:?}"
                ));
            }
        }
    }
    write_task_atomic(&task)?;
    let workspace = host.workspace_for_task(&task)?;
    let task_branch = task.task_branch.as_deref().ok_or_else(|| {
        format!("task {task_id} has no task_branch — call start_task before dispatching P-items")
    })?;
    let cwd = worktree::provision(&workspace, task_branch, task_id, p_item_id)?;
    let spec = worker_spawn_spec(&task, p_item_id, cwd)?;
    let worker_sid = host.enqueue_worker(&spec)?;
    if let Some(item) = task.plan.get_mut(p_item_id) {
        item.agent_session_id = Some(worker_sid);
    }
    write_task_atomic(&task)
}

/// Pause a running task. Flips status to Paused and asks the host to SIGSTOP
/// all related subprocesses. Signal failures are non-fatal.
pub fn pause_task(task_id: &str, host: &dyn TaskHost) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Paused) {
        return Ok(());
    }
    if !matches!(task.status, TaskStatus::Running) {
        return Err(format!(
            "task {task_id} is not running (status: {:?})",
            task.status
        ));
    }
    task.status = TaskStatus::Paused;
    write_task_atomic(&task)?;
    let _ = host.pause_task_sessions(task_id);
    Ok(())
}

/// Resume a paused task — host SIGCONT and flip Running.
pub fn resume_task(task_id: &str, host: &dyn TaskHost) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Running) {
        return Ok(());
    }
    if !matches!(task.status, TaskStatus::Paused) {
        return Err(format!(
            "task {task_id} is not paused (status: {:?})",
            task.status
        ));
    }
    task.status = TaskStatus::Running;
    write_task_atomic(&task)?;
    let _ = host.resume_task_sessions(task_id);
    Ok(())
}

/// Clear (delete) a task — host SIGTERM, then remove task json + materials.
pub fn clear_task(task_id: &str, host: &dyn TaskHost) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let _ = host.terminate_task_sessions(task_id);
    if let Ok(mut task) = get_task(task_id) {
        task.status = TaskStatus::Abandoned;
        let _ = write_task_atomic(&task);
    }
    let json = task_json_path(task_id)?;
    if json.exists() {
        fs::remove_file(&json).map_err(|e| format!("remove task json: {e}"))?;
    }
    let materials = tasks_dir()?.join(task_id);
    if materials.exists() {
        fs::remove_dir_all(&materials).map_err(|e| format!("remove materials dir: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{AcceptanceCriterion, ArtifactKind, PItem, PItemStatus};
    use crate::plan::DagPlan;
    use crate::runner::{LlmMediator, Resolution, TaskLifecycleHost};
    use crate::task::{Task, TaskStatus};
    use crate::worktree::ConflictSpec;
    use std::path::Path;
    use tempfile::TempDir;

    // ── FLEET_HOME isolation (mirror of deviation_ledger's helper) ───────────

    /// Claims the process-wide FLEET_HOME mutex for the lifetime of the guard so
    /// parallel tests don't clobber each other's `~/.fleet` redirection.
    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl FleetHomeOverride {
        fn new(tmp: &Path) -> Self {
            let lock = crate::paths::fleet_home_lock();
            let prev = std::env::var_os("FLEET_HOME");
            std::env::set_var("FLEET_HOME", tmp);
            FleetHomeOverride { prev, _lock: lock }
        }
    }
    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }

    // ── a TaskHost mock that needs no real git/master/worker ─────────────────

    struct MockHost {
        workspace: PathBuf,
    }
    impl TaskLifecycleHost for MockHost {
        fn workspace_for_task(&self, _task: &Task) -> Result<PathBuf, String> {
            Ok(self.workspace.clone())
        }
        fn enqueue_master(&self, _spec: &crate::master::MasterSpawnSpec) -> Result<String, String> {
            Ok("master-sid".into())
        }
        fn enqueue_worker(&self, _spec: &crate::worker::WorkerSpawnSpec) -> Result<String, String> {
            Ok("worker-sid".into())
        }
        fn pause_task_sessions(&self, _t: &str) -> Result<usize, String> { Ok(0) }
        fn resume_task_sessions(&self, _t: &str) -> Result<usize, String> { Ok(0) }
        fn terminate_task_sessions(&self, _t: &str) -> Result<usize, String> { Ok(0) }
    }
    impl LlmMediator for MockHost {
        fn resolve_conflicts(&self, _f: &[ConflictSpec]) -> Result<Vec<Resolution>, String> {
            Ok(vec![])
        }
    }

    /// A workspace with a minimal compiling cargo crate (so `Builds`/`TestsPass`
    /// have something real to check) under FLEET_HOME-isolated `~/.fleet`.
    fn compiling_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"p9_fixture\"\nversion=\"0.0.0\"\nedition=\"2021\"\n\n[[bin]]\nname=\"p9_fixture\"\npath=\"main.rs\"\n",
        ).unwrap();
        fs::write(
            root.join("main.rs"),
            "fn main(){}\n#[test] fn t_ok(){assert_eq!(1+1,2);}\n#[test] fn t_bad(){assert_eq!(1+1,3);}\n",
        ).unwrap();
        // Init a git repo with one commit on a `fleet/t` branch so `merge_back`
        // (called only on the AllPassed path) can open the repo and report
        // `NoChanges` (no worker branch exists in these unit tests).
        let repo = git2::Repository::init(root).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new("Cargo.toml")).unwrap();
        idx.add_path(Path::new("main.rs")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let commit = repo.find_commit(commit).unwrap();
        repo.branch("fleet/t", &commit, true).unwrap();
        dir
    }

    fn pitem(id: &str, acceptance: Vec<AcceptanceCriterion>, artifacts: Vec<ArtifactKind>, human_gate: bool) -> PItem {
        PItem {
            id: id.into(),
            desc: "do the thing".into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance,
            artifacts,
            skippable: None,
            human_gate,
            status: PItemStatus::Running,
            agent_session_id: None,
            started_at: Some(1),
            completed_at: None,
            output_summary: None,
        }
    }

    fn write_task(task_id: &str, workspace: &Path, item: PItem) -> Task {
        let task = Task {
            id: task_id.into(),
            project_id: "proj".into(),
            title: "t".into(),
            description: String::new(),
            inbox_materials: vec![],
            plan: DagPlan::from_items([item]),
            status: TaskStatus::Running,
            created_at: 0,
            started_at: Some(1),
            completed_at: None,
            task_branch: Some("fleet/t".into()),
            workspace: Some(workspace.to_path_buf()),
            master_session_id: Some("master-sid".into()),
            title_auto: false,
        };
        write_task_atomic(&task).unwrap();
        task
    }

    // A summary >= 50 chars so length validation never short-circuits the test.
    fn ok_summary() -> String {
        "Implemented the parser module end to end and added unit tests covering both the happy path and the error path.".to_string()
    }

    // ── [REQ-013][DEC-012] output_summary validation ─────────────────────────

    #[test]
    fn req013_summary_in_range_accepted_verbatim() {
        let s = ok_summary();
        let v = validate_output_summary(&s);
        assert_eq!(v.stored, s);
        assert!(!v.too_short && !v.truncated);
    }

    #[test]
    fn req013_short_summary_warns_not_rejected() {
        let v = validate_output_summary("too short");
        assert!(v.too_short, "under 50 chars must warn");
        assert!(!v.truncated);
        // still stored (warn, not reject)
        assert_eq!(v.stored, "too short");
    }

    #[test]
    fn req013_long_summary_truncated_to_200() {
        let s: String = "x".repeat(500);
        let v = validate_output_summary(&s);
        assert!(v.truncated);
        assert_eq!(v.stored.chars().count(), SUMMARY_MAX_CHARS);
        assert_eq!(v.original_chars, 500);
    }

    #[test]
    fn req013_char_count_is_unicode_scalar_not_bytes() {
        // 60 Chinese chars = 180 bytes; must be counted as 60 chars (in range),
        // not 180 (which would wrongly truncate).
        let s: String = "测".repeat(60);
        let v = validate_output_summary(&s);
        assert_eq!(v.original_chars, 60);
        assert!(!v.truncated, "60 chars is within 200, must not truncate");
    }

    #[test]
    fn req013_artifact_kind_mandatory() {
        assert!(require_artifact_kind(&[]).is_err(), "empty artifacts must reject");
        assert!(require_artifact_kind(&[ArtifactKind::GitDiff]).is_ok());
    }

    // ── [REQ-042][REQ-026] deviation-marker parsing ──────────────────────────

    #[test]
    fn req042_parses_ascii_and_chinese_markers_with_req_ids() {
        let summary = "Did the work.\n[DEVIATION] skipped retry logic, affects REQ-027 and REQ-013\n简化声明：未实现 REQ-099 的缓存\nnormal line";
        let devs = parse_deviation_markers(summary);
        assert_eq!(devs.len(), 2, "two marker lines → two deviations");
        assert_eq!(devs[0].affected_req, vec!["REQ-027", "REQ-013"]);
        assert!(devs[0].decision.contains("skipped retry logic"));
        assert_eq!(devs[1].affected_req, vec!["REQ-099"]);
    }

    #[test]
    fn req042_no_marker_yields_no_deviation() {
        assert!(parse_deviation_markers("clean summary, nothing weakened").is_empty());
    }

    #[test]
    fn req042_marker_is_case_insensitive_ascii() {
        let devs = parse_deviation_markers("[deviation] lowercased marker affecting REQ-001");
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].affected_req, vec!["REQ-001"]);
    }

    #[test]
    fn extract_req_ids_finds_all_and_ignores_partials() {
        assert_eq!(extract_req_ids("REQ-1 then REQ-027 and REQ- (no digits)"), vec!["REQ-1", "REQ-027"]);
        assert!(extract_req_ids("no requirements here").is_empty());
    }

    // ── deviation ledger mirror round-trips to the shared path ───────────────

    #[test]
    fn deviation_entry_appends_and_reads_back() {
        let tmp = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        let e = pending_entry(
            &ParsedDeviation { decision: "x".into(), affected_req: vec!["REQ-007".into()] },
            "task1", "p1",
        );
        append_deviation(&e).unwrap();
        let back = read_deviations().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].affected_req, vec!["REQ-007"]);
        assert_eq!(back[0].status, DeviationStatus::Pending);
    }

    #[test]
    fn pending_deviations_filter_by_affected_req() {
        let tmp = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        // pending touching REQ-027
        append_deviation(&pending_entry(
            &ParsedDeviation { decision: "a".into(), affected_req: vec!["REQ-027".into()] },
            "t", "p",
        )).unwrap();
        // pending touching a different REQ
        append_deviation(&pending_entry(
            &ParsedDeviation { decision: "b".into(), affected_req: vec!["REQ-999".into()] },
            "t", "p",
        )).unwrap();
        let hit = pending_deviations_for_reqs(&["REQ-027".into()]).unwrap();
        assert_eq!(hit.len(), 1, "only the REQ-027 deviation matches");
        let none = pending_deviations_for_reqs(&["REQ-555".into()]).unwrap();
        assert!(none.is_empty());
    }

    // ── [DEC-010] mark_done behaviour: REQ-003/004/050 ───────────────────────

    /// [REQ-003][REQ-004] A P-item declaring NO machine criteria — the planner
    /// supplied only proxy signals (worker self-report) and no real acceptance
    /// — must NOT be marked Done. Empty acceptance = nothing real was checked,
    /// so flipping Done would rely on proxy signals only, which the
    /// adversarial-audit pillar forbids.
    #[test]
    fn req003_proxy_only_no_real_acceptance_is_rejected() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        // No acceptance criteria at all, no human gate → only proxy signals left.
        let item = pitem("p1", vec![], vec![ArtifactKind::ManualNote], false);
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let err = mark_done("t1", "p1", &ok_summary(), &host).unwrap_err();
        assert!(err.starts_with("ACCEPTANCE_REJECTED"), "got: {err}");
        assert!(err.contains("REQ-003"), "must cite the proxy-signal red line: {err}");
        // P-item must NOT have been flipped to Done.
        assert_ne!(get_task("t1").unwrap().plan.get("p1").unwrap().status, PItemStatus::Done);
    }

    /// [REQ-003][REQ-011] Same empty-acceptance P-item, but human_gate=true:
    /// now the *user* (not a proxy) decides → WaitHuman, not a hard reject. This
    /// proves the empty-criteria guard keys off "no real evidence AND no human",
    /// not merely "no criteria".
    #[test]
    fn req003_empty_acceptance_with_human_gate_waits_human() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem("p1", vec![], vec![ArtifactKind::ManualNote], true);
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let err = mark_done("t1", "p1", &ok_summary(), &host).unwrap_err();
        assert!(err.starts_with("WAIT_HUMAN"), "got: {err}");
        assert_eq!(get_task("t1").unwrap().plan.get("p1").unwrap().status, PItemStatus::WaitHumanGate);
    }

    /// [REQ-004][REQ-006][REQ-050] Declared `[Builds, TestsPass(failing)]` —
    /// the TestsPass criterion fails → mark_done must REJECT, leaving the
    /// P-item Failed, not Done. This is the spec's "TestsPass missing/failing →
    /// reject" behaviour (DEC-010) at the mark-done layer.
    #[test]
    fn req050_failing_testspass_rejects_mark_done() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem(
            "p1",
            vec![
                AcceptanceCriterion::Builds,
                AcceptanceCriterion::TestsPass("cargo test t_bad".into()),
            ],
            vec![ArtifactKind::TestOutput],
            false,
        );
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let err = mark_done("t1", "p1", &ok_summary(), &host).unwrap_err();
        assert!(err.starts_with("ACCEPTANCE_REJECTED"), "got: {err}");
        // P-item must be Failed(AcceptanceRejected), not Done.
        let task = get_task("t1").unwrap();
        assert_eq!(
            task.plan.get("p1").unwrap().status,
            PItemStatus::Failed(FailReason::AcceptanceRejected)
        );
    }

    /// [REQ-004][REQ-011] A `HumanReview` criterion (machine checks clean) must
    /// route to the AskUserQuestion path: status parked in WaitHumanGate, a
    /// WAIT_HUMAN error returned, and NO merge/Done.
    #[test]
    fn req011_human_review_routes_to_wait_human() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem(
            "p1",
            vec![AcceptanceCriterion::Builds, AcceptanceCriterion::HumanReview],
            vec![ArtifactKind::GitDiff],
            false,
        );
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let err = mark_done("t1", "p1", &ok_summary(), &host).unwrap_err();
        assert!(err.starts_with("WAIT_HUMAN"), "got: {err}");
        let task = get_task("t1").unwrap();
        assert_eq!(task.plan.get("p1").unwrap().status, PItemStatus::WaitHumanGate);
    }

    /// [REQ-011] The P-item human_gate flag alone (machine checks clean, no
    /// HumanReview criterion) forces WaitHuman — proves the gate, not the
    /// criterion, is what parks it.
    #[test]
    fn req011_human_gate_flag_forces_wait_human() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem("p1", vec![AcceptanceCriterion::Builds], vec![ArtifactKind::FileList], true);
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let err = mark_done("t1", "p1", &ok_summary(), &host).unwrap_err();
        assert!(err.starts_with("WAIT_HUMAN"), "got: {err}");
    }

    /// [REQ-013] artifact_kind 必选 — a P-item with empty `artifacts` is
    /// rejected by mark_done before any audit runs.
    #[test]
    fn req013_mark_done_rejects_missing_artifact_kind() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem("p1", vec![AcceptanceCriterion::Builds], vec![], false);
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let err = mark_done("t1", "p1", &ok_summary(), &host).unwrap_err();
        assert!(err.contains("REQ-013"), "got: {err}");
    }

    /// [REQ-027][DEC-007] A worker-declared `[DEVIATION]` touching the P-item's
    /// covered REQ creates a Pending entry AND blocks mark-done (retry-once /
    /// AskUserQuestion). The summary names REQ-027 which the deviation marker
    /// also targets, so the gate intersects.
    #[test]
    fn req027_pending_deviation_blocks_mark_done() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem("p1", vec![AcceptanceCriterion::Builds], vec![ArtifactKind::ManualNote], false);
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let summary = format!(
            "{}\n[DEVIATION] could not fully satisfy REQ-027, deferred the deviation gate retry",
            ok_summary()
        );
        let err = mark_done("t1", "p1", &summary, &host).unwrap_err();
        assert!(err.starts_with("PENDING_DEVIATION"), "got: {err}");
        // The deviation was recorded (REQ-026/042).
        let devs = read_deviations().unwrap();
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].status, DeviationStatus::Pending);
        assert!(devs[0].affected_req.contains(&"REQ-027".to_string()));
        // P-item stayed Running (not Done, not Failed) — master will retry.
        assert_eq!(get_task("t1").unwrap().plan.get("p1").unwrap().status, PItemStatus::Running);
    }

    /// [REQ-026][REQ-050][REQ-042] Happy path: all machine criteria pass, no
    /// deviation, no gate → Done, summary stored, decision logged with the
    /// per-criterion verdicts. merge_back has no worker branch → NoChanges.
    #[test]
    fn allpassed_marks_done_and_logs_decision() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem(
            "p1",
            vec![AcceptanceCriterion::Builds, AcceptanceCriterion::TestsPass("cargo test t_ok".into())],
            vec![ArtifactKind::TestOutput],
            false,
        );
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let outcome = mark_done("t1", "p1", &ok_summary(), &host).unwrap();
        assert_eq!(outcome, MergeOutcome::NoChanges);
        let task = get_task("t1").unwrap();
        let p = task.plan.get("p1").unwrap();
        assert_eq!(p.status, PItemStatus::Done);
        assert!(p.output_summary.is_some());
        // [REQ-042] structured decision log carries the acceptance verdicts.
        let decisions = read_master_decisions().unwrap();
        let md = decisions.last().unwrap();
        assert_eq!(md.kind, "mark-done");
        assert_eq!(md.decision.as_deref(), Some("allPassed"));
        assert!(md.acceptance.iter().any(|a| a.contains("Builds=pass")));
        assert!(md.acceptance.iter().any(|a| a.contains("pass")));
    }

    // ── [WA2] live adversarial-audit orchestration ──────────────────────────

    /// Fake auditor launcher: instead of spawning `claude`, it immediately
    /// writes the canned `AuditFinding[]` JSON the test wants each session to
    /// "produce" to that session's output path. `per_session_json[i]` is the
    /// JSON string for session i; `None` means "this session never writes"
    /// (simulating a timeout / spawn miss).
    struct FakeAuditorLauncher {
        per_session_json: Vec<Option<String>>,
    }
    impl AuditorLauncher for FakeAuditorLauncher {
        fn launch_auditor(
            &self,
            session_index: usize,
            _spec: &AuditorSpawnSpec,
            output_path: &std::path::Path,
        ) -> Result<u32, String> {
            if let Some(Some(json)) = self.per_session_json.get(session_index) {
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(output_path, json).unwrap();
            }
            // Return a fake pid; the orchestrator only needs the file to appear.
            Ok(100_000 + session_index as u32)
        }
    }

    fn proxy_only_finding_json() -> String {
        // A Critical no-proxy-signal finding, identical (same dedup_key) across
        // sessions so 2 of them collude to confirm it.
        r#"[{"severity":"critical","category":"no-proxy-signal","finding":"Master marked Done on proxy signals","evidence":"token count = 50000","reqAffected":["REQ-003","REQ-043"]}]"#.to_string()
    }

    /// [WA2] Two of three sessions report the same Critical → tally confirms it
    /// (2-of-3) → verdict CRITICAL_CONFIRMED, and the confirmed findings are
    /// written to the audits ledger. This is the "auditors collude on a real
    /// red-line violation" path end to end through the orchestrator.
    #[test]
    fn audit_pitem_two_of_three_critical_is_confirmed() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let launcher = FakeAuditorLauncher {
            per_session_json: vec![
                Some(proxy_only_finding_json()),
                Some(proxy_only_finding_json()),
                Some("[]".to_string()), // third auditor missed it
            ],
        };
        let outcome = audit_pitem(
            "t1",
            "p1",
            &launcher,
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(outcome.verdict, AuditVerdict::CriticalConfirmed(1));
        assert_eq!(outcome.verdict.verdict_line(), "AUDIT_VERDICT: CRITICAL_CONFIRMED 1");
        assert_eq!(outcome.confirmed.len(), 1);
        assert_eq!(outcome.confirmed[0].finding.category, "no-proxy-signal");
        assert_eq!(outcome.confirmed[0].votes, 2);
        // The confirmed findings were persisted to the audits ledger.
        assert!(outcome.written_to.exists(), "audit ledger file must be written");
        let written = fs::read_to_string(&outcome.written_to).unwrap();
        assert!(written.contains("no-proxy-signal"));
    }

    /// [WA2] All three sessions report `[]` (clean) → verdict CLEAN, ledger
    /// written with an empty confirmed list.
    #[test]
    fn audit_pitem_all_clean_is_clean() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let launcher = FakeAuditorLauncher {
            per_session_json: vec![
                Some("[]".to_string()),
                Some("[]".to_string()),
                Some("[]".to_string()),
            ],
        };
        let outcome = audit_pitem(
            "t1",
            "p1",
            &launcher,
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(outcome.verdict, AuditVerdict::Clean);
        assert_eq!(outcome.verdict.verdict_line(), "AUDIT_VERDICT: CLEAN");
        assert!(outcome.confirmed.is_empty());
        assert!(outcome.written_to.exists());
    }

    /// [WA2] Spec red line: "超时按失败处理不能假装通过". When a majority of
    /// auditor sessions never deliver (timeout / spawn miss), the verdict MUST
    /// be CRITICAL_CONFIRMED (a confirmed `auditor-timeout`), NOT CLEAN. Here
    /// only one session reports clean; the other two never write, so a
    /// SHORT timeout fires and the two misses collude into a Critical timeout.
    #[test]
    fn audit_pitem_timeout_is_failure_not_silent_pass() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let launcher = FakeAuditorLauncher {
            per_session_json: vec![
                Some("[]".to_string()), // session 0 reports clean
                None,                   // session 1 never delivers
                None,                   // session 2 never delivers
            ],
        };
        // Short timeout: the two missing sessions hit the deadline fast.
        let outcome = audit_pitem(
            "t1",
            "p1",
            &launcher,
            std::time::Duration::from_millis(300),
        )
        .unwrap();
        // 2 of 3 sessions failed → a confirmed Critical auditor-timeout.
        assert!(
            matches!(outcome.verdict, AuditVerdict::CriticalConfirmed(n) if n >= 1),
            "majority timeout must be CRITICAL_CONFIRMED, got {:?}",
            outcome.verdict
        );
        assert_ne!(outcome.verdict, AuditVerdict::Clean, "timeout must NOT pass");
        assert!(
            outcome.confirmed.iter().any(|c| c.finding.category == "auditor-timeout"),
            "a non-responding quorum must surface auditor-timeout: {:?}",
            outcome.confirmed
        );
    }

    /// [REQ-042] mark_failed writes a structured decision log entry.
    #[test]
    fn mark_failed_logs_decision() {
        let tmp_home = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp_home.path());
        let ws = compiling_workspace();
        let item = pitem("p1", vec![AcceptanceCriterion::Builds], vec![ArtifactKind::FileList], false);
        write_task("t1", ws.path(), item);
        let host = MockHost { workspace: ws.path().to_path_buf() };
        let _ = mark_failed("t1", "p1", FailReason::BuildFailed, &host).unwrap();
        let decisions = read_master_decisions().unwrap();
        let md = decisions.last().unwrap();
        assert_eq!(md.kind, "mark-failed");
        assert_eq!(md.decision.as_deref(), Some("failed"));
    }
}
