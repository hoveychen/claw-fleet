//! Session leases — resources an agent session holds across commands and
//! turns (a dev board, a phone on the desk), not just for one command.
//!
//! A lease is an ordinary [`ResourceRequest`] of kind [`RequestKind::Lease`]
//! in the one queue, so it is scheduled, shown and logged like everything
//! else. What differs is who stands behind it: not a process but a session.
//!
//! * `take` — grant now or refuse (nothing is left queued);
//! * `queue` — wait in line, optionally not before `--after`; a waiter polls
//!   `mine` (exit 0 once it holds) from `fleet watch --until`;
//! * `release` / `unqueue` — let go (of everything, or `--need` just some) /
//!   leave the line;
//! * a session holding some may take or queue for more (an append): each
//!   addition is its own lease piece, and a wait for more while holding some
//!   is refused when it could close a deadlock cycle ([`super::deadlock`]).
//!
//! **Ending.** A lease is dropped once its session has been found with nothing
//! keeping it going — no Fleet-known process, live watch, schedule or loop,
//! pending card, registered handoff or queued resume — for [`GRACE`]. The
//! check is the plan reviver's ([`crate::plan_revive::coverage_for`]), so "the
//! session ended" means the same thing everywhere in Fleet. A session that
//! relayed its work is followed to the end of its handoff chain: the lease
//! moves to the successor instead (also done eagerly at handoff time,
//! [`transfer_session`]).
//!
//! The lookup reads several stores, so it runs outside the state lock and at
//! most every [`check_interval_ms`] across all processes sharing the file
//! (`State::leases_checked_ms`); a lease is only ever dropped by a pass that
//! has just looked.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{
    describe_wait, format_needs, now_ms, state_path, transact, transact_as, Cause, Holder,
    Priority, RequestKind, ResourceRequest, State, WaitOn,
};

/// How long a lease survives its session having nothing going on.
pub const GRACE: Duration = Duration::from_secs(120);

/// Overrides [`GRACE`], in seconds (tests and e2e runs use 0).
pub const GRACE_ENV: &str = "FLEET_LEASE_GRACE_SECS";

/// [`GRACE`] in ms, or [`GRACE_ENV`] when set.
pub fn grace_ms() -> u64 {
    std::env::var(GRACE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| s * 1000)
        .unwrap_or(GRACE.as_millis() as u64)
}

/// How often the sessions behind leases are looked up: every 10s, or as often
/// as the grace when that is shorter.
pub fn check_interval_ms(grace_ms: u64) -> u64 {
    grace_ms.min(10_000)
}

/// Where the sessions behind a set of leases stand.
#[derive(Debug, Default, Clone)]
pub(super) struct Facts {
    /// The lease owners that were looked up.
    pub sessions: BTreeSet<String>,
    /// Owner → the session at the end of its handoff chain, for owners that
    /// handed their work on.
    pub successor: BTreeMap<String, String>,
    /// Sessions (owners or successors) with something keeping them going.
    pub engaged: BTreeSet<String>,
}

#[cfg(test)]
thread_local! {
    /// Test seam: what [`facts_due`] returns instead of reading the real
    /// stores. Each call receives the lease owners.
    pub(super) static FAKE_FACTS: std::cell::RefCell<Option<Box<dyn Fn(&BTreeSet<String>) -> Facts>>> =
        const { std::cell::RefCell::new(None) };
}

/// The sessions behind the leases in `path`'s state, if a lookup is due.
/// Reads the file without the lock (a stale read only means a later check).
pub(super) fn facts_due(path: &Path) -> Option<Facts> {
    let raw = std::fs::read(path).ok()?;
    let state: State = serde_json::from_slice(&raw).ok()?;
    let sessions: BTreeSet<String> = state
        .grants
        .iter()
        .chain(&state.queue)
        .filter(|r| r.is_lease())
        .filter_map(|r| r.session_id.clone())
        .collect();
    if sessions.is_empty() {
        return None;
    }
    let now = now_ms();
    if now < state.leases_checked_ms + check_interval_ms(grace_ms()) {
        return None;
    }
    #[cfg(test)]
    {
        FAKE_FACTS.with(|f| f.borrow().as_ref().map(|f| f(&sessions)))
    }
    #[cfg(not(test))]
    {
        Some(gather(sessions))
    }
}

/// Look the sessions up in the real stores.
#[cfg_attr(test, allow(dead_code))]
fn gather(sessions: BTreeSet<String>) -> Facts {
    let index = crate::handoff::successor_index();
    let mut successor = BTreeMap::new();
    for s in &sessions {
        let mut tail = s.clone();
        // Bounded: a corrupt store must not loop forever.
        for _ in 0..64 {
            match index.get(&tail) {
                Some(next) if next != &tail => tail = next.clone(),
                _ => break,
            }
        }
        if &tail != s {
            successor.insert(s.clone(), tail);
        }
    }
    let owners: HashSet<String> = sessions.iter().chain(successor.values()).cloned().collect();
    let cov = crate::plan_revive::coverage_for(&owners);
    let engaged = owners
        .into_iter()
        .filter(|o| cov.covering(std::slice::from_ref(o)).is_some())
        .collect();
    Facts { sessions, successor, engaged }
}

/// Apply `facts` (when a lookup ran) to the leases in `state`: move handed-on
/// leases to their successor, mark idle ones, and drop those idle for `grace`.
/// Returns the ids dropped.
pub(super) fn settle(state: &mut State, facts: Option<&Facts>, now: u64, grace: u64) -> BTreeSet<String> {
    let mut lapsed = BTreeSet::new();
    let Some(f) = facts else { return lapsed };
    state.leases_checked_ms = now;
    for r in state.grants.iter_mut().chain(state.queue.iter_mut()) {
        if !r.is_lease() {
            continue;
        }
        let Some(sid) = r.session_id.clone() else { continue };
        if !f.sessions.contains(&sid) {
            // Changed hands since the lookup; the next one covers it.
            continue;
        }
        let owner = match f.successor.get(&sid) {
            Some(next) => {
                r.session_id = Some(next.clone());
                next.clone()
            }
            None => sid,
        };
        if f.engaged.contains(&owner) {
            r.idle_since_ms = None;
        } else if r.idle_since_ms.is_none() {
            r.idle_since_ms = Some(now);
        }
        if r.idle_since_ms.is_some_and(|t| now >= t + grace) {
            lapsed.insert(r.id.clone());
        }
    }
    if !lapsed.is_empty() {
        state.grants.retain(|r| !lapsed.contains(&r.id));
        state.queue.retain(|r| !lapsed.contains(&r.id));
    }
    lapsed
}

// ─────────────────────────────────────────────────────────────────────────────
// API
// ─────────────────────────────────────────────────────────────────────────────

/// A lease to take or queue for.
#[derive(Debug, Clone)]
pub struct LeaseSpec {
    /// The owning session (full id).
    pub session_id: String,
    pub needs: BTreeMap<String, u32>,
    pub priority: Priority,
    /// `queue` only: not granted before this (epoch ms).
    pub after_ms: Option<u64>,
    /// What the session does with it, shown to everyone.
    pub note: Option<String>,
    pub workspace: Option<String>,
}

/// A session's queued lease piece, while the session holds others.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PendingLease {
    pub lease: ResourceRequest,
    pub position: usize,
    pub wait_on: Vec<WaitOn>,
}

/// A session's lease, as `mine` / `take` / `queue` report it.
///
/// A session may hold several lease pieces (each take or granted queue that
/// added resources is its own piece) and wait for at most one more.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LeaseStatus {
    /// The session holds `needs` (every piece together); `lease` is the
    /// oldest piece, kept for readers of the one-lease shape. `pending`: a
    /// piece it waits for on top.
    #[serde(rename_all = "camelCase")]
    Held {
        lease: ResourceRequest,
        #[serde(default)]
        leases: Vec<ResourceRequest>,
        #[serde(default)]
        needs: BTreeMap<String, u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending: Option<PendingLease>,
    },
    /// The session waits in the queue and holds nothing.
    #[serde(rename_all = "camelCase")]
    Queued { lease: ResourceRequest, position: usize, wait_on: Vec<WaitOn> },
    /// `take` only: not available now, and nothing was queued.
    #[serde(rename_all = "camelCase")]
    Busy { wait_on: Vec<WaitOn> },
    /// The session neither holds nor waits for anything.
    None,
}

impl LeaseStatus {
    /// Holds something and waits for nothing more.
    pub fn is_held(&self) -> bool {
        matches!(self, Self::Held { pending: None, .. })
    }

    /// Waits in the queue — for a first lease, or for more on top of one.
    pub fn waiting(&self) -> bool {
        matches!(self, Self::Queued { .. } | Self::Held { pending: Some(_), .. })
    }

    /// Everything the session holds.
    pub fn held_needs(&self) -> BTreeMap<String, u32> {
        match self {
            Self::Held { needs, lease, .. } if needs.is_empty() => lease.needs.clone(),
            Self::Held { needs, .. } => needs.clone(),
            _ => BTreeMap::new(),
        }
    }

    /// The session holds all of `needs`.
    pub fn holds(&self, needs: &BTreeMap<String, u32>) -> bool {
        matches!(self, Self::Held { .. }) && holds_all(&self.held_needs(), needs)
    }

    /// One line for the CLI and the MCP tool.
    pub fn describe(&self) -> String {
        let piece = |l: &ResourceRequest| {
            format!(
                "lease {} for [{}]{}",
                l.id,
                format_needs(&l.needs),
                l.note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default()
            )
        };
        match self {
            Self::Held { lease, leases, needs, pending } => {
                let mut s = if leases.len() > 1 {
                    let parts: Vec<String> = leases.iter().map(piece).collect();
                    format!("holding [{}] as {}", format_needs(needs), parts.join(", "))
                } else {
                    format!("holding {}", piece(lease))
                };
                if let Some(p) = pending {
                    s.push_str(&format!(
                        "; queued (#{}) as lease {} for [{}] on top — waiting on {}",
                        p.position,
                        p.lease.id,
                        format_needs(&p.lease.needs),
                        describe_wait(&p.wait_on)
                    ));
                }
                s
            }
            Self::Queued { lease, position, wait_on } => format!(
                "queued (#{position}) as lease {} for [{}] — waiting on {}",
                lease.id,
                format_needs(&lease.needs),
                describe_wait(wait_on)
            ),
            Self::Busy { wait_on } => format!(
                "not available now — {}; nothing was queued (use queue to wait in line)",
                describe_wait(wait_on)
            ),
            Self::None => "this session holds no lease and is not queued for one".into(),
        }
    }
}

/// One lease piece [`release`] let go of, whole or in part.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReleasedPart {
    pub id: String,
    /// What was let go of.
    pub needs: BTreeMap<String, u32>,
    /// Held (released), or only queued (left the queue).
    pub was_held: bool,
    /// The piece is gone; otherwise it was shrunk and keeps the rest.
    pub whole: bool,
}

/// What [`release`] let go of.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LeaseRelease {
    pub released: Vec<ReleasedPart>,
    /// What the session still holds.
    pub kept: BTreeMap<String, u32>,
    /// What the session still waits for.
    pub still_queued: BTreeMap<String, u32>,
    /// Requests granted by the same pass, i.e. who got the resources next.
    pub handed_to: Vec<ResourceRequest>,
}

impl LeaseRelease {
    fn nothing() -> Self {
        Self {
            released: Vec::new(),
            kept: BTreeMap::new(),
            still_queued: BTreeMap::new(),
            handed_to: Vec::new(),
        }
    }

    /// One line for the CLI and the MCP tool.
    pub fn describe(&self) -> String {
        if self.released.is_empty() {
            return "ok: nothing of that is held or queued by this session — nothing to do.".into();
        }
        let parts: Vec<String> = self
            .released
            .iter()
            .map(|p| {
                let what = match (p.was_held, p.whole) {
                    (true, true) => "released",
                    (true, false) => "released (rest of the lease kept)",
                    (false, true) => "left the queue",
                    (false, false) => "no longer queued for",
                };
                format!("{} [{}] {what}", p.id, format_needs(&p.needs))
            })
            .collect();
        let mut s = format!("ok: {}", parts.join("; "));
        if !self.kept.is_empty() {
            s.push_str(&format!("; still holding [{}]", format_needs(&self.kept)));
        }
        if !self.still_queued.is_empty() {
            s.push_str(&format!("; still queued for [{}]", format_needs(&self.still_queued)));
        }
        if !self.handed_to.is_empty() {
            let to: Vec<String> = self
                .handed_to
                .iter()
                .map(|r| match (&r.session_id, r.is_lease()) {
                    (Some(s), true) => format!("{} (session {})", r.id, &s[..s.len().min(8)]),
                    _ => r.id.clone(),
                })
                .collect();
            s.push_str(&format!("; now granted: {}", to.join(", ")));
        }
        s.push('.');
        s
    }
}

/// A fresh lease id (`l…`, so it reads apart from command requests' `r…`).
fn new_lease_id() -> String {
    let id = super::new_request_id();
    format!("l{}", &id[1..])
}

/// Resolve `session` (a full id, or a prefix of at least 8 characters) to the
/// owner of a lease in `state`. Anything that matches no lease is returned as
/// given — the caller may be about to create one.
fn resolve(state: &State, session: &str) -> String {
    let owners: BTreeSet<&str> = state
        .grants
        .iter()
        .chain(&state.queue)
        .filter(|r| r.is_lease())
        .filter_map(|r| r.session_id.as_deref())
        .collect();
    if owners.contains(session) || session.len() < 8 {
        return session.to_string();
    }
    let mut hits = owners.iter().filter(|o| o.starts_with(session));
    match (hits.next(), hits.next()) {
        (Some(one), None) => one.to_string(),
        _ => session.to_string(),
    }
}

/// The session's queued lease piece (the first, should a handoff have
/// brought it a second).
fn queued_piece<'a>(state: &'a State, session: &str) -> Option<&'a ResourceRequest> {
    state
        .queue
        .iter()
        .find(|r| r.is_lease() && r.session_id.as_deref() == Some(session))
}

/// The session's lease as it stands. Call after a scheduling pass.
fn status_in(state: &State, session: &str, now: u64) -> LeaseStatus {
    let held: Vec<ResourceRequest> = state.session_leases(session).into_iter().cloned().collect();
    let queued = queued_piece(state, session).and_then(|r| {
        state
            .snapshot(now)
            .queue
            .into_iter()
            .find(|q| q.request.id == r.id)
            .map(|q| PendingLease { lease: q.request, position: q.position, wait_on: q.wait_on })
    });
    match (held.first().cloned(), queued) {
        (Some(lease), pending) => LeaseStatus::Held {
            lease,
            needs: state.session_holds(session),
            leases: held,
            pending,
        },
        (None, Some(p)) => LeaseStatus::Queued { lease: p.lease, position: p.position, wait_on: p.wait_on },
        (None, None) => LeaseStatus::None,
    }
}

fn holds_all(held: &BTreeMap<String, u32>, needs: &BTreeMap<String, u32>) -> bool {
    needs.iter().all(|(n, &u)| held.get(n).copied().unwrap_or(0) >= u)
}

/// What of `wanted` the session does not hold yet. Asking for more units of a
/// resource it holds some of is refused: two pieces of one resource would let
/// a session wait for a resource while holding it.
fn extra_needs(
    held: &BTreeMap<String, u32>,
    wanted: &BTreeMap<String, u32>,
) -> Result<BTreeMap<String, u32>, String> {
    let mut out = BTreeMap::new();
    for (name, &n) in wanted {
        match held.get(name).copied().unwrap_or(0) {
            0 => {
                out.insert(name.clone(), n);
            }
            h if h >= n => {}
            h => {
                return Err(format!(
                    "this session holds {name}:{h} and asked for {name}:{n} — growing a held \
                     resource is not supported; release {name} and take {name}:{n}"
                ))
            }
        }
    }
    Ok(out)
}

fn overlaps(a: &BTreeMap<String, u32>, b: &BTreeMap<String, u32>) -> bool {
    a.keys().any(|k| b.contains_key(k))
}

/// Why waiting for `want` is refused: the cycle it would close.
fn refuse_deadlock(state: &State, session: &str, want: &BTreeMap<String, u32>, cycle: &[super::deadlock::Step]) -> String {
    let held = state.session_holds(session);
    let holding = if held.is_empty() {
        "while this session's commands wait".to_string()
    } else {
        format!("while this session holds [{}]", format_needs(&held))
    };
    format!(
        "refused: waiting for [{}] {holding} would deadlock — {}. Release what you hold first \
         (fleet resource release --need …) and queue for everything at once, or wait until the \
         other side is done.",
        format_needs(want),
        super::deadlock::explain(cycle)
    )
}

fn new_request(spec: &LeaseSpec, now: u64) -> ResourceRequest {
    ResourceRequest {
        id: new_lease_id(),
        kind: RequestKind::Lease,
        needs: spec.needs.clone(),
        priority: spec.priority,
        enqueued_ms: now,
        granted_ms: None,
        manual: false,
        holder: Holder { pid: 0, start_time: None },
        command: String::new(),
        workspace: spec.workspace.clone(),
        session_id: Some(spec.session_id.clone()),
        after_ms: spec.after_ms,
        note: spec.note.clone(),
        skips: 0,
        idle_since_ms: None,
    }
}

fn require_session(session: &str) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("no session id: a lease belongs to an agent session (run this from one, \
                    or pass --session)"
            .into());
    }
    Ok(())
}

/// Take `spec.needs` now, or refuse ([`LeaseStatus::Busy`]) without queueing.
///
/// Already holding all of it is a no-op. What the session already holds is
/// kept; the rest is taken as a new lease piece (an append) — refused if,
/// with the session's other waits, it would close a deadlock cycle. A session
/// queued for some of it is granted that queued piece out of turn if the queue
/// allows it (its `--after` is ignored), else stays queued as it was.
pub fn take(spec: &LeaseSpec) -> Result<LeaseStatus, String> {
    take_at(&state_path()?, spec)
}

pub(crate) fn take_at(path: &Path, spec: &LeaseSpec) -> Result<LeaseStatus, String> {
    require_session(&spec.session_id)?;
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&super::holder_alive, now);
        s.check_fits(&spec.needs)?;
        let session = spec.session_id.as_str();
        let want = extra_needs(&s.session_holds(session), &spec.needs)?;
        if want.is_empty() {
            return Ok(status_in(s, session, now));
        }
        let backup = s.clone();
        let queued = queued_piece(s, session).filter(|q| overlaps(&q.needs, &want)).map(|q| q.id.clone());
        if let Some(qid) = queued {
            if let Some(r) = s.queue.iter_mut().find(|r| r.id == qid) {
                r.after_ms = None;
                let mut merged = r.needs.clone();
                for (name, &n) in &want {
                    let e = merged.entry(name.clone()).or_insert(0);
                    *e = (*e).max(n);
                }
                if r.needs != merged {
                    r.needs = merged;
                    r.skips = 0;
                }
                if spec.note.is_some() {
                    r.note = spec.note.clone();
                }
            }
            s.schedule(&super::holder_alive, now);
            if s.grants.iter().any(|g| g.id == qid) {
                if let Some(cycle) = super::deadlock::cycle_through(s, &qid) {
                    let msg = refuse_deadlock(&backup, session, &want, &cycle);
                    *s = backup;
                    return Err(msg);
                }
                return Ok(status_in(s, session, now));
            }
            *s = backup;
            return Ok(status_in(s, session, now));
        }
        let req = new_request(&LeaseSpec { after_ms: None, needs: want.clone(), ..spec.clone() }, now);
        let id = req.id.clone();
        s.enqueue(req)?;
        s.schedule(&super::holder_alive, now);
        if s.grants.iter().any(|g| g.id == id) {
            if let Some(cycle) = super::deadlock::cycle_through(s, &id) {
                let msg = refuse_deadlock(&backup, session, &want, &cycle);
                *s = backup;
                return Err(msg);
            }
            return Ok(status_in(s, session, now));
        }
        let wait_on = s
            .snapshot(now)
            .queue
            .into_iter()
            .find(|q| q.request.id == id)
            .map(|q| q.wait_on)
            .unwrap_or_default();
        *s = backup;
        Ok(LeaseStatus::Busy { wait_on })
    })?
}

/// Queue for `spec.needs` (granted at once if nothing stands in the way).
///
/// What the session already holds is kept, and it queues for the rest — a
/// wait on top of what it holds, refused if it could close a deadlock cycle.
/// Queueing again updates the queued piece in place, keeping its place in line.
pub fn queue(spec: &LeaseSpec) -> Result<LeaseStatus, String> {
    queue_at(&state_path()?, spec)
}

pub(crate) fn queue_at(path: &Path, spec: &LeaseSpec) -> Result<LeaseStatus, String> {
    require_session(&spec.session_id)?;
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&super::holder_alive, now);
        s.check_fits(&spec.needs)?;
        let session = spec.session_id.as_str();
        let want = extra_needs(&s.session_holds(session), &spec.needs)?;
        if want.is_empty() {
            return Ok(status_in(s, session, now));
        }
        let backup = s.clone();
        let id = match queued_piece(s, session).map(|q| q.id.clone()) {
            Some(qid) => {
                for name in want.keys() {
                    s.resources
                        .entry(name.clone())
                        .or_insert(super::ResourceDef { capacity: super::DEFAULT_CAPACITY });
                }
                if let Some(r) = s.queue.iter_mut().find(|r| r.id == qid) {
                    if r.needs != want {
                        r.needs = want.clone();
                        r.skips = 0;
                    }
                    r.after_ms = spec.after_ms;
                    if spec.note.is_some() {
                        r.note = spec.note.clone();
                    }
                }
                qid
            }
            None => {
                let req = new_request(&LeaseSpec { needs: want.clone(), ..spec.clone() }, now);
                let id = req.id.clone();
                s.enqueue(req)?;
                id
            }
        };
        s.schedule(&super::holder_alive, now);
        if let Some(cycle) = super::deadlock::cycle_through(s, &id) {
            let msg = refuse_deadlock(&backup, session, &want, &cycle);
            *s = backup;
            return Err(msg);
        }
        Ok(status_in(s, session, now))
    })?
}

/// The session's lease: held (with anything it waits for on top), queued
/// (with its place and what it waits on), or none.
pub fn status(session: &str) -> Result<LeaseStatus, String> {
    status_at(&state_path()?, session)
}

pub(crate) fn status_at(path: &Path, session: &str) -> Result<LeaseStatus, String> {
    require_session(session)?;
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&super::holder_alive, now);
        let owner = resolve(s, session);
        status_in(s, &owner, now)
    })
}

/// Let go of the session's leases — held or queued. `only`: just these
/// resources (units across its pieces; a piece left with nothing goes, one
/// left with something keeps it); `None`: everything. `by` is who asks;
/// anyone but the owner releases on its behalf (logged as kicked).
pub fn release(session: &str, by: Option<&str>, only: Option<&BTreeMap<String, u32>>) -> Result<LeaseRelease, String> {
    release_at(&state_path()?, session, by, only)
}

pub(crate) fn release_at(
    path: &Path,
    session: &str,
    by: Option<&str>,
    only: Option<&BTreeMap<String, u32>>,
) -> Result<LeaseRelease, String> {
    require_session(session)?;
    let found = transact(path, |s| {
        let owner = resolve(s, session);
        let ids: Vec<String> = s
            .grants
            .iter()
            .chain(&s.queue)
            .filter(|r| r.is_lease() && r.session_id.as_deref() == Some(owner.as_str()))
            .map(|r| r.id.clone())
            .collect();
        (owner, ids)
    })?;
    let (owner, ids) = found;
    if ids.is_empty() {
        return Ok(LeaseRelease::nothing());
    }
    let by_other = by.is_some_and(|b| !b.is_empty() && b != owner);
    let now = now_ms();
    transact_as(path, Cause::LeaseLeft { ids, by_other }, |s| {
        let pieces: Vec<(bool, ResourceRequest)> = s
            .grants
            .iter()
            .map(|r| (true, r))
            .chain(s.queue.iter().map(|r| (false, r)))
            .filter(|(_, r)| r.is_lease() && r.session_id.as_deref() == Some(owner.as_str()))
            .map(|(g, r)| (g, r.clone()))
            .collect();
        let mut left = only.cloned();
        let mut released = Vec::new();
        for (was_held, piece) in pieces {
            let gone: BTreeMap<String, u32> = match left.as_mut() {
                None => piece.needs.clone(),
                Some(left) => piece
                    .needs
                    .iter()
                    .filter_map(|(name, &n)| {
                        let l = left.get_mut(name)?;
                        let k = (*l).min(n);
                        *l -= k;
                        (k > 0).then(|| (name.clone(), k))
                    })
                    .collect(),
            };
            if gone.is_empty() {
                continue;
            }
            let rest: BTreeMap<String, u32> = piece
                .needs
                .iter()
                .filter_map(|(name, &n)| {
                    let r = n - gone.get(name).copied().unwrap_or(0);
                    (r > 0).then(|| (name.clone(), r))
                })
                .collect();
            let whole = rest.is_empty();
            if whole {
                s.remove(&piece.id);
            } else if let Some(r) = s.grants.iter_mut().chain(s.queue.iter_mut()).find(|r| r.id == piece.id) {
                r.needs = rest;
                if !was_held {
                    r.skips = 0;
                }
            }
            released.push(ReleasedPart { id: piece.id, needs: gone, was_held, whole });
        }
        let pass = s.schedule(&super::holder_alive, now);
        let handed_to = s
            .grants
            .iter()
            .filter(|g| pass.granted.contains(&g.id))
            .cloned()
            .collect();
        LeaseRelease {
            released,
            kept: s.session_holds(&owner),
            still_queued: queued_piece(s, &owner).map(|q| q.needs.clone()).unwrap_or_default(),
            handed_to,
        }
    })
}

/// Leave the queue: drop the session's queued lease piece, keeping whatever
/// it holds. Refused when the session holds a lease and waits for nothing.
pub fn unqueue(session: &str) -> Result<LeaseRelease, String> {
    unqueue_at(&state_path()?, session)
}

pub(crate) fn unqueue_at(path: &Path, session: &str) -> Result<LeaseRelease, String> {
    require_session(session)?;
    let found = transact(path, |s| {
        let owner = resolve(s, session);
        let queued = queued_piece(s, &owner).map(|q| q.id.clone());
        let held = s.session_leases(&owner).first().map(|g| g.id.clone());
        (queued, held)
    })?;
    match found {
        (Some(id), _) => release_id_at(path, &id, false),
        (None, Some(held)) => Err(format!(
            "{held} is held, not queued, and nothing more is queued — use release"
        )),
        (None, None) => Ok(LeaseRelease::nothing()),
    }
}

/// Release lease piece `id` (held or queued) whole. `by_other`: on its
/// session's behalf.
pub fn release_id(id: &str, by_other: bool) -> Result<LeaseRelease, String> {
    release_id_at(&state_path()?, id, by_other)
}

pub(crate) fn release_id_at(path: &Path, id: &str, by_other: bool) -> Result<LeaseRelease, String> {
    let now = now_ms();
    let cause = Cause::LeaseLeft { ids: vec![id.to_string()], by_other };
    transact_as(path, cause, |s| {
        let found = s
            .grants
            .iter()
            .map(|r| (true, r))
            .chain(s.queue.iter().map(|r| (false, r)))
            .find(|(_, r)| r.id == id && r.is_lease())
            .map(|(g, r)| (g, r.clone()));
        let Some((was_held, lease)) = found else {
            return Err(format!("no lease {id}"));
        };
        s.remove(id);
        let pass = s.schedule(&super::holder_alive, now);
        let handed_to = s
            .grants
            .iter()
            .filter(|g| pass.granted.contains(&g.id))
            .cloned()
            .collect();
        let owner = lease.session_id.clone().unwrap_or_default();
        Ok(LeaseRelease {
            released: vec![ReleasedPart { id: lease.id, needs: lease.needs, was_held, whole: true }],
            kept: s.session_holds(&owner),
            still_queued: queued_piece(s, &owner).map(|q| q.needs.clone()).unwrap_or_default(),
            handed_to,
        })
    })?
}

/// Move every lease of `from` to `to` (a handoff's successor). Returns the
/// leases moved.
pub fn transfer_session(from: &str, to: &str) -> Result<Vec<ResourceRequest>, String> {
    transfer_session_at(&state_path()?, from, to)
}

pub(crate) fn transfer_session_at(path: &Path, from: &str, to: &str) -> Result<Vec<ResourceRequest>, String> {
    if from.is_empty() || to.is_empty() || from == to {
        return Ok(Vec::new());
    }
    transact(path, |s| {
        let mut moved = Vec::new();
        for r in s.grants.iter_mut().chain(s.queue.iter_mut()) {
            if r.is_lease() && r.session_id.as_deref() == Some(from) {
                r.session_id = Some(to.to_string());
                r.idle_since_ms = None;
                moved.push(r.clone());
            }
        }
        moved
    })
}

/// The leases `session` holds or waits for, without changing anything but
/// what a lookup would have anyway.
pub fn leases_of_at(path: &Path, session: &str) -> Vec<ResourceRequest> {
    let Ok(raw) = std::fs::read(path) else { return Vec::new() };
    let Ok(state) = serde_json::from_slice::<State>(&raw) else { return Vec::new() };
    state
        .grants
        .iter()
        .chain(&state.queue)
        .filter(|r| r.is_lease() && r.session_id.as_deref() == Some(session))
        .cloned()
        .collect()
}

/// Parse `--after`: a duration from now (`30m`, `8h`), a time of day
/// (`HH:MM`, today or tomorrow), `MM-DD HH:MM` (this year) or
/// `YYYY-MM-DD HH:MM`, all local time.
pub fn parse_after(spec: &str, now: u64) -> Result<u64, String> {
    let s = spec.trim();
    if s.is_empty() {
        return Err("--after is empty".into());
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !s.contains(':')
        && !s.contains('-')
    {
        return crate::watch::parse_duration_secs(s).map(|secs| now + secs * 1000);
    }
    // `MM-DD HH:MM` → prefix this year.
    if let Some((date, _)) = s.split_once(' ') {
        if date.len() <= 5 && date.matches('-').count() == 1 {
            use chrono::{Datelike, TimeZone};
            let year = chrono::Local
                .timestamp_millis_opt(now as i64)
                .single()
                .map(|t| t.year())
                .ok_or("clock out of range")?;
            return crate::schedule::parse_at(&format!("{year}-{s}"), now)
                .map_err(|_| after_help(s));
        }
    }
    crate::schedule::parse_at(s, now).map_err(|_| after_help(s))
}

fn after_help(s: &str) -> String {
    format!("cannot parse --after '{s}' (try 30m, 8h, \"21:00\", \"11-24 09:00\" or \"2026-11-24 09:00\")")
}

#[cfg(test)]
mod tests {
    use super::super::{check_nested_at, history, kick_at, Nesting, WaitReason, KICK_GRACE};
    use super::*;
    use crate::resource_queue::{parse_needs, HistoryQuery, ResourceEventKind};

    fn needs(spec: &[&str]) -> BTreeMap<String, u32> {
        parse_needs(spec).unwrap()
    }

    fn spec(session: &str, need: &[&str]) -> LeaseSpec {
        LeaseSpec {
            session_id: session.into(),
            needs: needs(need),
            priority: Priority::Normal,
            after_ms: None,
            note: Some(format!("work of {session}")),
            workspace: None,
        }
    }

    fn job(id: &str, need: &[&str]) -> ResourceRequest {
        let mut r = new_request(&spec("x", need), 0);
        r.id = id.into();
        r.kind = RequestKind::Job;
        r.session_id = None;
        r.holder = Holder { pid: 1, start_time: Some(1) };
        r
    }

    fn lease_req(id: &str, session: &str, need: &[&str]) -> ResourceRequest {
        let mut r = new_request(&spec(session, need), 0);
        r.id = id.into();
        r
    }

    fn alive(_: &Holder) -> bool {
        true
    }

    fn granted(s: &State) -> Vec<&str> {
        s.grants.iter().map(|r| r.id.as_str()).collect()
    }

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        (dir, path)
    }

    fn kinds(path: &Path) -> Vec<ResourceEventKind> {
        let mut h = history::read_history_at(path, &HistoryQuery::default()).events;
        h.reverse();
        h.into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn a_lease_not_yet_due_is_not_granted_and_holds_nobody_up() {
        let mut s = State::default();
        let mut l = lease_req("l1", "sa", &["board"]);
        l.after_ms = Some(1_000);
        s.enqueue(l).unwrap();
        s.enqueue(job("j", &["board"])).unwrap();
        s.schedule(&alive, 500);
        assert_eq!(granted(&s), vec!["j"], "the job behind the early lease goes first");
        let snap = s.snapshot(500);
        assert_eq!(snap.queue[0].wait_on[0].reason, WaitReason::After);
        // Once due it queues like anyone else, behind the job's grant.
        s.remove("j");
        s.schedule(&alive, 1_000);
        assert_eq!(granted(&s), vec!["l1"]);
    }

    #[test]
    fn a_waiting_lease_lets_others_fill_the_gap_twice_then_holds_its_place() {
        let mut s = State::default();
        s.enqueue(job("holder", &["board"])).unwrap();
        s.schedule(&alive, 1);
        s.enqueue(lease_req("l", "sa", &["board", "phone"])).unwrap();
        s.schedule(&alive, 1);
        for (n, id) in ["p1", "p2"].iter().enumerate() {
            s.enqueue(job(id, &["phone"])).unwrap();
            s.schedule(&alive, 1);
            assert!(granted(&s).contains(id), "{id} fills the gap");
            assert_eq!(s.queue[0].skips, n as u32 + 1);
            s.remove(id);
            s.schedule(&alive, 1);
        }
        s.enqueue(job("p3", &["phone"])).unwrap();
        s.schedule(&alive, 1);
        assert!(!granted(&s).contains(&"p3"), "the third one waits behind the lease");
        let snap = s.snapshot(1);
        let p3 = snap.queue.iter().find(|q| q.request.id == "p3").unwrap();
        assert_eq!(p3.wait_on[0].reason, WaitReason::Reserved);
        assert_eq!(p3.wait_on[0].by, vec!["l".to_string()]);
    }

    #[test]
    fn a_waiting_command_still_lets_nobody_past() {
        let mut s = State::default();
        s.enqueue(job("holder", &["board"])).unwrap();
        s.schedule(&alive, 1);
        s.enqueue(job("big", &["board", "phone"])).unwrap();
        s.enqueue(job("small", &["phone"])).unwrap();
        s.schedule(&alive, 1);
        assert_eq!(granted(&s), vec!["holder"]);
    }

    #[test]
    fn settle_moves_handed_on_leases_and_drops_idle_ones_after_grace() {
        let mut s = State::default();
        s.enqueue(lease_req("la", "a", &["board"])).unwrap();
        s.enqueue(lease_req("lb", "b", &["phone"])).unwrap();
        s.enqueue(lease_req("lc", "c", &["desk"])).unwrap();
        s.schedule(&alive, 1);
        let facts = Facts {
            sessions: ["a", "b", "c"].map(String::from).into(),
            successor: [("a".to_string(), "a2".to_string())].into(),
            engaged: ["a2", "c"].map(String::from).into(),
        };
        let lapsed = settle(&mut s, Some(&facts), 1_000, 5_000);
        assert!(lapsed.is_empty());
        assert_eq!(s.leases_checked_ms, 1_000);
        let la = s.grants.iter().find(|r| r.id == "la").unwrap();
        assert_eq!(la.session_id.as_deref(), Some("a2"));
        assert_eq!(la.idle_since_ms, None);
        assert_eq!(s.grants.iter().find(|r| r.id == "lb").unwrap().idle_since_ms, Some(1_000));
        // Within the grace b stays; past it it goes. c came back meanwhile.
        let lapsed = settle(&mut s, Some(&facts), 5_999, 5_000);
        assert!(lapsed.is_empty());
        let lapsed = settle(&mut s, Some(&facts), 6_000, 5_000);
        assert_eq!(lapsed, ["lb".to_string()].into());
        assert_eq!(granted(&s), vec!["la", "lc"]);
        // No lookup → nothing changes.
        let before = s.clone();
        assert!(settle(&mut s, None, 99_999, 0).is_empty());
        assert_eq!(s, before);
    }

    #[test]
    fn take_queue_mine_release_follow_board_lock() {
        let (_d, path) = scratch();
        // A takes; B cannot take and is not left queued.
        assert!(take_at(&path, &spec("session-a", &["board"])).unwrap().is_held());
        let busy = take_at(&path, &spec("session-b", &["board"])).unwrap();
        assert!(matches!(busy, LeaseStatus::Busy { .. }), "{busy:?}");
        assert_eq!(status_at(&path, "session-b").unwrap(), LeaseStatus::None);
        // B queues and waits.
        let q = queue_at(&path, &spec("session-b", &["board"])).unwrap();
        assert!(matches!(q, LeaseStatus::Queued { position: 1, .. }), "{q:?}");
        // Taking again while holding is a no-op.
        assert!(take_at(&path, &spec("session-a", &["board"])).unwrap().is_held());
        // A releases: B holds, by an 8-char prefix too.
        let rel = release_at(&path, "session-a", None, None).unwrap();
        assert!(rel.released[0].was_held && rel.released[0].whole);
        assert_eq!(rel.handed_to.len(), 1);
        assert_eq!(rel.handed_to[0].session_id.as_deref(), Some("session-b"));
        assert!(status_at(&path, "session-b").unwrap().is_held());
        assert!(status_at(&path, "session-").unwrap().is_held());
        // Releasing a session with nothing is quiet.
        assert!(release_at(&path, "session-a", None, None).unwrap().released.is_empty());
        assert_eq!(
            kinds(&path),
            vec![
                ResourceEventKind::Enqueued,
                ResourceEventKind::Granted,
                ResourceEventKind::Enqueued,
                ResourceEventKind::Released,
                ResourceEventKind::Granted,
            ]
        );
    }

    #[test]
    fn queue_with_after_waits_and_unqueue_leaves() {
        let (_d, path) = scratch();
        let mut sp = spec("session-a", &["board"]);
        sp.after_ms = Some(now_ms() + 3_600_000);
        let q = queue_at(&path, &sp).unwrap();
        let LeaseStatus::Queued { wait_on, .. } = &q else { panic!("{q:?}") };
        assert_eq!(wait_on[0].reason, WaitReason::After);
        // Someone else takes the free board meanwhile.
        assert!(take_at(&path, &spec("session-b", &["board"])).unwrap().is_held());
        // take ignores one's own --after, but the board is busy now.
        let t = take_at(&path, &spec("session-a", &["board"])).unwrap();
        assert!(matches!(t, LeaseStatus::Queued { .. }), "still queued as before: {t:?}");
        let rel = unqueue_at(&path, "session-a").unwrap();
        assert!(!rel.released[0].was_held);
        assert_eq!(status_at(&path, "session-a").unwrap(), LeaseStatus::None);
        assert_eq!(*kinds(&path).last().unwrap(), ResourceEventKind::Cancelled);
    }

    #[test]
    fn a_lease_of_an_ended_session_is_reaped_and_handed_on() {
        let (_d, path) = scratch();
        assert!(take_at(&path, &spec("gone-session", &["board"])).unwrap().is_held());
        queue_at(&path, &spec("waiting-session", &["board"])).unwrap();
        // The owner was found idle long ago; the next lookup finds it still idle.
        transact(&path, |s| {
            s.grants[0].idle_since_ms = Some(1);
            s.leases_checked_ms = 0;
        })
        .unwrap();
        FAKE_FACTS.with(|f| {
            *f.borrow_mut() = Some(Box::new(|sessions: &BTreeSet<String>| Facts {
                sessions: sessions.clone(),
                successor: BTreeMap::new(),
                engaged: ["waiting-session".to_string()].into(),
            }))
        });
        let st = status_at(&path, "waiting-session").unwrap();
        FAKE_FACTS.with(|f| *f.borrow_mut() = None);
        assert!(st.is_held(), "{st:?}");
        let k = kinds(&path);
        assert_eq!(k[k.len() - 2..], [ResourceEventKind::Reaped, ResourceEventKind::Granted]);
    }

    #[test]
    fn handoff_transfer_moves_the_lease_and_logs_it() {
        let (_d, path) = scratch();
        take_at(&path, &spec("pred-session", &["board"])).unwrap();
        let moved = transfer_session_at(&path, "pred-session", "succ-session").unwrap();
        assert_eq!(moved.len(), 1);
        assert!(status_at(&path, "succ-session").unwrap().is_held());
        assert_eq!(status_at(&path, "pred-session").unwrap(), LeaseStatus::None);
        let h = history::read_history_at(&path, &HistoryQuery::default());
        let t = &h.events[0];
        assert_eq!(t.kind, ResourceEventKind::Transferred);
        assert_eq!(t.from_session_id.as_deref(), Some("pred-session"));
        assert_eq!(t.session_id.as_deref(), Some("succ-session"));
        assert_eq!(t.pid, None);
    }

    #[test]
    fn kicking_a_lease_releases_it_on_its_behalf() {
        let (_d, path) = scratch();
        let LeaseStatus::Held { lease, .. } = take_at(&path, &spec("owner-session", &["board"])).unwrap()
        else {
            panic!()
        };
        queue_at(&path, &spec("next-session", &["board"])).unwrap();
        let snap = kick_at(&path, &lease.id, KICK_GRACE).unwrap();
        assert_eq!(snap.holders[0].session_id.as_deref(), Some("next-session"));
        assert!(kinds(&path).contains(&ResourceEventKind::Kicked));
        // Releasing by another session is a kick too.
        release_at(&path, "next-session", Some("someone-else"), None).unwrap();
        assert_eq!(*kinds(&path).last().unwrap(), ResourceEventKind::Kicked);
    }

    #[test]
    fn commands_of_a_leasing_session_run_under_the_lease() {
        let (_d, path) = scratch();
        let LeaseStatus::Held { lease, .. } =
            take_at(&path, &spec("lease-session", &["board", "phone"])).unwrap()
        else {
            panic!()
        };
        let n = |need: &[&str], sid: Option<&str>| check_nested_at(&path, None, sid, &needs(need));
        assert_eq!(n(&["board"], Some("lease-session")).unwrap(), Nesting::Inherited(lease.id));
        assert_eq!(n(&["cargo"], Some("lease-session")).unwrap(), Nesting::TopLevel);
        assert!(n(&["board", "cargo"], Some("lease-session")).is_err());
        assert_eq!(n(&["board"], Some("other-session")).unwrap(), Nesting::TopLevel);
        assert_eq!(n(&["board"], None).unwrap(), Nesting::TopLevel);
    }

    fn held_status(st: &LeaseStatus) -> (Vec<String>, BTreeMap<String, u32>, Option<PendingLease>) {
        let LeaseStatus::Held { leases, needs, pending, .. } = st else { panic!("not held: {st:?}") };
        (leases.iter().map(|l| l.id.clone()).collect(), needs.clone(), pending.clone())
    }

    #[test]
    fn take_while_holding_appends_a_piece() {
        let (_d, path) = scratch();
        assert!(take_at(&path, &spec("session-a", &["round"])).unwrap().is_held());
        let st = take_at(&path, &spec("session-a", &["round", "wave"])).unwrap();
        let (pieces, held, pending) = held_status(&st);
        assert_eq!(pieces.len(), 2, "{st:?}");
        assert_eq!(held, needs(&["round", "wave"]));
        assert!(pending.is_none());
        assert!(st.holds(&needs(&["wave"])));
        assert!(st.describe().starts_with("holding [round, wave] as lease"), "{}", st.describe());
        // A command of the session may use both pieces at once.
        let nested = check_nested_at(&path, None, Some("session-a"), &needs(&["round", "wave"])).unwrap();
        assert_eq!(nested, Nesting::Inherited(pieces[0].clone()));
        // Asking for more units of a held resource is refused.
        transact(&path, |s| {
            s.resources.insert("gpu".into(), super::super::ResourceDef { capacity: 2 });
        })
        .unwrap();
        assert!(take_at(&path, &spec("session-a", &["gpu"])).unwrap().is_held());
        let err = take_at(&path, &spec("session-a", &["gpu:2"])).unwrap_err();
        assert!(err.contains("growing a held resource"), "{err}");
    }

    #[test]
    fn queue_while_holding_waits_on_top_and_is_granted_later() {
        let (_d, path) = scratch();
        take_at(&path, &spec("session-a", &["round"])).unwrap();
        take_at(&path, &spec("session-c", &["wave"])).unwrap();
        let st = queue_at(&path, &spec("session-a", &["wave"])).unwrap();
        let (_, held, pending) = held_status(&st);
        assert_eq!(held, needs(&["round"]));
        assert_eq!(pending.unwrap().lease.needs, needs(&["wave"]));
        assert!(st.waiting() && !st.is_held() && !st.holds(&needs(&["wave"])));
        assert!(st.describe().contains("on top — waiting on"), "{}", st.describe());
        release_at(&path, "session-c", None, None).unwrap();
        let st = status_at(&path, "session-a").unwrap();
        assert!(st.is_held() && st.holds(&needs(&["round", "wave"])), "{st:?}");
    }

    #[test]
    fn releasing_one_resource_hands_it_on_and_keeps_the_rest() {
        let (_d, path) = scratch();
        take_at(&path, &spec("session-a", &["round", "wave"])).unwrap();
        queue_at(&path, &spec("session-b", &["round"])).unwrap();
        let rel = release_at(&path, "session-a", None, Some(&needs(&["round"]))).unwrap();
        assert_eq!(rel.released.len(), 1);
        assert_eq!(rel.released[0].needs, needs(&["round"]));
        assert!(rel.released[0].was_held && !rel.released[0].whole);
        assert_eq!(rel.kept, needs(&["wave"]));
        assert_eq!(rel.handed_to[0].session_id.as_deref(), Some("session-b"));
        assert!(rel.describe().contains("still holding [wave]"), "{}", rel.describe());
        let a = status_at(&path, "session-a").unwrap();
        assert!(a.holds(&needs(&["wave"])) && !a.holds(&needs(&["round"])), "{a:?}");
        assert!(status_at(&path, "session-b").unwrap().is_held());
        // History: the piece shrank (released round, kept wave), then B got it.
        let h = history::read_history_at(&path, &HistoryQuery::default()).events;
        assert_eq!(h[0].kind, ResourceEventKind::Granted);
        assert_eq!(h[1].kind, ResourceEventKind::Released);
        assert_eq!(h[1].needs, needs(&["round"]));
        assert_eq!(h[1].kept, needs(&["wave"]));
        assert!(history::render_history(&history::ResourceHistory { events: vec![h[1].clone()], has_more: false })
            .contains("kept [wave]"));
        // Releasing what the session does not hold is quiet.
        assert!(release_at(&path, "session-a", None, Some(&needs(&["round"]))).unwrap().released.is_empty());
        // Releasing the rest drops the piece.
        let rel = release_at(&path, "session-a", None, Some(&needs(&["wave"]))).unwrap();
        assert!(rel.released[0].whole && rel.kept.is_empty());
        assert_eq!(status_at(&path, "session-a").unwrap(), LeaseStatus::None);
    }

    #[test]
    fn partial_release_spans_pieces() {
        let (_d, path) = scratch();
        take_at(&path, &spec("session-a", &["round"])).unwrap();
        take_at(&path, &spec("session-a", &["wave", "phone"])).unwrap();
        let rel = release_at(&path, "session-a", None, Some(&needs(&["round", "phone"]))).unwrap();
        assert_eq!(rel.released.len(), 2);
        assert!(rel.released[0].whole, "the round piece goes whole");
        assert!(!rel.released[1].whole, "the other keeps wave");
        assert_eq!(rel.kept, needs(&["wave"]));
    }

    #[test]
    fn crossed_waits_while_holding_are_refused() {
        let (_d, path) = scratch();
        take_at(&path, &spec("session-a", &["round"])).unwrap();
        take_at(&path, &spec("session-b", &["wave"])).unwrap();
        // A waits for wave while holding round: fine on its own.
        assert!(queue_at(&path, &spec("session-a", &["wave"])).unwrap().waiting());
        let before = std::fs::read(&path).unwrap();
        // B waiting for round while holding wave closes the cycle.
        let err = queue_at(&path, &spec("session-b", &["round"])).unwrap_err();
        assert!(err.contains("would deadlock"), "{err}");
        assert!(err.contains("waits for round held by lease"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing changed");
        // take never waits: round is held, so it is only busy.
        assert!(matches!(take_at(&path, &spec("session-b", &["round"])).unwrap(), LeaseStatus::Busy { .. }));
        // Nor may B's commands wait for round.
        let job = super::super::AcquireSpec {
            id: "jb".into(),
            kind: RequestKind::Job,
            needs: needs(&["round"]),
            priority: Priority::Normal,
            max_wait: Some(Duration::from_millis(1)),
            command: "flash".into(),
            workspace: None,
            session_id: Some("session-b".into()),
        };
        let err = super::super::acquire_at(&path, job, Holder::current(), Duration::from_millis(1), |_| true)
            .unwrap_err();
        let super::super::AcquireError::Rejected(msg) = err else { panic!("{err:?}") };
        assert!(msg.contains("would deadlock"), "{msg}");
        assert!(!kinds(&path).contains(&ResourceEventKind::Aborted), "refused, never queued");
        // Once A lets go of round, B may queue for it.
        unqueue_at(&path, "session-a").unwrap();
        assert!(queue_at(&path, &spec("session-b", &["round"])).unwrap().waiting());
    }

    #[test]
    fn a_lease_whose_session_waits_never_fills_a_gap() {
        // `phone` is reserved by c, waiting for the busy board with skips to
        // spare. A lease of a session that waits for nothing else may pass c;
        // one whose session also waits (a's job for desk) may not.
        let build = |also_waits: bool| {
            let mut s = State::default();
            s.enqueue(job("h", &["board"])).unwrap();
            s.enqueue(job("d", &["desk"])).unwrap();
            s.schedule(&alive, 1);
            s.enqueue(lease_req("lc", "c", &["board", "phone"])).unwrap();
            if also_waits {
                let mut ja = job("ja", &["desk"]);
                ja.session_id = Some("a".into());
                s.enqueue(ja).unwrap();
            }
            s.enqueue(lease_req("la", "a", &["phone"])).unwrap();
            s.schedule(&alive, 1);
            s
        };
        assert!(granted(&build(false)).contains(&"la"));
        let s = build(true);
        assert!(!granted(&s).contains(&"la"));
        let snap = s.snapshot(1);
        let la = snap.queue.iter().find(|q| q.request.id == "la").unwrap();
        assert_eq!(la.wait_on[0].reason, WaitReason::Reserved);
        assert_eq!(la.wait_on[0].by, vec!["lc".to_string()]);
    }

    #[test]
    fn ls_marks_a_queued_piece_of_a_holding_session() {
        let (_d, path) = scratch();
        take_at(&path, &spec("session-a", &["round"])).unwrap();
        take_at(&path, &spec("session-c", &["wave"])).unwrap();
        queue_at(&path, &spec("session-a", &["wave"])).unwrap();
        let text = super::super::render_snapshot(&super::super::snapshot_at(&path).unwrap());
        assert!(text.contains("· holds [round]"), "{text}");
    }

    #[test]
    fn after_accepts_durations_and_local_times() {
        let now = 1_700_000_000_000;
        assert_eq!(parse_after("30m", now).unwrap(), now + 1_800_000);
        assert_eq!(parse_after("8h", now).unwrap(), now + 8 * 3_600_000);
        let t = parse_after("21:00", now).unwrap();
        assert!(t > now && t <= now + 86_400_000);
        let md = parse_after("12-24 09:00", now).unwrap();
        assert_eq!(md, crate::schedule::parse_at("2023-12-24 09:00", now).unwrap());
        assert!(parse_after("2026-11-24 09:00", now).is_ok());
        assert!(parse_after("soon", now).is_err());
    }
}
