//! Resource queue — machine-global named resources that commands hold while
//! they run, with one shared queue instead of per-project lock files and
//! "I'm using it" messages between agents.
//!
//! A command declares everything it needs up front (`{test-db: 1, gpu: 1}`).
//! It is granted **all of it or none of it**, holds it for exactly as long as
//! its process lives, and is released by that process exiting — cleanly or not.
//! That removes the three ways the ad-hoc locks deadlocked:
//!
//! * **a dead holder** — a holder is a `(pid, process start time)` pair, and
//!   every scheduling pass first drops holders and waiters whose process is
//!   gone;
//! * **hold-and-wait** — nothing ever waits while holding a subset, and a
//!   command running under a grant may not ask for more (see [`check_nested`]);
//! * **stale word of mouth** — the table in `~/.fleet/resources/state.json` is
//!   the only source of truth, and anyone can read who holds and who waits.
//!
//! # Scheduling
//!
//! One queue for the whole machine. A request is inserted by
//! `(priority, enqueue time)`; a manual [`move_request`] rewrites its place and
//! pins it. Each pass walks the queue from the head with a *reserved set* —
//! the resources wanted by requests further ahead that could not be granted:
//!
//! * every unit it needs is free **and** none of its resources is reserved →
//!   grant the whole request;
//! * otherwise → it stays queued and its resources join the reserved set.
//!
//! So a request at the head that needs several resources cannot be starved by
//! a stream of smaller ones behind it (they may not take what it is waiting
//! for), while requests for unrelated resources still run ahead of it
//! (conservative backfill).
//!
//! # No scheduler process
//!
//! There is no daemon. Every waiter runs the same pass under an exclusive file
//! lock when it enqueues, every [`POLL`] while it waits, and when it releases;
//! so do the read/admin calls. The pass is a pure function of the state
//! ([`State::schedule`]), so whoever runs it reaches the same answer, and the
//! queue keeps moving with the desktop app and `fleet serve` both closed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub mod history;
use history::Cause;
pub use history::{
    read_history, render_history, HistoryQuery, ResourceEvent, ResourceEventKind, ResourceHistory,
};

/// Set in the environment of a command started under a grant; its value is the
/// grant's request id. [`check_nested`] reads it.
pub const GRANT_ENV: &str = "FLEET_RESOURCE_GRANT";

/// Exit code of a command that gave up waiting (`--max-wait`) without ever
/// starting — `EX_TEMPFAIL`. Distinct from anything the command itself returns
/// in practice, so a wrapper can tell "never ran" from "ran and failed".
pub const EXIT_QUEUE_TIMEOUT: i32 = 75;
/// Exit code of a command whose queued request was cancelled before it ran.
pub const EXIT_QUEUE_CANCELLED: i32 = 76;
/// Exit code of a command whose request was refused outright (bad `--need`,
/// more units than the resource has, a nested request for something new).
pub const EXIT_QUEUE_REJECTED: i32 = 64;

/// How often a waiter re-runs the scheduling pass.
pub const POLL: Duration = Duration::from_millis(500);

/// Capacity a resource gets when it is first named.
pub const DEFAULT_CAPACITY: u32 = 1;

const MAX_NAME_LEN: usize = 64;
const STATE_VERSION: u32 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Wire types
// ─────────────────────────────────────────────────────────────────────────────

/// Queue priority. Declaration order is queue order: `High` sorts first.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    #[default]
    Normal,
    Low,
}

impl std::str::FromStr for Priority {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" => Ok(Self::High),
            "normal" | "" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            other => Err(format!("unknown priority '{other}' (use high, normal or low)")),
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        })
    }
}

/// The process a request belongs to. It holds (or waits) for as long as that
/// process lives; the start time defends against the pid being reused.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub start_time: Option<u64>,
}

impl Holder {
    /// The calling process.
    pub fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            start_time: crate::session::process_start_time(pid),
        }
    }

    pub fn is_alive(&self) -> bool {
        crate::proc_runner::host_alive(self.pid, self.start_time)
    }
}

/// The command a granted request started: what has to die with its holder.
///
/// A holder killed outright (`kill -9`) cannot stop its command, and once the
/// next pass reaps the holder its resources go to the next request — so the
/// orphaned command is killed first (see `transact`), or two commands would
/// use one resource at once.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ChildProc {
    pid: u32,
    /// Pid-reuse guard, as for [`Holder`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    start_time: Option<u64>,
    /// The command leads its own process group (`pgid == pid`), so the whole
    /// group is killed. Otherwise it shares its holder's group (a foreground
    /// run in an interactive terminal) and only the command itself is.
    #[serde(default)]
    own_group: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    /// A `fleet job run --need …` job; the request id is the job id and the
    /// holder is the job's host process.
    Job,
    /// A foreground `fleet resource run`; the holder is that CLI process.
    Foreground,
}

/// One request for a set of resources — queued, or granted and holding.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResourceRequest {
    pub id: String,
    pub kind: RequestKind,
    /// Resource name → units.
    pub needs: BTreeMap<String, u32>,
    #[serde(default)]
    pub priority: Priority,
    pub enqueued_ms: u64,
    /// Set once granted; a granted request holds every unit in `needs`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub granted_ms: Option<u64>,
    /// Placed by hand ([`move_request`]): later arrivals do not jump it on
    /// priority alone.
    #[serde(default)]
    pub manual: bool,
    pub holder: Holder,
    /// What will run. Empty for an incognito session's job.
    #[serde(default)]
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum WaitReason {
    /// Not enough free units: `by` lists the requests holding the resource.
    Busy,
    /// Free, but promised to requests further ahead in the queue: `by` lists
    /// them.
    Reserved,
}

/// Why a queued request is not running yet, for one of its resources.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct WaitOn {
    pub resource: String,
    pub reason: WaitReason,
    /// Request ids — holders for `busy`, queued requests ahead for `reserved`.
    pub by: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct QueuedRequest {
    /// 1-based place in the queue.
    pub position: usize,
    pub request: ResourceRequest,
    pub wait_on: Vec<WaitOn>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResourceView {
    pub name: String,
    pub capacity: u32,
    pub in_use: u32,
    /// Ids of the requests holding it (see [`ResourceSnapshot::holders`]).
    pub holders: Vec<String>,
    /// Ids of the queued requests that need it, in queue order.
    pub waiters: Vec<String>,
}

/// The whole table: every resource, every holder, the queue in order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub now_ms: u64,
    pub resources: Vec<ResourceView>,
    /// Granted requests, oldest grant first.
    pub holders: Vec<ResourceRequest>,
    pub queue: Vec<QueuedRequest>,
}

/// Where [`move_request`] puts a queued request.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum MoveTarget {
    /// 1-based queue position (1 = head; past the end = last).
    Index(usize),
    /// Directly ahead of this queued request.
    Before(String),
}

/// How a job that declared `needs` ended without ever running its command.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum QueueEnd {
    /// `--max-wait` passed. Exit code [`EXIT_QUEUE_TIMEOUT`].
    Timeout,
    /// The queued request was cancelled. Exit code [`EXIT_QUEUE_CANCELLED`].
    Cancelled,
    /// The request was refused. Exit code [`EXIT_QUEUE_REJECTED`].
    Rejected,
}

/// The resource side of a job's [`crate::proc_runner::ProcRecord`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct JobQueue {
    pub needs: BTreeMap<String, u32>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_wait_ms: Option<u64>,
    /// The session that started the job (shown in the queue).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
    /// The job was started under this grant, which already holds everything it
    /// needs, so it never queued on its own.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inherited_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub granted_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub end: Option<QueueEnd>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Parsing
// ─────────────────────────────────────────────────────────────────────────────

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("resource name is empty".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!("resource name '{name}' is longer than {MAX_NAME_LEN} bytes"));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')))
    {
        return Err(format!(
            "resource name '{name}' contains '{bad}' (use letters, digits and - _ . /)"
        ));
    }
    Ok(())
}

/// `test-db` → `("test-db", 1)`, `gpu:2` → `("gpu", 2)`.
pub fn parse_need(spec: &str) -> Result<(String, u32), String> {
    let spec = spec.trim();
    let (name, units) = match spec.rsplit_once(':') {
        Some((name, n)) => {
            let units: u32 = n
                .parse()
                .map_err(|_| format!("cannot parse '{spec}': '{n}' is not a unit count (e.g. gpu:2)"))?;
            (name, units)
        }
        None => (spec, 1),
    };
    validate_name(name)?;
    if units == 0 {
        return Err(format!("'{spec}' asks for zero units of {name}"));
    }
    Ok((name.to_string(), units))
}

/// Parse repeated `--need` values. Naming a resource twice adds the units up.
pub fn parse_needs<S: AsRef<str>>(specs: &[S]) -> Result<BTreeMap<String, u32>, String> {
    let mut needs = BTreeMap::new();
    for spec in specs {
        let (name, units) = parse_need(spec.as_ref())?;
        *needs.entry(name).or_insert(0) += units;
    }
    Ok(needs)
}

/// `{gpu: 2, test-db: 1}` → `gpu:2, test-db`.
pub fn format_needs(needs: &BTreeMap<String, u32>) -> String {
    needs
        .iter()
        .map(|(name, &n)| if n == 1 { name.clone() } else { format!("{name}:{n}") })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `test-db (held by p1), gpu (reserved for p2)`.
pub fn describe_wait(wait_on: &[WaitOn]) -> String {
    wait_on
        .iter()
        .map(|w| {
            let who = if w.by.is_empty() { "nobody".to_string() } else { w.by.join(", ") };
            match w.reason {
                WaitReason::Busy => format!("{} (held by {who})", w.resource),
                WaitReason::Reserved => format!("{} (reserved for {who}, ahead in the queue)", w.resource),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ─────────────────────────────────────────────────────────────────────────────
// State + the scheduling pass (pure)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct ResourceDef {
    capacity: u32,
}

/// On-disk state. Every function on it is pure: the caller supplies the clock
/// and the liveness probe.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
struct State {
    version: u32,
    resources: BTreeMap<String, ResourceDef>,
    /// Granted requests, oldest grant first.
    grants: Vec<ResourceRequest>,
    /// Waiting requests, in queue order.
    queue: Vec<ResourceRequest>,
    /// Request id → the command its grant started ([`record_child`]). An
    /// entry lives exactly as long as its grant (`transact` prunes the rest).
    children: BTreeMap<String, ChildProc>,
    /// Grants being evicted by [`kick`]: when one of them leaves, the history
    /// records it as kicked rather than released. Pruned with the grant.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    kicking: BTreeSet<String>,
}

/// What one scheduling pass did.
#[derive(Debug, Default, PartialEq, Eq)]
struct Pass {
    /// Requests dropped because their process is gone.
    reaped: Vec<String>,
    granted: Vec<String>,
}

impl State {
    fn capacity(&self, name: &str) -> u32 {
        self.resources.get(name).map_or(DEFAULT_CAPACITY, |r| r.capacity)
    }

    fn in_use(&self) -> BTreeMap<String, u32> {
        let mut used = BTreeMap::new();
        for g in &self.grants {
            for (name, &n) in &g.needs {
                *used.entry(name.clone()).or_insert(0) += n;
            }
        }
        used
    }

    /// The scheduling rule: which queued requests (by index) are granted now,
    /// and what each of the others is waiting on. `by` of a `Busy` entry is
    /// left empty here — [`State::snapshot`] fills in the holders.
    fn plan(&self) -> (Vec<usize>, Vec<(usize, Vec<WaitOn>)>) {
        let mut used = self.in_use();
        let mut reserved: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut grant = Vec::new();
        let mut waits = Vec::new();
        for (i, req) in self.queue.iter().enumerate() {
            let mut blockers = Vec::new();
            for (name, &n) in &req.needs {
                let free = self
                    .capacity(name)
                    .saturating_sub(used.get(name).copied().unwrap_or(0));
                if free < n {
                    blockers.push(WaitOn {
                        resource: name.clone(),
                        reason: WaitReason::Busy,
                        by: Vec::new(),
                    });
                } else if let Some(ahead) = reserved.get(name) {
                    blockers.push(WaitOn {
                        resource: name.clone(),
                        reason: WaitReason::Reserved,
                        by: ahead.clone(),
                    });
                }
            }
            if blockers.is_empty() {
                for (name, &n) in &req.needs {
                    *used.entry(name.clone()).or_insert(0) += n;
                }
                grant.push(i);
            } else {
                for name in req.needs.keys() {
                    reserved.entry(name.clone()).or_default().push(req.id.clone());
                }
                waits.push((i, blockers));
            }
        }
        (grant, waits)
    }

    /// One pass: drop requests whose process is gone, then grant whatever the
    /// rule allows. Idempotent — a second pass over the result changes nothing.
    fn schedule(&mut self, alive: &dyn Fn(&Holder) -> bool, now_ms: u64) -> Pass {
        let mut pass = Pass::default();
        for list in [&mut self.grants, &mut self.queue] {
            list.retain(|r| {
                let keep = alive(&r.holder);
                if !keep {
                    pass.reaped.push(r.id.clone());
                }
                keep
            });
        }
        let (grant, _) = self.plan();
        // Highest index first, so earlier indexes stay valid while removing.
        let mut taken: Vec<ResourceRequest> =
            grant.iter().rev().map(|&i| self.queue.remove(i)).collect();
        taken.reverse();
        for mut req in taken {
            req.granted_ms = Some(now_ms);
            pass.granted.push(req.id.clone());
            self.grants.push(req);
        }
        pass
    }

    /// Check `needs` against the table without changing it: a request for more
    /// units than a resource has could never be granted and would hold up
    /// everything queued behind it.
    fn check_fits(&self, needs: &BTreeMap<String, u32>) -> Result<(), String> {
        if needs.is_empty() {
            return Err("no resources requested".into());
        }
        for (name, &n) in needs {
            validate_name(name)?;
            if n == 0 {
                return Err(format!("zero units of {name} requested"));
            }
            let cap = self.capacity(name);
            if n > cap {
                return Err(format!(
                    "{name}:{n} can never be granted — {name} has capacity {cap} \
                     (raise it with `fleet resource set {name} --capacity {n}`)"
                ));
            }
        }
        Ok(())
    }

    /// Add a request to the queue. Resources it names for the first time are
    /// created with [`DEFAULT_CAPACITY`]. It goes ahead of the first request of
    /// strictly lower priority that nobody placed by hand, else last — which is
    /// `(priority, enqueue time)` order until someone moves something.
    fn enqueue(&mut self, req: ResourceRequest) -> Result<(), String> {
        self.check_fits(&req.needs)?;
        if self.grants.iter().chain(&self.queue).any(|r| r.id == req.id) {
            return Err(format!("request {} is already in the resource queue", req.id));
        }
        for name in req.needs.keys() {
            self.resources
                .entry(name.clone())
                .or_insert(ResourceDef { capacity: DEFAULT_CAPACITY });
        }
        self.version = STATE_VERSION;
        let at = self
            .queue
            .iter()
            .position(|r| !r.manual && r.priority > req.priority)
            .unwrap_or(self.queue.len());
        self.queue.insert(at, req);
        Ok(())
    }

    fn queued_index(&self, id: &str) -> Result<usize, String> {
        if let Some(i) = self.queue.iter().position(|r| r.id == id) {
            return Ok(i);
        }
        if self.grants.iter().any(|r| r.id == id) {
            return Err(format!(
                "{id} is not queued — it already holds its resources and is running"
            ));
        }
        Err(format!("no queued request {id}"))
    }

    fn move_request(&mut self, id: &str, target: &MoveTarget) -> Result<(), String> {
        let from = self.queued_index(id)?;
        if let MoveTarget::Before(other) = target {
            if other == id {
                return Err("cannot move a request before itself".into());
            }
            self.queued_index(other)?;
        }
        let mut req = self.queue.remove(from);
        let to = match target {
            MoveTarget::Index(0) => {
                self.queue.insert(from, req);
                return Err("queue positions start at 1".into());
            }
            MoveTarget::Index(n) => (n - 1).min(self.queue.len()),
            MoveTarget::Before(other) => self
                .queue
                .iter()
                .position(|r| &r.id == other)
                .unwrap_or(self.queue.len()),
        };
        req.manual = true;
        self.queue.insert(to, req);
        Ok(())
    }

    /// Take a queued request out. A granted one is refused: stopping a running
    /// command is the job runner's business.
    fn cancel(&mut self, id: &str) -> Result<ResourceRequest, String> {
        let i = self.queued_index(id)?;
        Ok(self.queue.remove(i))
    }

    /// Drop `id` wherever it is. Returns whether it was there.
    fn remove(&mut self, id: &str) -> bool {
        let before = self.grants.len() + self.queue.len();
        self.grants.retain(|r| r.id != id);
        self.queue.retain(|r| r.id != id);
        before != self.grants.len() + self.queue.len()
    }

    /// Drop child entries whose grant is gone, returning the ones to kill: a
    /// grant that left with its holder dead was reaped, and its command may
    /// still run. A holder that released while alive has waited for its
    /// command already, so nothing of it is touched. `before` is the state
    /// this transaction started from — the only place a reaped grant's
    /// holder can still be looked up.
    fn settle_children(&mut self, before: &State, alive: &dyn Fn(&Holder) -> bool) -> Vec<ChildProc> {
        let mut orphans = Vec::new();
        let held: Vec<&str> = self.grants.iter().map(|g| g.id.as_str()).collect();
        self.children.retain(|id, child| {
            if held.contains(&id.as_str()) {
                return true;
            }
            let holder_dead = before
                .grants
                .iter()
                .find(|g| &g.id == id)
                .is_some_and(|g| !alive(&g.holder));
            if holder_dead {
                orphans.push(child.clone());
            }
            false
        });
        orphans
    }

    fn set_capacity(&mut self, name: &str, capacity: u32) -> Result<(), String> {
        validate_name(name)?;
        if capacity == 0 {
            return Err("capacity must be at least 1".into());
        }
        let too_big: Vec<&str> = self
            .queue
            .iter()
            .filter(|r| r.needs.get(name).is_some_and(|&n| n > capacity))
            .map(|r| r.id.as_str())
            .collect();
        if !too_big.is_empty() {
            return Err(format!(
                "queued request(s) {} need more than {capacity} of {name}; cancel them first",
                too_big.join(", ")
            ));
        }
        self.version = STATE_VERSION;
        self.resources
            .insert(name.to_string(), ResourceDef { capacity });
        Ok(())
    }

    fn remove_resource(&mut self, name: &str) -> Result<(), String> {
        let using: Vec<&str> = self
            .grants
            .iter()
            .chain(&self.queue)
            .filter(|r| r.needs.contains_key(name))
            .map(|r| r.id.as_str())
            .collect();
        if !using.is_empty() {
            return Err(format!(
                "{name} is in use by {} — only an idle resource can be removed",
                using.join(", ")
            ));
        }
        if self.resources.remove(name).is_none() {
            return Err(format!("no resource {name}"));
        }
        Ok(())
    }

    /// The table as the clients show it. Call after [`State::schedule`], so the
    /// queue holds only requests that really are waiting.
    fn snapshot(&self, now_ms: u64) -> ResourceSnapshot {
        let holders_of = |name: &str| -> Vec<String> {
            self.grants
                .iter()
                .filter(|g| g.needs.contains_key(name))
                .map(|g| g.id.clone())
                .collect()
        };
        let (_, waits) = self.plan();
        let mut wait_on: BTreeMap<usize, Vec<WaitOn>> = waits.into_iter().collect();
        let queue = self
            .queue
            .iter()
            .enumerate()
            .map(|(i, req)| {
                let mut blockers = wait_on.remove(&i).unwrap_or_default();
                for b in &mut blockers {
                    if b.reason == WaitReason::Busy {
                        b.by = holders_of(&b.resource);
                    }
                }
                QueuedRequest {
                    position: i + 1,
                    request: req.clone(),
                    wait_on: blockers,
                }
            })
            .collect();
        let used = self.in_use();
        let mut names: Vec<&String> = self.resources.keys().collect();
        // A hand-edited or older state may hold a request for an undeclared
        // resource; show it rather than hide who holds what.
        for r in self.grants.iter().chain(&self.queue) {
            for name in r.needs.keys() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names.sort();
        let resources = names
            .into_iter()
            .map(|name| ResourceView {
                name: name.clone(),
                capacity: self.capacity(name),
                in_use: used.get(name).copied().unwrap_or(0),
                holders: holders_of(name),
                waiters: self
                    .queue
                    .iter()
                    .filter(|r| r.needs.contains_key(name))
                    .map(|r| r.id.clone())
                    .collect(),
            })
            .collect();
        ResourceSnapshot {
            now_ms,
            resources,
            holders: self.grants.clone(),
            queue,
        }
    }

    /// Resolve a request made by a command that itself runs under grant
    /// `outer` (the [`GRANT_ENV`] value, if any).
    fn nesting(&self, outer: Option<&str>, needs: &BTreeMap<String, u32>) -> Result<Nesting, String> {
        let Some(outer) = outer.filter(|s| !s.is_empty()) else {
            return Ok(Nesting::TopLevel);
        };
        // A grant id that is no longer held is a leftover in the environment
        // of something that outlived its grant: nothing is held, so this is an
        // ordinary top-level request.
        let Some(held) = self.grants.iter().find(|g| g.id == outer) else {
            return Ok(Nesting::TopLevel);
        };
        let missing: BTreeMap<String, u32> = needs
            .iter()
            .filter(|(name, &n)| held.needs.get(*name).copied().unwrap_or(0) < n)
            .map(|(name, &n)| (name.clone(), n))
            .collect();
        if missing.is_empty() {
            return Ok(Nesting::Inherited(outer.to_string()));
        }
        Err(format!(
            "nested resource request refused: this command already runs under grant {outer}, \
             which holds [{}], and asked for [{}] on top. Waiting for more while holding some is \
             how deadlocks start — declare every resource on the outermost command instead \
             (add --need for [{}] there).",
            format_needs(&held.needs),
            format_needs(&missing),
            format_needs(&missing),
        ))
    }
}

/// How a request relates to the grant its caller runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nesting {
    /// Not under a grant: queue normally.
    TopLevel,
    /// Everything asked for is already held by this enclosing grant: run
    /// without queueing.
    Inherited(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Storage: one JSON file behind an exclusive file lock
// ─────────────────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `~/.fleet/resources/state.json` (under `FLEET_HOME` when that is set).
pub fn state_path() -> Result<PathBuf, String> {
    crate::session::get_fleet_dir()
        .map(|d| d.join("resources").join("state.json"))
        .ok_or_else(|| "cannot determine home dir".to_string())
}

fn load(path: &Path) -> Result<State, String> {
    use crate::atomic_json::JsonLoad;
    match crate::atomic_json::load_preserving::<State>(path) {
        JsonLoad::Loaded(s) => Ok(s),
        // A corrupt file has been renamed aside; start from an empty table.
        // Live holders are not re-learned, which is the lesser evil next to
        // every queued command on the machine failing.
        JsonLoad::Missing | JsonLoad::Corrupt => Ok(State::default()),
        JsonLoad::Unreadable => Err(format!("cannot read {}", path.display())),
    }
}

/// Run `f` on the state under the exclusive lock, persisting it if `f` changed
/// it. Unlike [`crate::atomic_json::with_file_lock`] a lock failure is an
/// error: an unlocked read-modify-write here can grant one resource twice.
fn transact<R>(path: &Path, f: impl FnOnce(&mut State) -> R) -> Result<R, String> {
    transact_as(path, Cause::None, f)
}

/// [`transact`], telling the history why a request it removes left.
fn transact_as<R>(path: &Path, cause: Cause, f: impl FnOnce(&mut State) -> R) -> Result<R, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("open {}: {e}", lock_path.display()))?;
    lock.lock()
        .map_err(|e| format!("lock {}: {e}", lock_path.display()))?;
    let mut state = load(path)?;
    let before = state.clone();
    let out = f(&mut state);
    // Whatever `f` did — a pass that reaped a dead holder, an admin removal —
    // a command left running by a dead holder dies before the new state is
    // written, i.e. before any waiter can see the grant its resources went to.
    for orphan in state.settle_children(&before, &holder_alive) {
        kill_orphan(&orphan);
    }
    let held: BTreeSet<String> = state.grants.iter().map(|g| g.id.clone()).collect();
    state.kicking.retain(|id| held.contains(id));
    let mut events = Vec::new();
    if state != before {
        events = history::events_between(&before, &state, &cause, now_ms(), &holder_alive);
        let json = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
        crate::atomic_json::write_atomic(path, &json)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    // Closing the file releases the lock.
    drop(lock);
    // History is written outside the state lock and never fails the call.
    history::append(path, &events);
    Ok(out)
}

fn holder_alive(h: &Holder) -> bool {
    h.is_alive()
}

/// How long [`kill_orphan`] waits for a killed command to be gone. It runs
/// under the state lock, so this bounds how long it can stall the queue.
const ORPHAN_KILL_WAIT: Duration = Duration::from_secs(3);

/// SIGKILL a reaped grant's command — its process group when it has one —
/// and wait (bounded) until it is gone. Guarded against pid reuse: a live
/// process at that pid with another start time is somebody else's.
#[cfg(unix)]
fn kill_orphan(child: &ChildProc) {
    let pid = child.pid as libc::pid_t;
    if pid <= 1 {
        return;
    }
    let leader_alive = crate::session::is_process_alive(child.pid);
    if leader_alive && !crate::proc_runner::host_alive(child.pid, child.start_time) {
        return;
    }
    // With the leader gone, a group with its id can only be the rest of the
    // original group: a pid is not handed out while a group of that id lives.
    let target = if child.own_group { -pid } else if leader_alive { pid } else { return };
    if unsafe { libc::kill(target, libc::SIGKILL) } != 0 {
        return;
    }
    crate::log_debug(&format!(
        "resource_queue: killed the orphaned command {target} of a dead holder"
    ));
    let deadline = Instant::now() + ORPHAN_KILL_WAIT;
    while Instant::now() < deadline {
        let gone = unsafe { libc::kill(target, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// No process groups to kill by id here; an orphaned command on Windows runs
/// on (the reaper only frees its resources).
#[cfg(not(unix))]
fn kill_orphan(_child: &ChildProc) {}

/// Record the command grant `id` just started, so it is killed if the grant's
/// holder dies first ([`ChildProc`]). `own_group`: the command leads its own
/// process group. A no-op when `id` holds nothing (an inherited job).
pub fn record_child(id: &str, pid: u32, own_group: bool) -> Result<(), String> {
    record_child_at(&state_path()?, id, pid, own_group)
}

fn record_child_at(path: &Path, id: &str, pid: u32, own_group: bool) -> Result<(), String> {
    let child = ChildProc {
        pid,
        start_time: crate::session::process_start_time(pid),
        own_group,
    };
    transact(path, |s| {
        if s.grants.iter().any(|g| g.id == id) {
            s.children.insert(id.to_string(), child);
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API — read / admin (callers: CLI now; fleet serve, Tauri, relay next)
// ─────────────────────────────────────────────────────────────────────────────

/// The current table. Runs a scheduling pass first, so dead holders and
/// waiters are already gone from what it returns.
pub fn snapshot() -> Result<ResourceSnapshot, String> {
    snapshot_at(&state_path()?)
}

fn snapshot_at(path: &Path) -> Result<ResourceSnapshot, String> {
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&holder_alive, now);
        s.snapshot(now)
    })
}

/// Move a queued request by hand. Returns the table afterwards.
pub fn move_request(id: &str, target: &MoveTarget) -> Result<ResourceSnapshot, String> {
    move_request_at(&state_path()?, id, target)
}

fn move_request_at(path: &Path, id: &str, target: &MoveTarget) -> Result<ResourceSnapshot, String> {
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&holder_alive, now);
        s.move_request(id, target)?;
        s.schedule(&holder_alive, now);
        Ok(s.snapshot(now))
    })?
}

/// Take a queued request out of the queue; its command never runs and ends
/// with [`EXIT_QUEUE_CANCELLED`]. Refused for a request that is already
/// running — stop that with `fleet job stop`.
pub fn cancel_request(id: &str) -> Result<ResourceSnapshot, String> {
    cancel_request_at(&state_path()?, id)
}

fn cancel_request_at(path: &Path, id: &str) -> Result<ResourceSnapshot, String> {
    let now = now_ms();
    transact_as(path, Cause::Cancelled(id.to_string()), |s| {
        s.schedule(&holder_alive, now);
        s.cancel(id)?;
        s.schedule(&holder_alive, now);
        Ok(s.snapshot(now))
    })?
}

/// Set a resource's capacity, creating it if needed. Lowering it below what is
/// in use is allowed (nothing more is granted until enough is released);
/// lowering it below what a queued request needs is refused.
pub fn set_capacity(name: &str, capacity: u32) -> Result<ResourceSnapshot, String> {
    set_capacity_at(&state_path()?, name, capacity)
}

fn set_capacity_at(path: &Path, name: &str, capacity: u32) -> Result<ResourceSnapshot, String> {
    let now = now_ms();
    transact_as(path, Cause::SetCapacity(name.to_string()), |s| {
        s.schedule(&holder_alive, now);
        s.set_capacity(name, capacity)?;
        s.schedule(&holder_alive, now);
        Ok(s.snapshot(now))
    })?
}

/// Delete a resource from the table — for one created by a typo in `--need`.
/// Refused while anything holds it or waits for it: the next request naming
/// it would only re-create it, and a holder's grant must stay explainable.
pub fn remove_resource(name: &str) -> Result<ResourceSnapshot, String> {
    remove_resource_at(&state_path()?, name)
}

fn remove_resource_at(path: &Path, name: &str) -> Result<ResourceSnapshot, String> {
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&holder_alive, now);
        s.remove_resource(name)?;
        Ok(s.snapshot(now))
    })?
}

/// How long [`kick`] gives a holder to let go after each signal: SIGTERM, then
/// SIGKILL.
pub const KICK_GRACE: Duration = Duration::from_secs(3);

/// Evict a holder: stop whatever runs under grant `id` and take its resources
/// back. Blocks until the grant is gone (at most twice [`KICK_GRACE`]), so the
/// returned table already shows the next request granted.
///
/// * a **job** is stopped the way `fleet job stop` does it — its command's
///   process group gets SIGTERM, then SIGKILL — and its host releases the
///   grant when the command exits;
/// * a **foreground** `fleet resource run` gets SIGTERM, which it forwards to
///   its command before releasing; if it is still holding after the grace,
///   its children and then it get SIGKILL and the dead holder is reaped.
///
/// The resources are never handed on while the evicted command may still be
/// running: the grant goes only when its holder lets go or is dead. Every
/// signal is preceded by the `(pid, start time)` check, so a recycled pid is
/// never signalled. A queued request is refused — that is [`cancel_request`].
pub fn kick(id: &str) -> Result<ResourceSnapshot, String> {
    kick_at(&state_path()?, id, KICK_GRACE)
}

fn kick_at(path: &Path, id: &str, grace: Duration) -> Result<ResourceSnapshot, String> {
    let now = now_ms();
    let held = transact(path, |s| {
        s.schedule(&holder_alive, now);
        if let Some(g) = s.grants.iter().find(|g| g.id == id).cloned() {
            s.kicking.insert(id.to_string());
            return Ok(g);
        }
        if s.queue.iter().any(|r| r.id == id) {
            return Err(format!(
                "{id} is still queued and holds nothing — cancel it instead"
            ));
        }
        Err(format!("no request {id} holds a resource"))
    })??;

    for force in [false, true] {
        evict::signal(&held, force);
        if wait_released(path, id, grace)? {
            return snapshot_at(path);
        }
    }
    Err(format!(
        "{id} still holds its resources {} after SIGKILL (pid {})",
        fmt_duration(2 * grace.as_millis() as u64),
        held.holder.pid
    ))
}

/// Poll until `id` no longer holds anything, running the pass each time so a
/// dead holder is reaped. Returns whether it let go within `limit`.
fn wait_released(path: &Path, id: &str, limit: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + limit;
    loop {
        let now = now_ms();
        let holding = transact(path, |s| {
            s.schedule(&holder_alive, now);
            s.grants.iter().any(|g| g.id == id)
        })?;
        if !holding {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The signals behind [`kick`].
mod evict {
    use super::{Holder, RequestKind, ResourceRequest};

    pub(super) fn signal(req: &ResourceRequest, force: bool) {
        // A job's command runs in its own process group, which `kill_proc`
        // targets. Fall back to the host process if the job record is gone.
        if req.kind == RequestKind::Job
            && crate::proc_runner::kill_proc(&req.id, force).is_ok()
        {
            return;
        }
        if !req.holder.is_alive() {
            return;
        }
        signal_holder(&req.holder, force);
    }

    #[cfg(unix)]
    fn signal_holder(holder: &Holder, force: bool) {
        let pid = holder.pid as libc::pid_t;
        if force {
            // `fleet resource run` waits for its command and only then
            // releases; SIGKILLing it alone would orphan the command while
            // the grant is reaped. Kill the command first.
            for child in children_of(holder.pid) {
                unsafe { libc::kill(child as libc::pid_t, libc::SIGKILL) };
            }
            // Re-check: the holder may have exited, and its pid been reused,
            // while the children were being killed.
            if holder.is_alive() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        } else {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }

    #[cfg(unix)]
    pub(super) fn children_of(pid: u32) -> Vec<u32> {
        crate::process_util::command("pgrep")
            .args(["-P", &pid.to_string()])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// No graceful tier on Windows: `taskkill /T` takes the holder and its
    /// command tree down together, and only `/F` reliably ends a console
    /// process, so both rounds force it.
    #[cfg(not(unix))]
    fn signal_holder(holder: &Holder, _force: bool) {
        let _ = crate::process_util::command("taskkill")
            .args(["/F", "/T", "/PID", &holder.pid.to_string()])
            .status();
    }
}

/// Whether a request for `needs` could ever be granted, without queueing it.
/// Lets a caller refuse a bad request before it starts a detached host.
pub fn check_fits(needs: &BTreeMap<String, u32>) -> Result<(), String> {
    transact(&state_path()?, |s| s.check_fits(needs))?
}

/// Resolve a request made from inside a command that already runs under a
/// grant ([`GRANT_ENV`] in the caller's environment):
///
/// * everything it asks for is held by that grant → [`Nesting::Inherited`],
///   run without queueing;
/// * it asks for anything else → an error telling the agent to declare every
///   resource on the outermost command. Waiting for more while holding some is
///   the one way a deadlock could still form, so it is refused, not detected.
pub fn check_nested(needs: &BTreeMap<String, u32>) -> Result<Nesting, String> {
    let outer = std::env::var(GRANT_ENV).ok();
    check_nested_at(&state_path()?, outer.as_deref(), needs)
}

fn check_nested_at(
    path: &Path,
    outer: Option<&str>,
    needs: &BTreeMap<String, u32>,
) -> Result<Nesting, String> {
    if outer.is_none_or(str::is_empty) {
        return Ok(Nesting::TopLevel);
    }
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&holder_alive, now);
        s.nesting(outer, needs)
    })?
}

/// Plain-text table for `fleet resource ls`.
pub fn render_snapshot(snap: &ResourceSnapshot) -> String {
    if snap.resources.is_empty() {
        return "no resources yet — one is created the first time a command names it with --need\n".into();
    }
    let short = |cmd: &str| -> String {
        let one_line = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
        if one_line.chars().count() > 60 {
            format!("{}…", one_line.chars().take(59).collect::<String>())
        } else {
            one_line
        }
    };
    let describe = |r: &ResourceRequest| -> String {
        let what = if r.command.is_empty() { "(private)".to_string() } else { short(&r.command) };
        let mut s = format!("{}  {what}", r.id);
        if let Some(sid) = &r.session_id {
            s.push_str(&format!("  [session {}]", sid.chars().take(8).collect::<String>()));
        }
        s
    };
    let mut out = String::new();
    for res in &snap.resources {
        out.push_str(&format!(
            "{}  {}/{} in use\n",
            res.name, res.in_use, res.capacity
        ));
        for id in &res.holders {
            if let Some(r) = snap.holders.iter().find(|r| &r.id == id) {
                let held = snap.now_ms.saturating_sub(r.granted_ms.unwrap_or(r.enqueued_ms));
                out.push_str(&format!(
                    "    held {}  {}\n",
                    fmt_duration(held),
                    describe(r)
                ));
            }
        }
    }
    if snap.queue.is_empty() {
        out.push_str("\nqueue: empty\n");
        return out;
    }
    out.push_str("\nqueue:\n");
    for q in &snap.queue {
        let r = &q.request;
        let waited = snap.now_ms.saturating_sub(r.enqueued_ms);
        out.push_str(&format!(
            "  #{}  {}\n      needs {} · {} priority{} · waiting {} on {}\n",
            q.position,
            describe(r),
            format_needs(&r.needs),
            r.priority,
            if r.manual { " · placed by hand" } else { "" },
            fmt_duration(waited),
            describe_wait(&q.wait_on),
        ));
    }
    out
}

pub(crate) fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match (s / 3600, (s / 60) % 60, s % 60) {
        (0, 0, sec) => format!("{sec}s"),
        (0, m, sec) => format!("{m}m{sec:02}s"),
        (h, m, sec) => format!("{h}h{m:02}m{sec:02}s"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API — acquire / release (callers: the job host, `fleet resource run`)
// ─────────────────────────────────────────────────────────────────────────────

/// A request to queue on behalf of the calling process.
#[derive(Debug, Clone)]
pub struct AcquireSpec {
    /// Request id — the job id for a job, [`new_request_id`] otherwise.
    pub id: String,
    pub kind: RequestKind,
    pub needs: BTreeMap<String, u32>,
    pub priority: Priority,
    /// Give up after this long in the queue. `None` waits forever.
    pub max_wait: Option<Duration>,
    pub command: String,
    pub workspace: Option<String>,
    pub session_id: Option<String>,
}

/// Where a queued request stands, passed to [`acquire`]'s observer each poll.
#[derive(Debug, Clone)]
pub struct Waiting {
    pub position: usize,
    pub wait_on: Vec<WaitOn>,
    pub waited: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireError {
    /// The request was refused, or the table could not be read or locked.
    Rejected(String),
    /// `max_wait` passed. The request is out of the queue; nothing was held.
    Timeout { waited: Duration, wait_on: Vec<WaitOn> },
    /// Someone cancelled the queued request.
    Cancelled,
    /// The observer asked to stop waiting. The request is out of the queue.
    Aborted,
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(e) => write!(f, "{e}"),
            Self::Timeout { waited, wait_on } => write!(
                f,
                "gave up after waiting {} for {} (--max-wait); the command was never started",
                fmt_duration(waited.as_millis() as u64),
                describe_wait(wait_on)
            ),
            Self::Cancelled => write!(
                f,
                "the queued request was cancelled; the command was never started"
            ),
            Self::Aborted => write!(f, "interrupted while queued; the command was never started"),
        }
    }
}

/// Held resources. They are released by [`Grant::release`], by dropping this,
/// or — if the process dies first — by the next scheduling pass anyone runs.
#[derive(Debug)]
pub struct Grant {
    id: String,
    path: PathBuf,
    released: bool,
    /// The command's exit code, for the history ([`Grant::release_with_exit`]).
    exit_code: Option<i32>,
}

impl Grant {
    /// The request id — the value to export as [`GRANT_ENV`] to the command.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn release(mut self) {
        self.release_now();
    }

    /// Release, recording the command's exit code in the history.
    pub fn release_with_exit(mut self, code: i32) {
        self.exit_code = Some(code);
        self.release_now();
    }

    fn release_now(&mut self) {
        if std::mem::replace(&mut self.released, true) {
            return;
        }
        let now = now_ms();
        let id = self.id.clone();
        let cause = Cause::Released { id: id.clone(), exit_code: self.exit_code };
        // Grant whatever this frees straight away rather than leaving it to
        // the next waiter's poll.
        let _ = transact_as(&self.path, cause, |s| {
            s.remove(&id);
            s.schedule(&holder_alive, now);
        });
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.release_now();
    }
}

/// An id for a request that is not a job.
pub fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("r{:x}{:x}{:x}", now_ms(), std::process::id() & 0xfff, seq & 0xff)
}

/// Queue `spec` for the calling process and block until every resource in it
/// is granted at once. Nothing is held while this waits.
///
/// `observe` is called on every poll that finds the request still queued;
/// returning `true` gives up ([`AcquireError::Aborted`]).
pub fn acquire(
    spec: AcquireSpec,
    observe: impl FnMut(&Waiting) -> bool,
) -> Result<Grant, AcquireError> {
    let path = state_path().map_err(AcquireError::Rejected)?;
    acquire_at(&path, spec, Holder::current(), POLL, observe)
}

enum Step {
    Granted,
    Waiting(Waiting),
    Gone,
    TimedOut(Vec<WaitOn>),
}

fn acquire_at(
    path: &Path,
    spec: AcquireSpec,
    holder: Holder,
    poll: Duration,
    mut observe: impl FnMut(&Waiting) -> bool,
) -> Result<Grant, AcquireError> {
    let started = Instant::now();
    let id = spec.id.clone();
    let request = ResourceRequest {
        id: id.clone(),
        kind: spec.kind,
        needs: spec.needs,
        priority: spec.priority,
        enqueued_ms: now_ms(),
        granted_ms: None,
        manual: false,
        holder,
        command: spec.command,
        workspace: spec.workspace,
        session_id: spec.session_id,
    };
    let now = now_ms();
    transact(path, |s| {
        s.schedule(&holder_alive, now);
        s.enqueue(request)
    })
    .and_then(|r| r)
    .map_err(AcquireError::Rejected)?;

    let leave = |path: &Path| {
        let now = now_ms();
        let _ = transact_as(path, Cause::Aborted(id.clone()), |s| {
            s.remove(&id);
            s.schedule(&holder_alive, now);
        });
    };
    loop {
        let now = now_ms();
        let waited = started.elapsed();
        let expired = spec.max_wait.is_some_and(|max| waited >= max);
        // Grant-or-give-up is decided in one transaction: checking the deadline
        // outside it could abandon a request that was granted in between.
        let cause = if expired { Cause::TimedOut(id.clone()) } else { Cause::None };
        let step = transact_as(path, cause, |s| {
            s.schedule(&holder_alive, now);
            if s.grants.iter().any(|g| g.id == id) {
                return Step::Granted;
            }
            let snap = s.snapshot(now);
            let Some(mine) = snap.queue.into_iter().find(|q| q.request.id == id) else {
                return Step::Gone;
            };
            if expired {
                s.remove(&id);
                s.schedule(&holder_alive, now);
                return Step::TimedOut(mine.wait_on);
            }
            Step::Waiting(Waiting {
                position: mine.position,
                wait_on: mine.wait_on,
                waited,
            })
        });
        match step {
            Ok(Step::Granted) => {
                return Ok(Grant {
                    id,
                    path: path.to_path_buf(),
                    released: false,
                    exit_code: None,
                })
            }
            Ok(Step::Gone) => return Err(AcquireError::Cancelled),
            Ok(Step::TimedOut(wait_on)) => return Err(AcquireError::Timeout { waited, wait_on }),
            Ok(Step::Waiting(w)) => {
                if observe(&w) {
                    leave(path);
                    return Err(AcquireError::Aborted);
                }
            }
            Err(e) => {
                leave(path);
                return Err(AcquireError::Rejected(e));
            }
        }
        let nap = match spec.max_wait {
            Some(max) => poll.min(max.saturating_sub(started.elapsed())),
            None => poll,
        };
        std::thread::sleep(nap.max(Duration::from_millis(10)));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Foreground runner — `fleet resource run --need … -- <command>`
// ─────────────────────────────────────────────────────────────────────────────

/// A command to run in the foreground under a grant.
#[derive(Debug, Clone)]
pub struct ForegroundSpec {
    pub needs: BTreeMap<String, u32>,
    pub priority: Priority,
    pub max_wait: Option<Duration>,
    /// Program and arguments, executed directly (no shell).
    pub argv: Vec<String>,
    pub session_id: Option<String>,
}

/// Queue in the calling process, then run `argv` with this process's stdio,
/// release when it exits and return its exit code (`128 + signal` if a signal
/// killed it). For wrappers — the cargo shim — that need the command's
/// stdin/stdout untouched and its exit status exact.
///
/// The command is never started without its resources: a wait that ends any
/// other way returns [`EXIT_QUEUE_TIMEOUT`], [`EXIT_QUEUE_CANCELLED`] or
/// [`EXIT_QUEUE_REJECTED`] after one line on stderr.
///
/// On Unix, SIGINT / SIGTERM / SIGHUP / SIGQUIT sent to this process are
/// forwarded to the command, and this process keeps waiting for it either way,
/// so the resources are never released while the command still runs. The one
/// exception is a keyboard signal (Ctrl-C, Ctrl-\) while this process is in
/// its terminal's foreground group: the terminal has already sent that to the
/// command, and it is not sent a second time.
pub fn run_foreground(spec: ForegroundSpec) -> i32 {
    let Some(program) = spec.argv.first() else {
        eprintln!("fleet resource run: no command given (put it after `--`)");
        return EXIT_QUEUE_REJECTED;
    };
    let nesting = match check_nested(&spec.needs) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("fleet resource run: {e}");
            return EXIT_QUEUE_REJECTED;
        }
    };
    #[cfg(unix)]
    fg_signals::install();

    let grant = match nesting {
        Nesting::Inherited(_) => None,
        Nesting::TopLevel => {
            let mut announced = false;
            let acquired = acquire(
                AcquireSpec {
                    id: new_request_id(),
                    kind: RequestKind::Foreground,
                    needs: spec.needs.clone(),
                    priority: spec.priority,
                    max_wait: spec.max_wait,
                    command: spec.argv.join(" "),
                    workspace: std::env::current_dir()
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned()),
                    session_id: spec.session_id.clone(),
                },
                |w| {
                    if !announced {
                        announced = true;
                        eprintln!(
                            "fleet: queued (#{}) for {} — waiting on {}",
                            w.position,
                            format_needs(&spec.needs),
                            describe_wait(&w.wait_on)
                        );
                    }
                    caught_signal().is_some()
                },
            );
            match acquired {
                Ok(g) => Some(g),
                Err(AcquireError::Aborted) => return 128 + caught_signal().unwrap_or(15),
                Err(e) => {
                    eprintln!("fleet resource run: {e}");
                    return match e {
                        AcquireError::Timeout { .. } => EXIT_QUEUE_TIMEOUT,
                        AcquireError::Cancelled => EXIT_QUEUE_CANCELLED,
                        _ => EXIT_QUEUE_REJECTED,
                    };
                }
            }
        }
    };

    let mut cmd = std::process::Command::new(program);
    cmd.args(&spec.argv[1..]);
    if let Some(g) = &grant {
        cmd.env(GRANT_ENV, g.id());
    }
    // Off a terminal's foreground (an agent's shell, CI, a job), the command
    // gets its own process group, so the whole of it — cargo and its rustc
    // children — can be killed if this process is killed outright. In the
    // foreground of a terminal it stays in ours: that is the group Ctrl-C
    // and the terminal's input go to.
    #[cfg(unix)]
    let own_group = !fg_signals::in_foreground_group();
    #[cfg(not(unix))]
    let own_group = false;
    #[cfg(unix)]
    if own_group {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let code = match cmd.spawn() {
        Ok(mut child) => {
            #[cfg(unix)]
            fg_signals::set_child(child.id(), own_group);
            if let Some(g) = &grant {
                if let Err(e) = record_child_at(&g.path, g.id(), child.id(), own_group) {
                    eprintln!("fleet resource run: could not record the command in the queue: {e}");
                }
            }
            let status = child.wait();
            #[cfg(unix)]
            fg_signals::set_child(0, false);
            match status {
                Ok(status) => exit_code_of(status),
                Err(e) => {
                    eprintln!("fleet resource run: wait for {program}: {e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("fleet resource run: cannot run {program}: {e}");
            if e.kind() == std::io::ErrorKind::NotFound { 127 } else { 126 }
        }
    };
    if let Some(g) = grant {
        g.release_with_exit(code);
    }
    code
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

fn caught_signal() -> Option<i32> {
    #[cfg(unix)]
    {
        fg_signals::caught()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Signal plumbing for [`run_foreground`]: remember the last termination
/// signal, and pass it on to the running command.
#[cfg(unix)]
mod fg_signals {
    use std::sync::atomic::{AtomicI32, Ordering};

    /// The running command's pid — negated when it leads its own process
    /// group, so a forwarded signal reaches the whole group.
    static CHILD: AtomicI32 = AtomicI32::new(0);
    static CAUGHT: AtomicI32 = AtomicI32::new(0);

    extern "C" fn on_signal(sig: libc::c_int) {
        CAUGHT.store(sig, Ordering::SeqCst);
        let target = CHILD.load(Ordering::SeqCst);
        if target == 0 {
            return;
        }
        // Ctrl-C / Ctrl-\ are raised by the terminal for its whole foreground
        // process group, so a command in that group with us has its own copy.
        // The sender pid cannot tell that case apart: measured on macOS 27, a
        // terminal-raised SIGINT arrives with a non-zero `si_pid`. A command
        // in its own group (target < 0) never shares our foreground.
        let keyboard = sig == libc::SIGINT || sig == libc::SIGQUIT;
        if keyboard && target > 0 && in_foreground_group() {
            return;
        }
        unsafe { libc::kill(target, sig) };
    }

    /// Whether this process is in the foreground process group of a terminal
    /// on its stdio. Only async-signal-safe calls: it runs in the handler.
    pub(super) fn in_foreground_group() -> bool {
        let me = unsafe { libc::getpgrp() };
        (0..=2).any(|fd| unsafe { libc::tcgetpgrp(fd) } == me)
    }

    pub(super) fn install() {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_signal as usize;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(sig, &action, std::ptr::null_mut());
            }
        }
    }

    pub(super) fn set_child(pid: u32, own_group: bool) {
        let pid = pid as i32;
        CHILD.store(if own_group { -pid } else { pid }, Ordering::SeqCst);
    }

    pub(super) fn caught() -> Option<i32> {
        match CAUGHT.load(Ordering::SeqCst) {
            0 => None,
            sig => Some(sig),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs(spec: &[&str]) -> BTreeMap<String, u32> {
        parse_needs(spec).unwrap()
    }

    fn req(id: &str, spec: &[&str]) -> ResourceRequest {
        req_pid(id, spec, 1)
    }

    fn req_pid(id: &str, spec: &[&str], pid: u32) -> ResourceRequest {
        ResourceRequest {
            id: id.into(),
            kind: RequestKind::Job,
            needs: needs(spec),
            priority: Priority::Normal,
            enqueued_ms: 0,
            granted_ms: None,
            manual: false,
            holder: Holder { pid, start_time: Some(1) },
            command: format!("cmd-{id}"),
            workspace: None,
            session_id: None,
        }
    }

    fn all_alive(_: &Holder) -> bool {
        true
    }

    fn ids(list: &[ResourceRequest]) -> Vec<&str> {
        list.iter().map(|r| r.id.as_str()).collect()
    }

    /// Enqueue then run a pass, as every real caller does.
    fn submit(s: &mut State, r: ResourceRequest) -> Pass {
        s.enqueue(r).unwrap();
        s.schedule(&all_alive, 1)
    }

    fn finish(s: &mut State, id: &str) -> Pass {
        assert!(s.remove(id), "{id} was not in the table");
        s.schedule(&all_alive, 1)
    }

    #[test]
    fn need_specs_parse_names_units_and_repeats() {
        assert_eq!(parse_need("test-db").unwrap(), ("test-db".into(), 1));
        assert_eq!(parse_need("gpu:2").unwrap(), ("gpu".into(), 2));
        assert_eq!(parse_need("claude-fleet/e2e").unwrap(), ("claude-fleet/e2e".into(), 1));
        assert!(parse_need("gpu:0").is_err());
        assert!(parse_need("gpu:two").is_err());
        assert!(parse_need("").is_err());
        assert!(parse_need("has space").is_err());
        assert_eq!(needs(&["gpu", "gpu", "db"]), needs(&["gpu:2", "db"]));
        assert_eq!(format_needs(&needs(&["gpu:2", "db"])), "db, gpu:2");
    }

    #[test]
    fn a_first_use_creates_the_resource_with_capacity_one() {
        let mut s = State::default();
        let pass = submit(&mut s, req("a", &["db"]));
        assert_eq!(pass.granted, vec!["a"]);
        assert_eq!(s.resources["db"].capacity, 1);
        assert!(s.grants[0].granted_ms.is_some());

        let pass = submit(&mut s, req("b", &["db"]));
        assert!(pass.granted.is_empty(), "capacity 1 is exclusive");
        assert_eq!(ids(&s.queue), vec!["b"]);

        let pass = finish(&mut s, "a");
        assert_eq!(pass.granted, vec!["b"]);
    }

    /// The property the whole design rests on: a request that cannot have
    /// everything holds nothing while it waits.
    #[test]
    fn a_request_is_granted_everything_or_holds_nothing() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        let pass = submit(&mut s, req("both", &["a", "b"]));
        assert!(pass.granted.is_empty());
        assert_eq!(s.in_use().get("b"), None, "b is free and must stay unheld by the waiter");
        assert_eq!(ids(&s.grants), vec!["holder"]);

        let pass = finish(&mut s, "holder");
        assert_eq!(pass.granted, vec!["both"]);
        assert_eq!(s.in_use(), needs(&["a", "b"]));
    }

    /// A multi-resource request at the head is not starved: later requests may
    /// not take a resource it is waiting for, even when that resource is free.
    #[test]
    fn requests_behind_a_waiting_head_cannot_take_what_it_reserved() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        submit(&mut s, req("head", &["a", "b"]));
        for i in 0..5 {
            let pass = submit(&mut s, req(&format!("small{i}"), &["b"]));
            assert!(pass.granted.is_empty(), "b is reserved for head");
        }
        let snap = s.snapshot(1);
        assert_eq!(snap.queue[0].wait_on, vec![WaitOn {
            resource: "a".into(),
            reason: WaitReason::Busy,
            by: vec!["holder".into()],
        }]);
        assert_eq!(snap.queue[1].wait_on, vec![WaitOn {
            resource: "b".into(),
            reason: WaitReason::Reserved,
            by: vec!["head".into()],
        }]);

        let pass = finish(&mut s, "holder");
        assert_eq!(pass.granted, vec!["head"], "head goes first, alone");
        let pass = finish(&mut s, "head");
        assert_eq!(pass.granted, vec!["small0"]);
    }

    /// Conservative backfill: a request for resources nobody ahead wants runs
    /// straight away instead of idling behind the queue.
    #[test]
    fn an_unrelated_request_runs_ahead_of_the_queue() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        submit(&mut s, req("head", &["a", "b"]));
        let pass = submit(&mut s, req("other", &["c"]));
        assert_eq!(pass.granted, vec!["other"]);
        assert_eq!(ids(&s.queue), vec!["head"]);
    }

    #[test]
    fn capacity_counts_units() {
        let mut s = State::default();
        s.set_capacity("gpu", 3).unwrap();
        assert_eq!(submit(&mut s, req("one", &["gpu"])).granted, vec!["one"]);
        assert_eq!(submit(&mut s, req("two", &["gpu:2"])).granted, vec!["two"]);
        assert!(submit(&mut s, req("three", &["gpu"])).granted.is_empty());
        let snap = s.snapshot(1);
        assert_eq!(snap.resources[0].in_use, 3);
        assert_eq!(snap.resources[0].holders, vec!["one", "two"]);
        assert_eq!(snap.resources[0].waiters, vec!["three"]);
        assert_eq!(finish(&mut s, "two").granted, vec!["three"]);
    }

    /// With capacity above one, a small request still may not slip past a
    /// bigger one that is waiting for the same resource.
    #[test]
    fn a_big_request_is_not_overtaken_on_a_multi_unit_resource() {
        let mut s = State::default();
        s.set_capacity("gpu", 2).unwrap();
        submit(&mut s, req("one", &["gpu"]));
        assert!(submit(&mut s, req("big", &["gpu:2"])).granted.is_empty());
        assert!(
            submit(&mut s, req("small", &["gpu"])).granted.is_empty(),
            "one unit is free, but it is reserved for big"
        );
        assert_eq!(finish(&mut s, "one").granted, vec!["big"]);
    }

    #[test]
    fn a_request_that_can_never_fit_is_refused_up_front() {
        let mut s = State::default();
        let err = s.enqueue(req("greedy", &["gpu:2"])).unwrap_err();
        assert!(err.contains("capacity 1"), "{err}");
        assert!(err.contains("fleet resource set gpu --capacity 2"), "{err}");
        assert!(s.queue.is_empty() && s.resources.is_empty(), "a refused request leaves no trace");
        assert!(s.enqueue(req("empty", &[])).is_err());

        submit(&mut s, req("a", &["db"]));
        assert!(s.enqueue(req("a", &["db"])).unwrap_err().contains("already"));
    }

    #[test]
    fn the_queue_orders_by_priority_then_arrival() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        let mut with = |id: &str, p: Priority| {
            let mut r = req(id, &["a"]);
            r.priority = p;
            submit(&mut s, r);
        };
        with("n1", Priority::Normal);
        with("l1", Priority::Low);
        with("h1", Priority::High);
        with("n2", Priority::Normal);
        with("h2", Priority::High);
        assert_eq!(ids(&s.queue), vec!["h1", "h2", "n1", "n2", "l1"]);
    }

    #[test]
    fn a_manual_move_reorders_and_survives_later_arrivals() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        for id in ["n1", "n2", "n3"] {
            submit(&mut s, req(id, &["a"]));
        }
        s.move_request("n3", &MoveTarget::Index(1)).unwrap();
        assert_eq!(ids(&s.queue), vec!["n3", "n1", "n2"]);
        assert!(s.queue[0].manual);
        s.move_request("n1", &MoveTarget::Index(99)).unwrap();
        assert_eq!(ids(&s.queue), vec!["n3", "n2", "n1"]);
        s.move_request("n1", &MoveTarget::Before("n2".into())).unwrap();
        assert_eq!(ids(&s.queue), vec!["n3", "n1", "n2"]);

        // A high-priority arrival goes ahead of the untouched normal request
        // but not ahead of the ones somebody placed by hand.
        let mut high = req("h1", &["a"]);
        high.priority = Priority::High;
        submit(&mut s, high);
        assert_eq!(ids(&s.queue), vec!["n3", "n1", "h1", "n2"]);

        assert!(s.move_request("n1", &MoveTarget::Index(0)).is_err());
        assert_eq!(ids(&s.queue), vec!["n3", "n1", "h1", "n2"], "a refused move changes nothing");
        assert!(s.move_request("n1", &MoveTarget::Before("n1".into())).is_err());
        assert!(s.move_request("n1", &MoveTarget::Before("nope".into())).is_err());
        assert!(s.move_request("holder", &MoveTarget::Index(1)).unwrap_err().contains("running"));

        // The order someone chose is the order they are served in.
        assert_eq!(finish(&mut s, "holder").granted, vec!["n3"]);
    }

    #[test]
    fn cancel_takes_a_queued_request_out_and_refuses_a_running_one() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        submit(&mut s, req("head", &["a", "b"]));
        submit(&mut s, req("small", &["b"]));
        assert!(s.cancel("holder").unwrap_err().contains("running"));
        assert!(s.cancel("nope").is_err());
        assert_eq!(s.cancel("head").unwrap().id, "head");
        // With head gone, nothing reserves b any more.
        assert_eq!(s.schedule(&all_alive, 1).granted, vec!["small"]);
    }

    #[test]
    fn capacity_changes_apply_to_the_next_pass() {
        let mut s = State::default();
        submit(&mut s, req("a", &["db"]));
        submit(&mut s, req("b", &["db"]));
        s.set_capacity("db", 2).unwrap();
        assert_eq!(s.schedule(&all_alive, 1).granted, vec!["b"]);

        // Lowering below what is in use grants nothing new until it drains.
        s.set_capacity("db", 1).unwrap();
        submit(&mut s, req("c", &["db"]));
        assert!(finish(&mut s, "a").granted.is_empty(), "still one holder at capacity 1");
        assert_eq!(finish(&mut s, "b").granted, vec!["c"]);

        assert!(s.set_capacity("db", 0).is_err());
        s.set_capacity("gpu", 2).unwrap();
        submit(&mut s, req("x", &["gpu:2"]));
        submit(&mut s, req("y", &["gpu:2"]));
        let err = s.set_capacity("gpu", 1).unwrap_err();
        assert!(err.contains('y'), "names the queued request it would strand: {err}");
    }

    /// A dead holder frees its resources and a dead waiter leaves the queue —
    /// including a dead head that was reserving something for itself.
    #[test]
    fn dead_holders_and_dead_waiters_are_reaped_on_every_pass() {
        let mut s = State::default();
        submit(&mut s, req_pid("holder", &["a"], 10));
        submit(&mut s, req_pid("head", &["a", "b"], 20));
        submit(&mut s, req_pid("small", &["b"], 30));
        assert!(s.grants.len() == 1 && s.queue.len() == 2);

        // The waiting head dies: its reservation of b goes with it.
        let pass = s.schedule(&|h: &Holder| h.pid != 20, 2);
        assert_eq!(pass.reaped, vec!["head"]);
        assert_eq!(pass.granted, vec!["small"]);

        // The holder dies: a is free for the next request.
        submit(&mut s, req_pid("next", &["a"], 40));
        let pass = s.schedule(&|h: &Holder| h.pid != 10, 3);
        assert_eq!(pass.reaped, vec!["holder"]);
        assert_eq!(pass.granted, vec!["next"]);
    }

    /// A command a dead holder left running is handed back for killing; one
    /// whose holder released while alive (it waited for its command) is not.
    #[test]
    fn only_a_reaped_grant_leaves_its_command_to_be_killed() {
        let child = |pid| ChildProc { pid, start_time: Some(7), own_group: true };
        let mut s = State::default();
        submit(&mut s, req_pid("dead", &["a"], 10));
        submit(&mut s, req_pid("done", &["b"], 20));
        submit(&mut s, req_pid("running", &["c"], 30));
        s.children.insert("dead".into(), child(11));
        s.children.insert("done".into(), child(21));
        s.children.insert("running".into(), child(31));
        let before = s.clone();
        let alive = |h: &Holder| h.pid != 10;

        assert!(s.remove("done"), "released by its live holder");
        let pass = s.schedule(&alive, 2);
        assert_eq!(pass.reaped, vec!["dead"]);
        let orphans = s.settle_children(&before, &alive);
        assert_eq!(orphans, vec![child(11)], "only the dead holder's command");
        assert_eq!(s.children.keys().collect::<Vec<_>>(), vec!["running"], "entries live as long as their grant");
        assert!(s.settle_children(&s.clone(), &alive).is_empty(), "settling again changes nothing");
    }

    #[test]
    fn a_pass_is_idempotent() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        submit(&mut s, req("head", &["a", "b"]));
        submit(&mut s, req("small", &["b"]));
        let before = s.clone();
        assert_eq!(s.schedule(&all_alive, 9), Pass::default());
        assert_eq!(s, before);
    }

    #[test]
    fn a_nested_request_inherits_what_is_held_and_is_refused_anything_else() {
        let mut s = State::default();
        s.set_capacity("gpu", 2).unwrap();
        submit(&mut s, req("outer", &["db", "gpu:2"]));

        assert_eq!(s.nesting(None, &needs(&["db"])).unwrap(), Nesting::TopLevel);
        assert_eq!(s.nesting(Some(""), &needs(&["db"])).unwrap(), Nesting::TopLevel);
        assert_eq!(
            s.nesting(Some("outer"), &needs(&["db", "gpu"])).unwrap(),
            Nesting::Inherited("outer".into())
        );

        let err = s.nesting(Some("outer"), &needs(&["db", "cache"])).unwrap_err();
        assert!(err.contains("outermost"), "{err}");
        assert!(err.contains("cache"), "names what to declare: {err}");
        // More units than the outer grant holds is also "something new".
        s.set_capacity("gpu", 4).unwrap();
        assert!(s.nesting(Some("outer"), &needs(&["gpu:3"])).is_err());

        // A grant that is gone held nothing: an ordinary request.
        assert_eq!(s.nesting(Some("long-gone"), &needs(&["db"])).unwrap(), Nesting::TopLevel);
        // Still queued is not held either.
        submit(&mut s, req("waiting", &["db"]));
        assert_eq!(s.nesting(Some("waiting"), &needs(&["cache"])).unwrap(), Nesting::TopLevel);
    }

    /// Randomised workload over a few resources: at no point is a resource
    /// over capacity or a request half-granted, and once arrivals stop every
    /// request is served (no deadlock, no starvation).
    #[test]
    fn random_workloads_never_overcommit_and_always_drain() {
        let names = ["a", "b", "c", "d"];
        for seed in 1..=40u64 {
            let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut next = |bound: u64| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng % bound
            };
            let mut s = State::default();
            s.set_capacity("d", 2).unwrap();
            let mut served = Vec::new();
            let mut submitted = 0usize;
            let check = |s: &State| {
                for (name, used) in s.in_use() {
                    assert!(used <= s.capacity(&name), "seed {seed}: {name} over capacity");
                }
                for g in &s.grants {
                    assert!(g.granted_ms.is_some());
                }
                for q in &s.queue {
                    assert!(q.granted_ms.is_none(), "seed {seed}: {} waits while marked granted", q.id);
                }
            };
            for step in 0..200 {
                // Arrivals for the first 120 steps, then only completions.
                if step < 120 && next(3) != 0 {
                    let mut want = Vec::new();
                    for name in names {
                        if next(3) == 0 {
                            want.push(name);
                        }
                    }
                    if want.is_empty() {
                        want.push(names[next(4) as usize]);
                    }
                    let mut r = req(&format!("q{submitted}"), &want);
                    r.priority = [Priority::High, Priority::Normal, Priority::Low][next(3) as usize];
                    submitted += 1;
                    served.extend(submit(&mut s, r).granted);
                    check(&s);
                }
                if !s.grants.is_empty() && next(2) == 0 {
                    let id = s.grants[next(s.grants.len() as u64) as usize].id.clone();
                    served.extend(finish(&mut s, &id).granted);
                    check(&s);
                }
            }
            // Drain: finishing holders one at a time must empty the queue.
            let mut rounds = 0;
            while let Some(g) = s.grants.first() {
                let id = g.id.clone();
                served.extend(finish(&mut s, &id).granted);
                check(&s);
                rounds += 1;
                assert!(rounds <= submitted + 1, "seed {seed}: not draining");
            }
            assert!(s.queue.is_empty(), "seed {seed}: {} request(s) stranded with nothing held", s.queue.len());
            served.sort();
            served.dedup();
            assert_eq!(served.len(), submitted, "seed {seed}: every request is served exactly once");
        }
    }

    #[test]
    fn state_round_trips_through_the_locked_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources").join("state.json");
        let mut r = req("a", &["db"]);
        r.holder = Holder::current();
        transact(&path, |s| s.enqueue(r)).unwrap().unwrap();
        let snap = snapshot_at(&path).unwrap();
        assert_eq!(snap.holders.len(), 1, "the pass in snapshot grants it");
        assert_eq!(snap.resources[0].holders, vec!["a"]);
        let json = serde_json::to_string(&snap).unwrap();
        assert_eq!(serde_json::from_str::<ResourceSnapshot>(&json).unwrap(), snap);
        assert!(json.contains("\"inUse\":1"), "camelCase on the wire: {json}");

        // A request whose process does not exist is reaped by whoever looks.
        let mut dead = req("ghost", &["db"]);
        dead.holder = Holder { pid: u32::MAX - 1, start_time: Some(1) };
        transact(&path, |s| s.enqueue(dead)).unwrap().unwrap();
        let snap = snapshot_at(&path).unwrap();
        assert!(snap.queue.is_empty());
        assert_eq!(set_capacity_at(&path, "db", 3).unwrap().resources[0].capacity, 3);
    }

    fn spec(id: &str, want: &[&str], max_wait: Option<Duration>) -> AcquireSpec {
        AcquireSpec {
            id: id.into(),
            kind: RequestKind::Foreground,
            needs: needs(want),
            priority: Priority::Normal,
            max_wait,
            command: id.into(),
            workspace: None,
            session_id: None,
        }
    }

    const FAST: Duration = Duration::from_millis(20);

    #[test]
    fn acquire_blocks_until_release_and_times_out_without_holding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let first = acquire_at(&path, spec("first", &["a"], None), Holder::current(), FAST, |_| false).unwrap();
        assert_eq!(first.id(), "first");

        // max-wait: gives up, is out of the queue, and b was never held.
        let err = acquire_at(
            &path,
            spec("impatient", &["a", "b"], Some(Duration::from_millis(150))),
            Holder::current(),
            FAST,
            |_| false,
        )
        .unwrap_err();
        match err {
            AcquireError::Timeout { wait_on, .. } => {
                assert_eq!(wait_on[0].resource, "a");
                assert_eq!(wait_on[0].by, vec!["first"]);
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
        let snap = snapshot_at(&path).unwrap();
        assert!(snap.queue.is_empty());
        assert_eq!(snap.holders.len(), 1);

        // A waiter is granted as soon as the holder releases.
        let p = path.clone();
        let waiter = std::thread::spawn(move || {
            let mut polls = 0;
            let g = acquire_at(&p, spec("second", &["a"], None), Holder::current(), FAST, |w| {
                polls += 1;
                assert_eq!(w.position, 1);
                false
            })
            .unwrap();
            (g, polls)
        });
        std::thread::sleep(Duration::from_millis(120));
        first.release();
        let (second, polls) = waiter.join().unwrap();
        assert!(polls >= 1, "it did wait");
        drop(second);
        assert!(snapshot_at(&path).unwrap().holders.is_empty(), "dropping a grant releases it");
    }

    #[test]
    fn acquire_reports_cancellation_and_abort() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let _first = acquire_at(&path, spec("first", &["a"], None), Holder::current(), FAST, |_| false).unwrap();

        let p = path.clone();
        let cancelled = std::thread::spawn(move || {
            acquire_at(&p, spec("victim", &["a"], None), Holder::current(), FAST, |_| false)
        });
        std::thread::sleep(Duration::from_millis(100));
        cancel_request_at(&path, "victim").unwrap();
        assert_eq!(cancelled.join().unwrap().unwrap_err(), AcquireError::Cancelled);

        let err = acquire_at(&path, spec("quitter", &["a"], None), Holder::current(), FAST, |_| true).unwrap_err();
        assert_eq!(err, AcquireError::Aborted);
        assert!(snapshot_at(&path).unwrap().queue.is_empty(), "an aborted request leaves the queue");

        let err = acquire_at(&path, spec("greedy", &["a:5"], None), Holder::current(), FAST, |_| false).unwrap_err();
        assert!(matches!(err, AcquireError::Rejected(_)));
    }

    #[test]
    fn rendering_names_holders_and_what_each_waiter_is_blocked_on() {
        let mut s = State::default();
        submit(&mut s, req("holder", &["a"]));
        submit(&mut s, req("head", &["a", "b"]));
        submit(&mut s, req("small", &["b"]));
        let text = render_snapshot(&s.snapshot(5_000));
        assert!(text.contains("a  1/1 in use"), "{text}");
        assert!(text.contains("b  0/1 in use"), "{text}");
        assert!(text.contains("#1  head"), "{text}");
        assert!(text.contains("a (held by holder)"), "{text}");
        assert!(text.contains("b (reserved for head"), "{text}");
        assert!(render_snapshot(&State::default().snapshot(0)).contains("no resources yet"));
    }

    #[test]
    fn only_an_idle_resource_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        set_capacity_at(&path, "typo-db", 1).unwrap();
        let mut r = req("user", &["db"]);
        r.holder = Holder::current();
        transact(&path, |s| s.enqueue(r)).unwrap().unwrap();
        let mut w = req("waiter", &["db"]);
        w.holder = Holder::current();
        transact(&path, |s| s.enqueue(w)).unwrap().unwrap();

        let err = remove_resource_at(&path, "db").unwrap_err();
        assert!(err.contains("user") && err.contains("waiter"), "{err}");
        assert!(remove_resource_at(&path, "nope").unwrap_err().contains("no resource nope"));

        let snap = remove_resource_at(&path, "typo-db").unwrap();
        let names: Vec<&str> = snap.resources.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["db"]);
    }

    /// Start `sh -c <script>` and reap it on a thread, so the moment it dies
    /// its pid stops looking alive (an unreaped zombie still answers kill 0).
    #[cfg(unix)]
    fn spawn_holder(script: &str) -> (Holder, std::thread::JoinHandle<()>) {
        let mut child = std::process::Command::new("sh").args(["-c", script]).spawn().unwrap();
        let pid = child.id();
        // Give the shell a moment to install its traps before anyone signals.
        std::thread::sleep(Duration::from_millis(150));
        let holder = Holder { pid, start_time: crate::session::process_start_time(pid) };
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });
        (holder, reaper)
    }

    #[cfg(unix)]
    fn grant_to(path: &Path, id: &str, want: &[&str], holder: Holder) {
        let mut r = req(id, want);
        r.kind = RequestKind::Foreground;
        r.holder = holder;
        transact(path, |s| {
            s.enqueue(r).unwrap();
            s.schedule(&holder_alive, 1);
        })
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn kick_terminates_a_holder_and_grants_the_next_waiter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let (holder, reaper) = spawn_holder("sleep 30");
        grant_to(&path, "victim", &["db"], holder);
        let mut next = req("next", &["db"]);
        next.holder = Holder::current();
        transact(&path, |s| s.enqueue(next)).unwrap().unwrap();
        assert!(kick_at(&path, "next", Duration::from_secs(2)).unwrap_err().contains("cancel it instead"));
        assert!(kick_at(&path, "ghost", Duration::from_secs(2)).unwrap_err().contains("no request ghost"));

        let started = Instant::now();
        let snap = kick_at(&path, "victim", Duration::from_secs(5)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "SIGTERM was enough: {:?}", started.elapsed());
        assert_eq!(ids(&snap.holders), vec!["next"], "the waiter is granted in the same call");
        reaper.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn kick_escalates_to_sigkill_for_a_holder_that_ignores_sigterm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let (holder, reaper) = spawn_holder("trap '' TERM; sleep 30; sleep 30");
        let pid = holder.pid;
        grant_to(&path, "stubborn", &["db"], holder);
        let commands = evict::children_of(pid);
        assert!(!commands.is_empty(), "the shell runs sleep as a child");

        let snap = kick_at(&path, "stubborn", Duration::from_millis(300)).unwrap();
        assert!(snap.holders.is_empty());
        reaper.join().unwrap();
        // Its command went down with it rather than being orphaned (an orphan
        // is reparented and reaped by init, so a dead one stops answering).
        std::thread::sleep(Duration::from_millis(200));
        for c in commands {
            assert_ne!(unsafe { libc::kill(c as libc::pid_t, 0) }, 0, "command {c} outlived the kick");
        }
    }

    fn kinds(path: &Path, q: &HistoryQuery) -> Vec<(ResourceEventKind, Option<String>)> {
        let mut h = history::read_history_at(path, q).events;
        h.reverse();
        h.into_iter().map(|e| (e.kind, e.request_id.or(e.resource))).collect()
    }

    #[test]
    fn history_records_every_transition() {
        use ResourceEventKind as K;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let s = |x: &str| Some(x.to_string());

        let first = acquire_at(&path, spec("first", &["a"], None), Holder::current(), FAST, |_| false).unwrap();
        let err = acquire_at(&path, spec("late", &["a"], Some(Duration::from_millis(60))), Holder::current(), FAST, |_| false);
        assert!(matches!(err, Err(AcquireError::Timeout { .. })));
        first.release_with_exit(3);

        // A queued request cancelled by hand, and a dead one reaped.
        let blocker = acquire_at(&path, spec("blocker", &["b"], None), Holder::current(), FAST, |_| false).unwrap();
        let mut waiter = req("waiter", &["b"]);
        waiter.holder = Holder::current();
        transact(&path, |st| st.enqueue(waiter)).unwrap().unwrap();
        let mut ghost = req("ghost", &["b"]);
        ghost.holder = Holder { pid: u32::MAX - 1, start_time: Some(1) };
        transact(&path, |st| st.enqueue(ghost)).unwrap().unwrap();
        cancel_request_at(&path, "waiter").unwrap();
        drop(blocker);

        set_capacity_at(&path, "a", 2).unwrap();
        remove_resource_at(&path, "a").unwrap();

        let all = kinds(&path, &HistoryQuery::default());
        assert_eq!(
            all,
            vec![
                (K::Enqueued, s("first")),
                (K::Granted, s("first")),
                (K::Enqueued, s("late")),
                (K::TimedOut, s("late")),
                (K::Released, s("first")),
                (K::Enqueued, s("blocker")),
                (K::Granted, s("blocker")),
                (K::Enqueued, s("waiter")),
                (K::Enqueued, s("ghost")),
                // One transaction: the cancel's pass reaps the ghost too.
                (K::Cancelled, s("waiter")),
                (K::Reaped, s("ghost")),
                (K::Released, s("blocker")),
                (K::CapacitySet, s("a")),
                (K::ResourceRemoved, s("a")),
            ]
        );
        let h = history::read_history_at(&path, &HistoryQuery::default());
        let released = h.events.iter().find(|e| e.kind == K::Released && e.request_id == s("first")).unwrap();
        assert_eq!(released.exit_code, Some(3));
        assert!(released.held_ms.is_some());
        let cap = h.events.iter().find(|e| e.kind == K::CapacitySet).unwrap();
        assert_eq!((cap.prev_capacity, cap.capacity), (Some(1), Some(2)));

        // Filters and paging.
        let only_b = kinds(&path, &HistoryQuery { resource: s("b"), ..Default::default() });
        assert_eq!(only_b.len(), 7);
        let page = history::read_history_at(&path, &HistoryQuery { limit: Some(2), ..Default::default() });
        assert_eq!(page.events.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.events[0].kind, K::ResourceRemoved, "newest first");
        let one = kinds(&path, &HistoryQuery { request_id: s("late"), ..Default::default() });
        assert_eq!(one, vec![(K::Enqueued, s("late")), (K::TimedOut, s("late"))]);
        assert!(render_history(&h).contains("timed_out"));
    }

    #[test]
    fn a_kicked_grant_is_recorded_as_kicked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let g = acquire_at(&path, spec("k", &["a"], None), Holder::current(), FAST, |_| false).unwrap();
        transact(&path, |s| s.kicking.insert("k".into())).unwrap();
        g.release_with_exit(143);
        let h = history::read_history_at(&path, &HistoryQuery::default());
        assert_eq!(h.events[0].kind, ResourceEventKind::Kicked);
        assert_eq!(h.events[0].exit_code, Some(143));
        let st = load(&path).unwrap();
        assert!(st.kicking.is_empty(), "pruned with the grant");
    }

    #[test]
    fn history_rotates_and_reads_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let live = history::history_path_for(&path);
        std::fs::write(&live, vec![b' '; history::ROTATE_BYTES as usize]).unwrap();
        let g = acquire_at(&path, spec("r", &["a"], None), Holder::current(), FAST, |_| false).unwrap();
        drop(g);
        assert!(std::fs::metadata(&live).unwrap().len() < 4096, "rotated");
        assert!(dir.path().join("history.1.jsonl").exists());
        assert_eq!(history::read_history_at(&path, &HistoryQuery::default()).events.len(), 3);
    }

    #[test]
    fn an_unwritable_history_never_fails_scheduling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // A directory where the log file should be: every append fails.
        std::fs::create_dir_all(history::history_path_for(&path)).unwrap();
        let g = acquire_at(&path, spec("x", &["a"], None), Holder::current(), FAST, |_| false).unwrap();
        drop(g);
        assert!(snapshot_at(&path).unwrap().holders.is_empty());
    }
}
