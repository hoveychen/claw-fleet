//! Sub-task recovery — which of a coordinator's sub-tasks died mid-turn and may
//! be woken again, and the guarded resume that wakes one.
//!
//! A task's main conversation (the coordinator) spawns sub-tasks with
//! `fleet__spawn`. When the network drops or an account error hits, the
//! coordinator is resumed by the usual mechanisms, but its sub-tasks stay dead
//! mid-turn. Waking all of them blindly is wrong: a sub-task that already handed
//! back its final report, or whose task the boss already closed, must never be
//! woken again. So every child is classified first ([`RecoveryClass`]) and only
//! [`RecoveryClass::Recoverable`] ones may be resumed — [`resume_child`]
//! re-probes right before acting so a stale listing cannot slip one through.
//!
//! The per-session verdict comes from [`crate::session_probe`]; this module only
//! maps it (plus the task-outcome record) onto a recovery decision.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::session_probe::{Phase, TailVerdict};

/// Prompt a recovered sub-task resumes with when the caller gives none.
/// Product text: the agent reads it as its next user turn.
pub const DEFAULT_RESUME_PROMPT: &str = "上一回合因网络/服务中断而中断，请从中断处继续你的任务。\
先核对已经做完的部分（git 状态、已有提交、已写的文件），不要重复已完成的工作。";

/// Mechanism name the resume shows up under in drive-lease logs and refusals.
const DRIVER: &str = "subtask_recovery";

/// What may be done about one sub-task.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryClass {
    /// Died mid-turn (process gone before the turn ended), or a server /
    /// network error whose retries are exhausted or were never registered, or
    /// a rate limit nothing is on record to resume. Safe to wake.
    Recoverable,
    /// The last turn ended cleanly — it handed back its result. Never woken.
    Done,
    /// A turn is in flight or something on record will start one.
    Running,
    /// A failure Fleet is still retrying by itself, or one still inside the
    /// probe's grace window. Leave it to the scheduler.
    Blocked,
    /// Waiting for the boss on a decision card / permission prompt.
    Parked,
    /// Cut off on purpose (Esc, SIGINT, `turn_aborted`). Not auto-woken: the
    /// interrupt may have been the point.
    UserInterrupted,
    /// Authentication / account / credit failure (403, OAuth expired, out of
    /// credits). Waking it would just fail again; listed so the boss sees it.
    AuthFailed,
    /// Any other permanent failure (invalid request, unknown model, remote
    /// workspace lost).
    Failed,
    /// The sub-task's own task was already closed (End / Abandon Task).
    Closed,
    /// The probe could not read the session's state.
    Unknown,
}

impl RecoveryClass {
    pub fn is_recoverable(self) -> bool {
        self == RecoveryClass::Recoverable
    }
}

/// One sub-task of a coordinator, as [`children_of`] reports it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubtaskStatus {
    /// The session the coordinator spawned.
    pub session_id: String,
    /// The live end of that sub-task's own relay chain (equal to
    /// `session_id` when it never handed off) — the session a resume targets.
    pub effective_id: String,
    /// The coordinator hop that spawned it.
    pub spawned_by: String,
    /// Plan step it was dispatched for (`fleet__spawn` `step`), when named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Engine (`claude` / `codex` / `dsh`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub phase: Phase,
    pub class: RecoveryClass,
    /// One line for a human: why this class. Product text (Chinese).
    pub detail: String,
}

/// Map one probe onto a recovery decision. Pure, so every row is testable.
///
/// - `tail`: the transcript-tail verdict the probe read (absent for dsh).
/// - `remote_lost`: the workspace's remote side is disconnected.
/// - `closed`: the sub-task's task carries a terminal outcome.
pub fn class_of(
    phase: Phase,
    tail: Option<&TailVerdict>,
    remote_lost: bool,
    closed: bool,
) -> RecoveryClass {
    if closed {
        return RecoveryClass::Closed;
    }
    match phase {
        Phase::Running => RecoveryClass::Running,
        Phase::Parked => RecoveryClass::Parked,
        Phase::Blocked => RecoveryClass::Blocked,
        Phase::Unknown => RecoveryClass::Unknown,
        Phase::Done => RecoveryClass::Done,
        Phase::Failed => {
            if remote_lost {
                return RecoveryClass::Failed;
            }
            match tail {
                Some(TailVerdict::InFlight)
                | Some(TailVerdict::ServerError)
                | Some(TailVerdict::RateLimited) => RecoveryClass::Recoverable,
                Some(TailVerdict::Interrupted) => RecoveryClass::UserInterrupted,
                Some(TailVerdict::Fatal(e)) if is_account_error(e) => RecoveryClass::AuthFailed,
                Some(TailVerdict::Fatal(_)) => RecoveryClass::Failed,
                // `classify` never turns these into Failed; be conservative if
                // it ever does.
                Some(TailVerdict::Ended) => RecoveryClass::Done,
                Some(TailVerdict::Empty) | None => RecoveryClass::Unknown,
            }
        }
    }
}

/// Whether a fatal API error text is about credentials or the account rather
/// than the request. Matches Claude Code's `error` enums (`authentication_failed`,
/// `oauth_org_not_allowed`, `billing_error`) and Codex's `codex_error_info`
/// (`usage_limit_exceeded`), plus the message texts seen in real transcripts
/// ("Request not allowed" is the 403).
fn is_account_error(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    [
        "authentication",
        "oauth",
        "not logged in",
        "/login",
        "request not allowed",
        "permission_error",
        "403",
        "401",
        "unauthorized",
        "billing",
        "credit",
        "usage_limit",
        "subscription",
    ]
    .iter()
    .any(|k| e.contains(k))
}

/// The sub-task ids spawned by any hop of `chain`, given every spawn edge
/// `(child, parent)`. Pure. Children that are themselves hops of the chain are
/// dropped, and a child whose own line was relayed counts once: `line_root`
/// maps a session to the first hop of its relay chain.
pub fn children_from_edges(
    chain: &[String],
    edges: impl IntoIterator<Item = (String, String)>,
    line_root: impl Fn(&str) -> String,
) -> Vec<(String, String)> {
    let hops: HashSet<&str> = chain.iter().map(String::as_str).collect();
    let mut seen_lines: HashSet<String> = HashSet::new();
    let mut out: Vec<(String, String)> = edges
        .into_iter()
        .filter(|(child, parent)| hops.contains(parent.as_str()) && !hops.contains(child.as_str()))
        .collect();
    out.sort();
    out.retain(|(child, _)| seen_lines.insert(line_root(child)));
    out
}

fn line_root(session_id: &str) -> String {
    crate::task_review::task_sessions(session_id)
        .into_iter()
        .next()
        .unwrap_or_else(|| session_id.to_string())
}

/// Whether any hop of `session_id`'s own task carries a terminal outcome.
fn task_closed(session_id: &str) -> bool {
    crate::task_review::task_sessions(session_id)
        .iter()
        .any(|id| crate::task_outcome::read(id).is_some())
}

/// Probe one sub-task and classify it.
fn status_of(child: &str, spawned_by: &str) -> SubtaskStatus {
    let facts = crate::session_probe::gather(child);
    let probe = crate::session_probe::classify(&facts, now_ms());
    let closed = task_closed(child);
    let class = class_of(
        probe.phase,
        facts.tail.as_ref(),
        facts.remote_disconnect.is_some(),
        closed,
    );
    let spec = crate::launch_spec::get(child);
    let detail = if closed {
        "所属任务已结束（老板已验收或放弃）".to_string()
    } else {
        probe.detail
    };
    SubtaskStatus {
        session_id: child.to_string(),
        effective_id: probe.effective_id,
        spawned_by: spawned_by.to_string(),
        step: spec.as_ref().and_then(|s| s.spawned_for_step.clone()),
        workspace: crate::launch_spec::get(&facts.effective_id)
            .and_then(|s| s.workspace)
            .or_else(|| spec.and_then(|s| s.workspace)),
        source: probe.source,
        phase: probe.phase,
        class,
        detail,
    }
}

/// Every sub-task spawned by any hop of `coordinator_session_id`'s relay chain,
/// each probed and classified. Ordered by session id.
pub fn children_of(coordinator_session_id: &str) -> Vec<SubtaskStatus> {
    let chain = crate::task_review::task_sessions(coordinator_session_id);
    let edges = crate::launch_spec::registry()
        .into_iter()
        .filter_map(|(id, spec)| spec.spawned_by_session_id.map(|p| (id, p)));
    children_from_edges(&chain, edges, line_root)
        .into_iter()
        .map(|(child, parent)| status_of(&child, &parent))
        .collect()
}

/// Why [`resume_child`] will not wake a session of this class. Agent-facing.
fn refusal(class: RecoveryClass) -> &'static str {
    match class {
        RecoveryClass::Recoverable => "",
        RecoveryClass::Done => {
            "its last turn ended cleanly — it already handed back its result, so it is never \
             woken again. To give it new work, spawn a new sub-task with fleet__spawn"
        }
        RecoveryClass::Running => "a turn is in flight (or about to start) — nothing to recover",
        RecoveryClass::Blocked => {
            "Fleet is still retrying it by itself (rate limit / server error), or the failure is \
             too recent to call — leave it to the scheduler"
        }
        RecoveryClass::Parked => "it is waiting for the boss on a decision card",
        RecoveryClass::UserInterrupted => {
            "it was interrupted on purpose (Esc / interrupt), not by a failure — ask the boss \
             before restarting it"
        }
        RecoveryClass::AuthFailed => {
            "its last turn failed on authentication / account access; resuming would fail the \
             same way until the account is fixed"
        }
        RecoveryClass::Failed => "its last turn hit a permanent error that a resume would not fix",
        RecoveryClass::Closed => "its task was already closed by the boss",
        RecoveryClass::Unknown => "its state could not be read, so it is not safe to wake",
    }
}

/// Wake one sub-task that died mid-turn. Re-probes first and refuses anything
/// that is not [`RecoveryClass::Recoverable`]; a [`RecoveryClass::Done`]
/// sub-task is always refused. `prompt` falls back to
/// [`DEFAULT_RESUME_PROMPT`] when empty. Returns a one-line `ok: …` report.
pub fn resume_child(child_id: &str, prompt: Option<&str>) -> Result<String, String> {
    let child = crate::session_probe::resolve_id(child_id)?;
    let parent = crate::launch_spec::spawned_by_of(&child).unwrap_or_default();
    let st = status_of(&child, &parent);
    let short = short_id(&child);
    if !st.class.is_recoverable() {
        return Err(format!(
            "refused to resume {short}: {} (class {:?}; {})",
            refusal(st.class),
            st.class,
            st.detail
        ));
    }
    let target = st.effective_id.clone();
    if !crate::launch_spec::was_fleet_spawned(&target) {
        return Err(format!(
            "refused to resume {short}: {} was not started by Fleet, so it cannot be resumed \
             headlessly without risking a second process on someone's transcript",
            short_id(&target)
        ));
    }
    let workspace = st
        .workspace
        .clone()
        .ok_or_else(|| format!("refused to resume {short}: no workspace recorded for it"))?;
    let prompt = prompt
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(DEFAULT_RESUME_PROMPT);
    let model = crate::session::resolve_session_model_spec(&target)
        .or_else(|| crate::launch_spec::model_of(&target));
    let spec = crate::agent_source::ResumeSpec {
        session_id: target.clone(),
        workspace_path: workspace,
        prompt: prompt.to_string(),
        model,
        effort: crate::launch_spec::effort_of(&target),
        // A recovery must not re-grant an elevated mode.
        permission_mode: None,
        images: Vec::new(),
    };
    crate::agent_source::resume_session(
        st.source.as_deref().unwrap_or("claude"),
        &spec,
        crate::session_driver::Driver::continue_(DRIVER),
        Box::new(|_| {}),
    )
    .map_err(|e| format!("could not resume {short}: {e}"))?;
    let via = if target == child {
        String::new()
    } else {
        format!(
            " (via the live end of its relay chain, {})",
            short_id(&target)
        )
    };
    Ok(format!("ok: resumed {short}{via}; it was: {}", st.detail))
}

/// Body of the list call every client makes (`sessionId` = any hop of the
/// coordinator's chain).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubtaskListRequest {
    pub session_id: String,
}

/// Body of the batch resume every client makes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResumeChildrenRequest {
    pub session_ids: Vec<String>,
    /// Falls back to [`DEFAULT_RESUME_PROMPT`] when absent or blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// What happened to one id of a [`resume_children`] batch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOutcome {
    pub session_id: String,
    pub ok: bool,
    /// [`resume_child`]'s `ok: …` report, or its refusal / error.
    pub message: String,
}

/// Resume a batch of sub-tasks — the manual "recover interrupted sub-tasks"
/// action. Each id goes through [`resume_child`] on its own, so a refusal on
/// one never blocks the rest. Blank and repeated ids are dropped; order is
/// kept.
pub fn resume_children(session_ids: &[String], prompt: Option<&str>) -> Vec<ResumeOutcome> {
    resume_children_with(session_ids, |id| resume_child(id, prompt))
}

fn resume_children_with(
    session_ids: &[String],
    mut resume: impl FnMut(&str) -> Result<String, String>,
) -> Vec<ResumeOutcome> {
    let mut seen: HashSet<&str> = HashSet::new();
    session_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && seen.insert(id))
        .map(|id| {
            let (ok, message) = match resume(id) {
                Ok(m) => (true, m),
                Err(m) => (false, m),
            };
            ResumeOutcome {
                session_id: id.to_string(),
                ok,
                message,
            }
        })
        .collect()
}

fn short_id(id: &str) -> &str {
    let id = id.strip_prefix("session-").unwrap_or(id);
    id.get(..8).unwrap_or(id)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_PHASES: [Phase; 6] = [
        Phase::Running,
        Phase::Parked,
        Phase::Blocked,
        Phase::Unknown,
        Phase::Done,
        Phase::Failed,
    ];

    fn all_tails() -> Vec<Option<TailVerdict>> {
        vec![
            None,
            Some(TailVerdict::Ended),
            Some(TailVerdict::InFlight),
            Some(TailVerdict::Interrupted),
            Some(TailVerdict::RateLimited),
            Some(TailVerdict::ServerError),
            Some(TailVerdict::Fatal(
                "authentication_failed: Request not allowed".into(),
            )),
            Some(TailVerdict::Fatal("invalid_request: bad".into())),
            Some(TailVerdict::Empty),
        ]
    }

    #[test]
    fn done_is_never_recoverable() {
        for tail in all_tails() {
            for remote in [false, true] {
                for closed in [false, true] {
                    let c = class_of(Phase::Done, tail.as_ref(), remote, closed);
                    assert!(!c.is_recoverable(), "Done with {tail:?} became {c:?}");
                }
            }
        }
        // A clean tail never reads as recoverable whatever the phase.
        for phase in ALL_PHASES {
            assert!(!class_of(phase, Some(&TailVerdict::Ended), false, false).is_recoverable());
        }
    }

    #[test]
    fn closed_tasks_are_never_recoverable() {
        for phase in ALL_PHASES {
            for tail in all_tails() {
                assert_eq!(
                    class_of(phase, tail.as_ref(), false, true),
                    RecoveryClass::Closed
                );
            }
        }
    }

    #[test]
    fn only_failed_mid_turn_or_transient_errors_are_recoverable() {
        let f = |t: TailVerdict| class_of(Phase::Failed, Some(&t), false, false);
        assert_eq!(f(TailVerdict::InFlight), RecoveryClass::Recoverable);
        assert_eq!(f(TailVerdict::ServerError), RecoveryClass::Recoverable);
        assert_eq!(f(TailVerdict::RateLimited), RecoveryClass::Recoverable);
        assert_eq!(f(TailVerdict::Interrupted), RecoveryClass::UserInterrupted);
        assert_eq!(
            f(TailVerdict::Fatal(
                "authentication_failed: Failed to authenticate. API Error: 403 Request not allowed"
                    .into()
            )),
            RecoveryClass::AuthFailed
        );
        assert_eq!(
            f(TailVerdict::Fatal(
                "oauth_org_not_allowed: Your organization has disabled".into()
            )),
            RecoveryClass::AuthFailed
        );
        assert_eq!(
            f(TailVerdict::Fatal(
                "usage_limit_exceeded: out of credits".into()
            )),
            RecoveryClass::AuthFailed
        );
        assert_eq!(
            f(TailVerdict::Fatal(
                "invalid_request: safeguards flagged".into()
            )),
            RecoveryClass::Failed
        );
        assert_eq!(f(TailVerdict::Empty), RecoveryClass::Unknown);
        // A lost remote workspace is not fixed by a resume.
        assert_eq!(
            class_of(Phase::Failed, Some(&TailVerdict::InFlight), true, false),
            RecoveryClass::Failed
        );
    }

    #[test]
    fn non_failed_phases_are_left_alone() {
        for tail in all_tails() {
            assert_eq!(
                class_of(Phase::Running, tail.as_ref(), false, false),
                RecoveryClass::Running
            );
            assert_eq!(
                class_of(Phase::Parked, tail.as_ref(), false, false),
                RecoveryClass::Parked
            );
            // Inside the grace window / retries pending: the scheduler owns it.
            assert_eq!(
                class_of(Phase::Blocked, tail.as_ref(), false, false),
                RecoveryClass::Blocked
            );
            assert_eq!(
                class_of(Phase::Unknown, tail.as_ref(), false, false),
                RecoveryClass::Unknown
            );
        }
    }

    #[test]
    fn every_non_recoverable_class_has_a_refusal() {
        for c in [
            RecoveryClass::Done,
            RecoveryClass::Running,
            RecoveryClass::Blocked,
            RecoveryClass::Parked,
            RecoveryClass::UserInterrupted,
            RecoveryClass::AuthFailed,
            RecoveryClass::Failed,
            RecoveryClass::Closed,
            RecoveryClass::Unknown,
        ] {
            assert!(!refusal(c).is_empty(), "{c:?}");
        }
        assert!(refusal(RecoveryClass::Done).contains("never woken"));
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn children_cover_every_hop_of_the_coordinator_chain() {
        // Coordinator chain c1 -> c2 (handoff). c1 spawned a, c2 spawned b;
        // x belongs to another coordinator; c2 is a hop, not a child, even if
        // a note says c1 spawned it.
        let chain = vec![s("c1"), s("c2")];
        let edges = vec![
            (s("a"), s("c1")),
            (s("b"), s("c2")),
            (s("x"), s("other")),
            (s("c2"), s("c1")),
        ];
        let got = children_from_edges(&chain, edges, |id| id.to_string());
        assert_eq!(got, vec![(s("a"), s("c1")), (s("b"), s("c2"))]);
    }

    #[test]
    fn a_relayed_child_line_counts_once() {
        // Child a handed off to a2, and both somehow carry the spawn edge.
        let chain = vec![s("c1")];
        let edges = vec![(s("a2"), s("c1")), (s("a"), s("c1")), (s("b"), s("c1"))];
        let root = |id: &str| if id == "a2" { s("a") } else { id.to_string() };
        let got = children_from_edges(&chain, edges, root);
        assert_eq!(got, vec![(s("a"), s("c1")), (s("b"), s("c1"))]);
    }

    #[test]
    fn status_round_trips_through_json() {
        let st = SubtaskStatus {
            session_id: s("a"),
            effective_id: s("a"),
            spawned_by: s("c1"),
            step: Some(s("P1")),
            workspace: None,
            source: Some(s("claude")),
            phase: Phase::Failed,
            class: RecoveryClass::Recoverable,
            detail: s("进程在回合中途退出"),
        };
        let v = serde_json::to_value(&st).unwrap();
        assert_eq!(v["class"], "recoverable");
        assert_eq!(v["effectiveId"], "a");
        let back: SubtaskStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back, st);
    }

    #[test]
    fn batch_resume_reports_each_id_and_skips_blank_and_repeats() {
        let ids = vec![s("a"), s(" "), s("b"), s("a"), s(""), s(" c ")];
        let mut called = Vec::new();
        let got = resume_children_with(&ids, |id| {
            called.push(id.to_string());
            if id == "b" {
                Err(format!("refused to resume {id}: done"))
            } else {
                Ok(format!("ok: resumed {id}"))
            }
        });
        assert_eq!(called, vec![s("a"), s("b"), s("c")]);
        assert_eq!(
            got,
            vec![
                ResumeOutcome { session_id: s("a"), ok: true, message: s("ok: resumed a") },
                ResumeOutcome {
                    session_id: s("b"),
                    ok: false,
                    message: s("refused to resume b: done")
                },
                ResumeOutcome { session_id: s("c"), ok: true, message: s("ok: resumed c") },
            ]
        );
    }

    #[test]
    fn requests_read_camel_case_and_prompt_is_optional() {
        let r: ResumeChildrenRequest =
            serde_json::from_value(serde_json::json!({"sessionIds": ["a"]})).unwrap();
        assert_eq!(r.session_ids, vec![s("a")]);
        assert_eq!(r.prompt, None);
        let l: SubtaskListRequest =
            serde_json::from_value(serde_json::json!({"sessionId": "c1"})).unwrap();
        assert_eq!(l.session_id, "c1");
        let v = serde_json::to_value(ResumeOutcome {
            session_id: s("a"),
            ok: false,
            message: s("x"),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"sessionId": "a", "ok": false, "message": "x"}));
    }
}
