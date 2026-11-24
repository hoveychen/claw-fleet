//! Deadlock avoidance for requests that wait while their session holds
//! something.
//!
//! A command never holds and waits: it is granted everything at once and a
//! nested request for more is refused. A session lease can: a session holding
//! some resources may queue for more (an append), and the commands it runs
//! queue like anyone else's. Those waits are allowed as long as they cannot
//! close a cycle, judged on a **conservative wait-for graph** over every
//! request in the table:
//!
//! * a queued request waits for every granted lease holding one of its
//!   resources, and for every queued request ahead of it that needs one of
//!   them — whatever its `--after`, its spare skips or the resource's spare
//!   capacity;
//! * a granted lease waits for every queued request of its session: the
//!   session does not let go while it waits for them. A queued lease does too,
//!   for the time it is granted;
//! * a granted command waits for nothing — it ends by itself.
//!
//! A new wait — an enqueue, a request whose needs changed, a manual move, a
//! lease piece taken on top of a session's waits — is refused when the request
//! it touches lies on a cycle of that graph ([`cycle_through`]). Every edge it
//! adds touches that request, so the graph stays acyclic.
//!
//! Grants made by the scheduling pass never add an edge that can close a
//! cycle: a lease whose session waits for anything else is never granted past
//! a request ahead of it (`State::plan`), so whoever starts waiting on a new
//! grant was already waiting on it as a queued request ahead — the edge was
//! there. A command or a lease whose session waits for nothing else has no
//! outgoing edge once granted, so nothing can lead back through it.
//!
//! The graph over-approximates real waits: it may refuse a wait that would
//! have worked out (a reservation that was going to be skipped, a resource
//! with room for both). It never lets a cycle in.

use std::collections::{BTreeMap, VecDeque};

use super::{format_needs, RequestKind, ResourceRequest, State};

/// Why one request waits for the next on a cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Why {
    /// The next one is a lease holding this resource.
    HeldBy(String),
    /// The next one is queued ahead and needs this resource too.
    Behind(String),
    /// The next one is a queued request of the same session, which keeps
    /// this lease (granted, or once granted) while it waits.
    Session,
}

/// One step of a cycle: the request, and why it waits for the next one (the
/// last one waits for the first).
pub(super) type Step = (ResourceRequest, Why);

/// Requests in the table: grants first, then the queue in order.
fn nodes(state: &State) -> Vec<&ResourceRequest> {
    state.grants.iter().chain(&state.queue).collect()
}

/// The conservative wait-for graph (see the module docs): node index → its
/// outgoing edges `(to, why)`, at most one per target.
fn edges(state: &State) -> Vec<Vec<(usize, Why)>> {
    let granted = state.grants.len();
    let all = nodes(state);
    let same_session = |a: &ResourceRequest, b: &ResourceRequest| {
        a.session_id.is_some() && a.session_id == b.session_id
    };
    let mut out: Vec<Vec<(usize, Why)>> = vec![Vec::new(); all.len()];
    let push = |from: usize, to: usize, why: Why, out: &mut Vec<Vec<(usize, Why)>>| {
        if from != to && !out[from].iter().any(|(t, _)| *t == to) {
            out[from].push((to, why));
        }
    };
    for (gi, g) in state.grants.iter().enumerate() {
        if !g.is_lease() {
            continue;
        }
        for (qi, q) in state.queue.iter().enumerate() {
            if same_session(g, q) {
                push(gi, granted + qi, Why::Session, &mut out);
            }
        }
    }
    for (qi, q) in state.queue.iter().enumerate() {
        let from = granted + qi;
        for name in q.needs.keys() {
            for (gi, g) in state.grants.iter().enumerate() {
                if g.is_lease() && g.needs.contains_key(name) {
                    push(from, gi, Why::HeldBy(name.clone()), &mut out);
                }
            }
            for (ai, ahead) in state.queue[..qi].iter().enumerate() {
                if ahead.needs.contains_key(name) {
                    push(from, granted + ai, Why::Behind(name.clone()), &mut out);
                }
            }
        }
        if q.is_lease() {
            for (oi, other) in state.queue.iter().enumerate() {
                if oi != qi && same_session(q, other) {
                    push(from, granted + oi, Why::Session, &mut out);
                }
            }
        }
    }
    out
}

/// The shortest cycle through request `id` in the wait-for graph, if any.
pub(super) fn cycle_through(state: &State, id: &str) -> Option<Vec<Step>> {
    let all = nodes(state);
    let start = all.iter().position(|r| r.id == id)?;
    let edges = edges(state);
    // Breadth-first from `start`; `prev[n]` is how n was reached.
    let mut prev: BTreeMap<usize, (usize, Why)> = BTreeMap::new();
    let mut todo = VecDeque::from([start]);
    while let Some(n) = todo.pop_front() {
        for (to, why) in &edges[n] {
            if *to == start {
                // Walk back from n to start, then close the loop.
                let mut path = vec![(n, why.clone())];
                let mut at = n;
                while at != start {
                    let (p, w) = prev[&at].clone();
                    path.push((p, w));
                    at = p;
                }
                path.reverse();
                return Some(path.into_iter().map(|(i, w)| (all[i].clone(), w)).collect());
            }
            if !prev.contains_key(to) && *to != start {
                prev.insert(*to, (n, why.clone()));
                todo.push_back(*to);
            }
        }
    }
    None
}

/// `lease l123 [board] (session abcd1234)`, `job j9 [cargo]`.
fn label(r: &ResourceRequest) -> String {
    let kind = match r.kind {
        RequestKind::Lease => "lease",
        RequestKind::Job => "job",
        RequestKind::Foreground => "command",
    };
    let mut s = format!("{kind} {} [{}]", r.id, format_needs(&r.needs));
    if let Some(sid) = &r.session_id {
        s.push_str(&format!(" (session {})", sid.chars().take(8).collect::<String>()));
    }
    s
}

/// The cycle in words: "A waits for board held by B, which waits for … A".
pub(super) fn explain(cycle: &[Step]) -> String {
    let mut out = String::new();
    for (i, (r, why)) in cycle.iter().enumerate() {
        let next = &cycle[(i + 1) % cycle.len()].0;
        let link = match why {
            Why::HeldBy(name) => format!("waits for {name} held by"),
            Why::Behind(name) => format!("waits for {name} behind"),
            Why::Session if r.granted_ms.is_some() => {
                "is kept by a session that waits for".to_string()
            }
            Why::Session => "would be kept, once granted, by a session that waits for".to_string(),
        };
        if i == 0 {
            out.push_str(&label(r));
        }
        out.push_str(&format!(" {link} {}", label(next)));
        if i + 1 < cycle.len() {
            out.push(',');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{Holder, Priority};
    use super::*;

    fn req(id: &str, kind: RequestKind, session: Option<&str>, need: &[&str]) -> ResourceRequest {
        ResourceRequest {
            id: id.into(),
            kind,
            needs: need.iter().map(|n| (n.to_string(), 1)).collect(),
            priority: Priority::Normal,
            enqueued_ms: 0,
            granted_ms: None,
            manual: false,
            holder: Holder { pid: 1, start_time: Some(1) },
            command: String::new(),
            workspace: None,
            session_id: session.map(str::to_string),
            after_ms: None,
            note: None,
            skips: 0,
            idle_since_ms: None,
        }
    }

    fn lease(id: &str, session: &str, need: &[&str]) -> ResourceRequest {
        req(id, RequestKind::Lease, Some(session), need)
    }

    fn granted(mut r: ResourceRequest) -> ResourceRequest {
        r.granted_ms = Some(1);
        r
    }

    fn ids(cycle: &[Step]) -> Vec<&str> {
        cycle.iter().map(|(r, _)| r.id.as_str()).collect()
    }

    #[test]
    fn two_sessions_each_holding_what_the_other_appends_for_is_a_cycle() {
        let state = State {
            grants: vec![granted(lease("la", "a", &["round"])), granted(lease("lb", "b", &["wave"]))],
            queue: vec![lease("qa", "a", &["wave"]), lease("qb", "b", &["round"])],
            ..Default::default()
        };
        let c = cycle_through(&state, "qb").expect("cycle");
        assert_eq!(ids(&c), vec!["qb", "la", "qa", "lb"]);
        assert_eq!(c[0].1, Why::HeldBy("round".into()));
        assert_eq!(c[1].1, Why::Session);
        let text = explain(&c);
        assert!(text.starts_with("lease qb [round] (session b) waits for round held by lease la"), "{text}");
        assert!(text.ends_with("waits for lease qb [round] (session b)"), "{text}");
    }

    #[test]
    fn a_holder_waiting_for_a_resource_held_by_a_plain_holder_is_no_cycle() {
        let state = State {
            grants: vec![granted(lease("la", "a", &["round"])), granted(lease("lb", "b", &["wave"]))],
            queue: vec![lease("qa", "a", &["wave"])],
            ..Default::default()
        };
        assert!(cycle_through(&state, "qa").is_none());
    }

    #[test]
    fn a_reservation_ahead_counts_as_a_wait() {
        // c waits (holding nothing) for round, which a holds; a appends for
        // wave, which c is ahead for: a would wait on c, c on a.
        let state = State {
            grants: vec![granted(lease("la", "a", &["round"]))],
            queue: vec![lease("qc", "c", &["round", "wave"]), lease("qa", "a", &["wave"])],
            ..Default::default()
        };
        assert_eq!(ids(&cycle_through(&state, "qa").unwrap()), vec!["qa", "qc", "la"]);
    }

    #[test]
    fn commands_close_a_cycle_through_their_sessions_leases() {
        // a's job waits for wave held by b's lease; b's job for round held by
        // a's lease. Each session waits for its job.
        let state = State {
            grants: vec![granted(lease("la", "a", &["round"])), granted(lease("lb", "b", &["wave"]))],
            queue: vec![
                req("ja", RequestKind::Job, Some("a"), &["wave"]),
                req("jb", RequestKind::Job, Some("b"), &["round"]),
            ],
            ..Default::default()
        };
        assert_eq!(ids(&cycle_through(&state, "jb").unwrap()), vec!["jb", "la", "ja", "lb"]);
    }

    #[test]
    fn a_granted_command_waits_for_nothing() {
        // The holder of cargo is a running command: it ends by itself.
        let state = State {
            grants: vec![
                granted(req("build", RequestKind::Job, Some("b"), &["cargo"])),
                granted(lease("la", "a", &["round"])),
            ],
            queue: vec![
                req("ja", RequestKind::Job, Some("a"), &["cargo"]),
                req("jb", RequestKind::Job, Some("b"), &["round"]),
            ],
            ..Default::default()
        };
        assert!(cycle_through(&state, "ja").is_none());
        assert!(cycle_through(&state, "jb").is_none());
    }

    #[test]
    fn a_session_waiting_on_its_own_queued_lease_is_a_cycle() {
        // The session queued a lease for wave, then a job for wave behind it:
        // once the lease is granted the job waits for its own session.
        let state = State {
            queue: vec![lease("qa", "a", &["wave"]), req("ja", RequestKind::Job, Some("a"), &["wave"])],
            ..Default::default()
        };
        assert_eq!(ids(&cycle_through(&state, "ja").unwrap()), vec!["ja", "qa"]);
    }

    #[test]
    fn plain_queues_of_commands_never_cycle() {
        let state = State {
            grants: vec![granted(req("r1", RequestKind::Foreground, None, &["cargo"]))],
            queue: vec![
                req("r2", RequestKind::Foreground, None, &["cargo"]),
                req("r3", RequestKind::Job, Some("x"), &["cargo", "db"]),
                req("r4", RequestKind::Job, Some("x"), &["db"]),
            ],
            ..Default::default()
        };
        for id in ["r1", "r2", "r3", "r4"] {
            assert!(cycle_through(&state, id).is_none(), "{id}");
        }
    }
}
