//! Resource queue history — an append-only event log next to the state file
//! (`~/.fleet/resources/history.jsonl`).
//!
//! The state file only knows the present: once a grant is released nothing of
//! it is left. Every transaction that changes the table diffs the state before
//! and after ([`events_between`]) and appends what happened — enqueue, grant,
//! release, kick, cancel, timeout, reap, capacity change, resource removal.
//!
//! The diff runs under the state lock (it is a few comparisons); the file
//! write happens after the lock is dropped, under its own short lock, and its
//! failure is only logged: history must never hold up or fail scheduling.
//!
//! Rotation: when `history.jsonl` grows past [`ROTATE_BYTES`] it is renamed to
//! `history.1.jsonl` (replacing the previous one), so at most about twice that
//! is ever kept. Readers read both.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Priority, RequestKind, ResourceRequest, State};

/// Size at which the live log is rotated.
pub const ROTATE_BYTES: u64 = 2 * 1024 * 1024;
/// Default and maximum number of events one [`read_history`] call returns.
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 2000;

/// What happened. A reader skips lines it cannot parse, so an older build reading a newer
/// log drops those lines instead of failing.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ResourceEventKind {
    /// A request joined the queue.
    Enqueued,
    /// Every resource of a request was granted.
    Granted,
    /// A holder let go of its grant (its command ended).
    Released,
    /// A holder was evicted by hand (`fleet resource kick`), or a session
    /// lease was released on its session's behalf by someone else.
    Kicked,
    /// A queued request was cancelled before it ran.
    Cancelled,
    /// A queued request gave up at `--max-wait`.
    TimedOut,
    /// A queued request's own process stopped waiting (interrupted, or the
    /// queue could not be read).
    Aborted,
    /// A request was dropped because its process is gone — for a session
    /// lease, because its session has ended.
    Reaped,
    /// A session lease moved to the session that took over its owner's work
    /// (a handoff); `from_session_id` is the previous owner.
    Transferred,
    /// A resource's capacity was set.
    CapacitySet,
    /// A resource was deleted from the table.
    ResourceRemoved,
}

/// One line of the log. Request fields are set on request events; `resource`
/// / `capacity` on resource events.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResourceEvent {
    pub ts_ms: u64,
    pub kind: ResourceEventKind,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_kind: Option<RequestKind>,
    /// Resource name → units the request asked for.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub needs: BTreeMap<String, u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub priority: Option<Priority>,
    /// The holder process.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace: Option<String>,
    /// A session lease's description.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
    /// On `transferred`: the session the lease moved away from.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from_session_id: Option<String>,
    /// Time spent queued: on `granted`, and on a queued request that left.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub waited_ms: Option<u64>,
    /// Time the grant was held: on `released` / `kicked` / `reaped` of a grant.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub held_ms: Option<u64>,
    /// The command's exit code, when its holder reported one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<i32>,
    /// The resource of a `capacity_set` / `resource_removed` event.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capacity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prev_capacity: Option<u32>,
    /// On a lease piece released in part: what it still holds (or, for a
    /// queued piece, still waits for). `needs` is what was let go of.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub kept: BTreeMap<String, u32>,
}

impl ResourceEvent {
    fn bare(ts_ms: u64, kind: ResourceEventKind) -> Self {
        Self {
            ts_ms,
            kind,
            request_id: None,
            request_kind: None,
            needs: BTreeMap::new(),
            priority: None,
            pid: None,
            session_id: None,
            command: None,
            workspace: None,
            note: None,
            from_session_id: None,
            waited_ms: None,
            held_ms: None,
            exit_code: None,
            resource: None,
            capacity: None,
            prev_capacity: None,
            kept: BTreeMap::new(),
        }
    }

    fn of(ts_ms: u64, kind: ResourceEventKind, r: &ResourceRequest) -> Self {
        Self {
            request_id: Some(r.id.clone()),
            request_kind: Some(r.kind),
            needs: r.needs.clone(),
            priority: Some(r.priority),
            // A lease has no holder process.
            pid: (!r.is_lease()).then_some(r.holder.pid),
            session_id: r.session_id.clone(),
            command: (!r.command.is_empty()).then(|| r.command.clone()),
            workspace: r.workspace.clone(),
            note: r.note.clone(),
            ..Self::bare(ts_ms, kind)
        }
    }

    /// Whether this event concerns resource `name`.
    pub fn touches(&self, name: &str) -> bool {
        self.resource.as_deref() == Some(name) || self.needs.contains_key(name)
    }
}

/// One page of history, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResourceHistory {
    pub events: Vec<ResourceEvent>,
    /// More, older events match: ask again with `before_ms` = the last
    /// event's `ts_ms`.
    pub has_more: bool,
}

/// Filter for [`read_history`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    /// Only events touching this resource.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resource: Option<String>,
    /// Only events of this request.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_id: Option<String>,
    /// Only events strictly older than this (paging cursor).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub before_ms: Option<u64>,
    /// At most this many (default [`DEFAULT_LIMIT`], capped at [`MAX_LIMIT`]).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub limit: Option<usize>,
}

/// Why a transaction changed the table, for the requests it removes. The
/// diff alone cannot tell a release from a cancel from a timeout; a removal
/// it has no hint for is classified by where the request was and whether
/// its holder is alive.
#[derive(Debug, Clone, Default)]
pub(super) enum Cause {
    #[default]
    None,
    Released { id: String, exit_code: Option<i32> },
    /// Session lease pieces left the table or shrank: released (held) or
    /// unqueued (queued); `by_other` — on its session's behalf, logged as
    /// kicked.
    LeaseLeft { ids: Vec<String>, by_other: bool },
    Cancelled(String),
    TimedOut(String),
    Aborted(String),
    SetCapacity(String),
}

/// What changed between `before` and `after`, in the order it happened:
/// requests leaving, leases changing hands, requests arriving, grants, then
/// resource changes. `alive` says whether whatever stands behind a request
/// (its process; a lease's session) is still there.
pub(super) fn events_between(
    before: &State,
    after: &State,
    cause: &Cause,
    now: u64,
    alive: &dyn Fn(&ResourceRequest) -> bool,
) -> Vec<ResourceEvent> {
    use ResourceEventKind as K;
    let find = |s: &State, id: &str| -> Option<(bool, ResourceRequest)> {
        s.grants
            .iter()
            .find(|r| r.id == id)
            .map(|r| (true, r.clone()))
            .or_else(|| s.queue.iter().find(|r| r.id == id).map(|r| (false, r.clone())))
    };
    let mut out = Vec::new();

    // Left the table.
    for (was_granted, r) in before
        .grants
        .iter()
        .map(|r| (true, r))
        .chain(before.queue.iter().map(|r| (false, r)))
    {
        if find(after, &r.id).is_some() {
            continue;
        }
        let hinted = |id: &str| id == r.id;
        let kind = if was_granted && before.kicking.contains(&r.id) {
            K::Kicked
        } else {
            match cause {
                Cause::Released { id, .. } if hinted(id) => K::Released,
                Cause::LeaseLeft { ids, by_other } if ids.iter().any(|id| hinted(id)) => match (was_granted, by_other) {
                    (false, _) => K::Cancelled,
                    (true, true) => K::Kicked,
                    (true, false) => K::Released,
                },
                Cause::Cancelled(id) if hinted(id) => K::Cancelled,
                Cause::TimedOut(id) if hinted(id) => K::TimedOut,
                Cause::Aborted(id) if hinted(id) => K::Aborted,
                _ if !alive(r) => K::Reaped,
                _ if was_granted => K::Released,
                _ => K::Cancelled,
            }
        };
        let mut e = ResourceEvent::of(now, kind, r);
        if let Cause::Released { id, exit_code } = cause {
            if hinted(id) {
                e.exit_code = *exit_code;
            }
        }
        if was_granted {
            e.held_ms = r.granted_ms.map(|g| now.saturating_sub(g));
        } else {
            e.waited_ms = Some(now.saturating_sub(r.enqueued_ms));
        }
        out.push(e);
    }

    // Lease pieces released in part: shrank, but stay.
    if let Cause::LeaseLeft { ids, by_other } = cause {
        for r in after.grants.iter().chain(&after.queue) {
            if !ids.contains(&r.id) {
                continue;
            }
            let Some((was_granted, prev)) = find(before, &r.id) else { continue };
            if prev.needs == r.needs {
                continue;
            }
            let gone: BTreeMap<String, u32> = prev
                .needs
                .iter()
                .filter_map(|(name, &n)| {
                    let k = n.saturating_sub(r.needs.get(name).copied().unwrap_or(0));
                    (k > 0).then(|| (name.clone(), k))
                })
                .collect();
            if gone.is_empty() {
                continue;
            }
            let kind = match (was_granted, by_other) {
                (false, _) => K::Cancelled,
                (true, true) => K::Kicked,
                (true, false) => K::Released,
            };
            let mut e = ResourceEvent::of(now, kind, r);
            e.needs = gone;
            e.kept = r.needs.clone();
            if was_granted {
                e.held_ms = prev.granted_ms.map(|g| now.saturating_sub(g));
            } else {
                e.waited_ms = Some(now.saturating_sub(prev.enqueued_ms));
            }
            out.push(e);
        }
    }

    // Moved to another session.
    for r in after.grants.iter().chain(&after.queue) {
        let Some((_, prev)) = find(before, &r.id) else { continue };
        if r.is_lease() && prev.session_id != r.session_id {
            let mut e = ResourceEvent::of(now, K::Transferred, r);
            e.from_session_id = prev.session_id.clone();
            out.push(e);
        }
    }

    // Arrived, and granted.
    let arrived = |r: &ResourceRequest| find(before, &r.id).is_none();
    for r in after.queue.iter().chain(&after.grants) {
        if arrived(r) {
            out.push(ResourceEvent::of(now, K::Enqueued, r));
        }
    }
    for r in &after.grants {
        if matches!(find(before, &r.id), Some((true, _))) {
            continue;
        }
        let at = r.granted_ms.unwrap_or(now);
        // Stamped with the transaction's clock, read after the pass's own:
        // a grant must not sort ahead of the release or reap that freed it.
        let mut e = ResourceEvent::of(at.max(now), K::Granted, r);
        e.waited_ms = Some(at.saturating_sub(r.enqueued_ms));
        out.push(e);
    }

    // Resources.
    for (name, def) in &after.resources {
        let prev = before.resources.get(name).map(|d| d.capacity);
        let explicit = matches!(cause, Cause::SetCapacity(n) if n == name);
        if prev.is_some_and(|p| p != def.capacity) || (prev.is_none() && explicit) {
            let mut e = ResourceEvent::bare(now, K::CapacitySet);
            e.resource = Some(name.clone());
            e.capacity = Some(def.capacity);
            e.prev_capacity = prev;
            out.push(e);
        }
    }
    for (name, def) in &before.resources {
        if !after.resources.contains_key(name) {
            let mut e = ResourceEvent::bare(now, K::ResourceRemoved);
            e.resource = Some(name.clone());
            e.prev_capacity = Some(def.capacity);
            out.push(e);
        }
    }
    out
}

/// `history.jsonl` next to the state file.
pub fn history_path_for(state_path: &Path) -> PathBuf {
    state_path.with_file_name("history.jsonl")
}

fn rotated_path(live: &Path) -> PathBuf {
    live.with_file_name("history.1.jsonl")
}

/// Append `events`, rotating first if the log is too big. Best effort: a
/// failure is logged and dropped.
pub(super) fn append(state_path: &Path, events: &[ResourceEvent]) {
    if events.is_empty() {
        return;
    }
    if let Err(e) = append_inner(&history_path_for(state_path), events) {
        crate::log_debug(&format!("resource_queue: history append failed: {e}"));
    }
}

fn append_inner(live: &Path, events: &[ResourceEvent]) -> Result<(), String> {
    let mut buf = Vec::new();
    for e in events {
        serde_json::to_writer(&mut buf, e).map_err(|e| e.to_string())?;
        buf.push(b'\n');
    }
    let mut lock_name = live.as_os_str().to_owned();
    lock_name.push(".lock");
    // The lock serializes writers (one `write` per batch, never interleaved)
    // and makes rotate-then-append atomic against other appenders.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(lock_name))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    if std::fs::metadata(live).is_ok_and(|m| m.len() >= ROTATE_BYTES) {
        std::fs::rename(live, rotated_path(live)).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(live)
        .map_err(|e| e.to_string())?;
    f.write_all(&buf).map_err(|e| e.to_string())?;
    drop(lock);
    Ok(())
}

fn read_lines(path: &Path, out: &mut Vec<ResourceEvent>) {
    let Ok(f) = std::fs::File::open(path) else { return };
    for line in std::io::BufReader::new(f).lines() {
        let Ok(line) = line else { break };
        if let Ok(e) = serde_json::from_str::<ResourceEvent>(&line) {
            out.push(e);
        }
    }
}

/// Events matching `q`, newest first.
pub fn read_history_at(state_path: &Path, q: &HistoryQuery) -> ResourceHistory {
    let live = history_path_for(state_path);
    let mut all = Vec::new();
    read_lines(&rotated_path(&live), &mut all);
    read_lines(&live, &mut all);
    // Appenders in different processes may land slightly out of order.
    all.sort_by_key(|e| e.ts_ms);
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut matching = all.into_iter().rev().filter(|e| {
        q.before_ms.is_none_or(|b| e.ts_ms < b)
            && q.resource.as_deref().is_none_or(|n| e.touches(n))
            && q.request_id.as_deref().is_none_or(|id| e.request_id.as_deref() == Some(id))
    });
    let events: Vec<ResourceEvent> = matching.by_ref().take(limit).collect();
    let has_more = matching.next().is_some();
    ResourceHistory { events, has_more }
}

/// Events matching `q` from this machine's log, newest first.
pub fn read_history(q: &HistoryQuery) -> Result<ResourceHistory, String> {
    Ok(read_history_at(&super::state_path()?, q))
}

/// Plain-text listing for `fleet resource history`, oldest first.
pub fn render_history(h: &ResourceHistory) -> String {
    use super::{fmt_duration, format_needs};
    if h.events.is_empty() {
        return "no resource history yet\n".into();
    }
    let mut out = String::new();
    for e in h.events.iter().rev() {
        let when = chrono::DateTime::from_timestamp_millis(e.ts_ms as i64)
            .map(|t| t.with_timezone(&chrono::Local).format("%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        let kind = serde_json::to_value(e.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let mut line = format!("{when}  {kind:<16}");
        if let Some(id) = &e.request_id {
            line.push_str(&format!(" {id}  [{}]", format_needs(&e.needs)));
            if !e.kept.is_empty() {
                line.push_str(&format!("  kept [{}]", format_needs(&e.kept)));
            }
        }
        if let Some(name) = &e.resource {
            line.push_str(&format!(" {name}"));
            match (e.prev_capacity, e.capacity) {
                (Some(p), Some(c)) => line.push_str(&format!("  capacity {p} → {c}")),
                (None, Some(c)) => line.push_str(&format!("  capacity {c}")),
                _ => {}
            }
        }
        if let Some(w) = e.waited_ms {
            line.push_str(&format!("  waited {}", fmt_duration(w)));
        }
        if let Some(h) = e.held_ms {
            line.push_str(&format!("  held {}", fmt_duration(h)));
        }
        if let Some(c) = e.exit_code {
            line.push_str(&format!("  exit {c}"));
        }
        if let Some(pid) = e.pid {
            line.push_str(&format!("  pid {pid}"));
        }
        if let Some(sid) = &e.session_id {
            let short = |s: &str| s.chars().take(8).collect::<String>();
            match &e.from_session_id {
                Some(from) => line.push_str(&format!("  session {} → {}", short(from), short(sid))),
                None => line.push_str(&format!("  session {}", short(sid))),
            }
        }
        if e.request_kind == Some(RequestKind::Lease) {
            line.push_str("  lease");
            if let Some(n) = &e.note {
                line.push_str(&format!(" · {n}"));
            }
        }
        if let Some(cmd) = &e.command {
            let one: String = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
            let short: String = one.chars().take(60).collect();
            line.push_str(&format!("  {short}{}", if one.chars().count() > 60 { "…" } else { "" }));
        }
        out.push_str(&line);
        out.push('\n');
    }
    if h.has_more {
        out.push_str("(older events not shown — raise --limit or page with --before)\n");
    }
    out
}
