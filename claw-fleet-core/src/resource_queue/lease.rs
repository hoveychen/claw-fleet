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
//! * `release` / `unqueue` — let go / leave the line;
//! * a session holds at most one lease: asking for more while holding some is
//!   refused (release first, then take everything at once), as for commands.
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

/// A session's lease, as `mine` / `take` / `queue` report it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LeaseStatus {
    /// The session holds it.
    Held { lease: ResourceRequest },
    /// The session waits in the queue.
    #[serde(rename_all = "camelCase")]
    Queued { lease: ResourceRequest, position: usize, wait_on: Vec<WaitOn> },
    /// `take` only: not available now, and nothing was queued.
    #[serde(rename_all = "camelCase")]
    Busy { wait_on: Vec<WaitOn> },
    /// The session neither holds nor waits for anything.
    None,
}

impl LeaseStatus {
    pub fn is_held(&self) -> bool {
        matches!(self, Self::Held { .. })
    }

    /// One line for the CLI and the MCP tool.
    pub fn describe(&self) -> String {
        match self {
            Self::Held { lease } => format!(
                "holding lease {} for [{}]{}",
                lease.id,
                format_needs(&lease.needs),
                lease.note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default()
            ),
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

/// What [`release`] let go of.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LeaseRelease {
    /// The lease removed — held, or only queued (`was_held` false).
    pub lease: Option<ResourceRequest>,
    pub was_held: bool,
    /// Requests granted by the same pass, i.e. who got the resources next.
    pub handed_to: Vec<ResourceRequest>,
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

fn find_lease<'a>(state: &'a State, session: &str) -> Option<(bool, &'a ResourceRequest)> {
    state
        .grants
        .iter()
        .find(|r| r.is_lease() && r.session_id.as_deref() == Some(session))
        .map(|r| (true, r))
        .or_else(|| {
            state
                .queue
                .iter()
                .find(|r| r.is_lease() && r.session_id.as_deref() == Some(session))
                .map(|r| (false, r))
        })
}

/// The session's lease as it stands. Call after a scheduling pass.
fn status_in(state: &State, session: &str, now: u64) -> LeaseStatus {
    match find_lease(state, session) {
        Some((true, r)) => LeaseStatus::Held { lease: r.clone() },
        Some((false, r)) => {
            let snap = state.snapshot(now);
            let q = snap.queue.into_iter().find(|q| q.request.id == r.id);
            match q {
                Some(q) => LeaseStatus::Queued {
                    lease: q.request,
                    position: q.position,
                    wait_on: q.wait_on,
                },
                None => LeaseStatus::None,
            }
        }
        None => LeaseStatus::None,
    }
}

fn holds_all(held: &BTreeMap<String, u32>, needs: &BTreeMap<String, u32>) -> bool {
    needs.iter().all(|(n, &u)| held.get(n).copied().unwrap_or(0) >= u)
}

fn refuse_more(lease: &ResourceRequest, needs: &BTreeMap<String, u32>) -> String {
    format!(
        "this session already holds lease {} for [{}] and asked for [{}]. Waiting for more \
         while holding some is how deadlocks start — release it first, then take everything \
         at once.",
        lease.id,
        format_needs(&lease.needs),
        format_needs(needs)
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
/// Already holding all of it is a no-op; a session already queued is granted
/// out of turn if the queue allows it (its `--after` is ignored), else stays
/// queued as it was.
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
        match find_lease(s, session).map(|(g, r)| (g, r.clone())) {
            Some((true, held)) => {
                if holds_all(&held.needs, &spec.needs) {
                    return Ok(LeaseStatus::Held { lease: held });
                }
                return Err(refuse_more(&held, &spec.needs));
            }
            Some((false, queued)) => {
                let backup = s.clone();
                if let Some(r) = s.queue.iter_mut().find(|r| r.id == queued.id) {
                    r.after_ms = None;
                    if r.needs != spec.needs {
                        r.needs = spec.needs.clone();
                        r.skips = 0;
                    }
                    if spec.note.is_some() {
                        r.note = spec.note.clone();
                    }
                }
                s.schedule(&super::holder_alive, now);
                if s.grants.iter().any(|g| g.id == queued.id) {
                    return Ok(status_in(s, session, now));
                }
                *s = backup;
                return Ok(status_in(s, session, now));
            }
            None => {}
        }
        let backup = s.clone();
        let req = new_request(&LeaseSpec { after_ms: None, ..spec.clone() }, now);
        let id = req.id.clone();
        s.enqueue(req)?;
        s.schedule(&super::holder_alive, now);
        if s.grants.iter().any(|g| g.id == id) {
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
/// Queueing again updates the request in place, keeping its place in line.
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
        match find_lease(s, session).map(|(g, r)| (g, r.clone())) {
            Some((true, held)) => {
                if holds_all(&held.needs, &spec.needs) {
                    return Ok(LeaseStatus::Held { lease: held });
                }
                return Err(refuse_more(&held, &spec.needs));
            }
            Some((false, queued)) => {
                for name in spec.needs.keys() {
                    s.resources
                        .entry(name.clone())
                        .or_insert(super::ResourceDef { capacity: super::DEFAULT_CAPACITY });
                }
                if let Some(r) = s.queue.iter_mut().find(|r| r.id == queued.id) {
                    if r.needs != spec.needs {
                        r.needs = spec.needs.clone();
                        r.skips = 0;
                    }
                    r.after_ms = spec.after_ms;
                    if spec.note.is_some() {
                        r.note = spec.note.clone();
                    }
                }
            }
            None => s.enqueue(new_request(spec, now))?,
        }
        s.schedule(&super::holder_alive, now);
        Ok(status_in(s, session, now))
    })?
}

/// The session's lease: held, queued (with its place and what it waits on),
/// or none.
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

/// Let go of the session's lease — held or queued. `by` is who asks; anyone
/// but the owner releases on its behalf (logged as kicked).
pub fn release(session: &str, by: Option<&str>) -> Result<LeaseRelease, String> {
    release_at(&state_path()?, session, by)
}

pub(crate) fn release_at(path: &Path, session: &str, by: Option<&str>) -> Result<LeaseRelease, String> {
    require_session(session)?;
    let found = transact(path, |s| {
        let owner = resolve(s, session);
        find_lease(s, &owner).map(|(_, r)| (r.id.clone(), owner))
    })?;
    let Some((id, owner)) = found else {
        return Ok(LeaseRelease { lease: None, was_held: false, handed_to: Vec::new() });
    };
    let by_other = by.is_some_and(|b| !b.is_empty() && b != owner);
    release_id_at(path, &id, by_other)
}

/// Release lease `id` (held or queued). `by_other`: on its session's behalf.
pub fn release_id(id: &str, by_other: bool) -> Result<LeaseRelease, String> {
    release_id_at(&state_path()?, id, by_other)
}

pub(crate) fn release_id_at(path: &Path, id: &str, by_other: bool) -> Result<LeaseRelease, String> {
    let now = now_ms();
    let cause = Cause::LeaseLeft { id: id.to_string(), by_other };
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
        Ok(LeaseRelease { lease: Some(lease), was_held, handed_to })
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
        // Taking again while holding is a no-op; asking for more is refused.
        assert!(take_at(&path, &spec("session-a", &["board"])).unwrap().is_held());
        assert!(take_at(&path, &spec("session-a", &["board", "phone"])).is_err());
        // A releases: B holds, by an 8-char prefix too.
        let rel = release_at(&path, "session-a", None).unwrap();
        assert!(rel.was_held);
        assert_eq!(rel.handed_to.len(), 1);
        assert_eq!(rel.handed_to[0].session_id.as_deref(), Some("session-b"));
        assert!(status_at(&path, "session-b").unwrap().is_held());
        assert!(status_at(&path, "session-").unwrap().is_held());
        // Releasing a session with nothing is quiet.
        assert_eq!(release_at(&path, "session-a", None).unwrap().lease, None);
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
        let rel = release_at(&path, "session-a", None).unwrap();
        assert!(!rel.was_held);
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
        let LeaseStatus::Held { lease } = take_at(&path, &spec("owner-session", &["board"])).unwrap()
        else {
            panic!()
        };
        queue_at(&path, &spec("next-session", &["board"])).unwrap();
        let snap = kick_at(&path, &lease.id, KICK_GRACE).unwrap();
        assert_eq!(snap.holders[0].session_id.as_deref(), Some("next-session"));
        assert!(kinds(&path).contains(&ResourceEventKind::Kicked));
        // Releasing by another session is a kick too.
        release_at(&path, "next-session", Some("someone-else")).unwrap();
        assert_eq!(*kinds(&path).last().unwrap(), ResourceEventKind::Kicked);
    }

    #[test]
    fn commands_of_a_leasing_session_run_under_the_lease() {
        let (_d, path) = scratch();
        let LeaseStatus::Held { lease } =
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
