//! Row-level diffs of the session list, so a push channel can ship the rows
//! that changed instead of the whole list.
//!
//! The list is rebuilt by a full source rescan on every change and used to be
//! pushed whole each time: ~1.7MB of JSON for ~1,150 rows on a busy box
//! (measured 2026-10-03), when a typical tick touches one or two rows.
//! [`DeltaTracker`] keeps a per-id content hash of the last list it framed and
//! turns the next list into a [`SessionsFrame::Delta`] against it.
//!
//! Frames are sequence-numbered so a consumer can tell when it missed one: a
//! delta applies only on top of the state named by its `base_seq`. A consumer
//! whose own seq differs must refetch a full frame
//! ([`DeltaTracker::snapshot`]) rather than apply it.
//!
//! The mobile relay diffs its slimmed snapshot with the same helpers
//! ([`per_id_hashes`], [`diff_snapshot`], [`snapshot_hash`]).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One push of the session list: the whole list, or the rows that changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionsFrame {
    /// The whole list, in display order. Replaces whatever the consumer held.
    #[serde(rename_all = "camelCase")]
    Full { seq: u64, sessions: Vec<Value> },
    /// Changes on top of the state the frame numbered `base_seq` produced.
    #[serde(rename_all = "camelCase")]
    Delta {
        seq: u64,
        base_seq: u64,
        /// New rows and rows whose content changed, whole.
        upsert: Vec<Value>,
        /// Ids no longer in the list.
        remove: Vec<String>,
        /// The full id order, present only when it differs from the previous
        /// frame's. Absent means: keep the existing rows where they are and
        /// append new ones at the end.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<Vec<String>>,
    },
}

impl SessionsFrame {
    pub fn seq(&self) -> u64 {
        match self {
            SessionsFrame::Full { seq, .. } | SessionsFrame::Delta { seq, .. } => *seq,
        }
    }
}

/// Turns successive session lists into frames. Not thread-safe on its own;
/// callers that emit from several threads hold it behind one mutex and emit
/// while holding it, so frames leave in seq order.
#[derive(Debug, Default)]
pub struct DeltaTracker {
    seq: u64,
    /// id → content hash of the last list framed. `None` until the first frame,
    /// and after [`reset`](Self::reset): the next frame is then a full one.
    baseline: Option<HashMap<String, u64>>,
    order: Vec<String>,
    /// The last list framed, kept so [`snapshot`](Self::snapshot) can answer a
    /// resync without a rescan.
    last: Vec<Value>,
}

impl DeltaTracker {
    /// `const` so a process-wide tracker can live in a `static Mutex`.
    pub const fn new() -> Self {
        Self {
            seq: 0,
            baseline: None,
            order: Vec::new(),
            last: Vec::new(),
        }
    }

    /// Frame `sessions` (a JSON array of session objects, in display order).
    /// `None` when nothing changed since the last frame — no seq is consumed.
    pub fn frame(&mut self, sessions: &[Value]) -> Option<SessionsFrame> {
        let order = ids_of(sessions);
        let Some(baseline) = self.baseline.as_ref() else {
            return Some(self.seed(sessions, order));
        };
        let (upsert, remove) = diff_rows(baseline, sessions);
        let order_changed = order != self.order;
        if upsert.is_empty() && remove.is_empty() && !order_changed {
            return None;
        }
        let base_seq = self.seq;
        self.seq += 1;
        self.baseline = Some(hashes_of(sessions));
        self.last = sessions.to_vec();
        self.order = order.clone();
        Some(SessionsFrame::Delta {
            seq: self.seq,
            base_seq,
            upsert,
            remove,
            order: order_changed.then_some(order),
        })
    }

    /// A full frame of the last list framed, under the current seq. What a
    /// consumer that fell out of step fetches; consumes no seq, so deltas
    /// already in flight still apply on top of it.
    pub fn snapshot(&self) -> SessionsFrame {
        SessionsFrame::Full {
            seq: self.seq,
            sessions: self.last.clone(),
        }
    }

    /// Make the next [`frame`](Self::frame) a full one.
    pub fn reset(&mut self) {
        self.baseline = None;
    }

    fn seed(&mut self, sessions: &[Value], order: Vec<String>) -> SessionsFrame {
        self.seq += 1;
        self.baseline = Some(hashes_of(sessions));
        self.last = sessions.to_vec();
        self.order = order;
        SessionsFrame::Full {
            seq: self.seq,
            sessions: sessions.to_vec(),
        }
    }
}

fn ids_of(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .filter_map(|s| s.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn hashes_of(rows: &[Value]) -> HashMap<String, u64> {
    rows.iter()
        .filter_map(|s| Some((s.get("id")?.as_str()?.to_string(), snapshot_hash(s))))
        .collect()
}

fn diff_rows(prev: &HashMap<String, u64>, rows: &[Value]) -> (Vec<Value>, Vec<String>) {
    let mut upsert = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for s in rows {
        let Some(id) = s.get("id").and_then(Value::as_str) else {
            continue;
        };
        seen.insert(id);
        if prev.get(id) != Some(&snapshot_hash(s)) {
            upsert.push(s.clone());
        }
    }
    let remove = prev
        .keys()
        .filter(|id| !seen.contains(id.as_str()))
        .cloned()
        .collect();
    (upsert, remove)
}

/// Per-`id` hash of each session object in a JSON array. Sessions without a
/// string `id` are skipped (they can't be keyed).
pub(crate) fn per_id_hashes(list: &Value) -> HashMap<String, u64> {
    list.as_array().map(|a| hashes_of(a)).unwrap_or_default()
}

/// Diff a JSON array of sessions against a baseline, keyed by `id`. Returns
/// `(upsert, remove)`: every new or changed row whole, and every baseline id
/// the list no longer has. `upsert` keeps the list's order.
pub(crate) fn diff_snapshot(prev: &HashMap<String, u64>, list: &Value) -> (Vec<Value>, Vec<String>) {
    match list.as_array() {
        Some(a) => diff_rows(prev, a),
        None => (Vec::new(), prev.keys().cloned().collect()),
    }
}

/// Content hash of one JSON value. Never 0, which callers use as "nothing sent
/// yet".
pub(crate) fn snapshot_hash(v: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish() | 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(v: Value) -> Vec<Value> {
        v.as_array().unwrap().clone()
    }

    #[test]
    fn first_frame_is_full_and_numbered_one() {
        let mut t = DeltaTracker::new();
        let list = rows(json!([{"id": "a"}, {"id": "b"}]));
        let f = t.frame(&list).unwrap();
        assert_eq!(f, SessionsFrame::Full { seq: 1, sessions: list });
    }

    #[test]
    fn unchanged_list_yields_no_frame_and_keeps_seq() {
        let mut t = DeltaTracker::new();
        let list = rows(json!([{"id": "a", "x": 1}]));
        t.frame(&list);
        assert!(t.frame(&list).is_none());
        assert_eq!(t.snapshot().seq(), 1);
    }

    #[test]
    fn delta_carries_changed_new_and_removed_rows_without_order_when_order_kept() {
        let mut t = DeltaTracker::new();
        t.frame(&rows(json!([{"id": "a", "x": 1}, {"id": "b", "x": 1}, {"id": "c"}])));
        let f = t
            .frame(&rows(json!([{"id": "a", "x": 1}, {"id": "b", "x": 2}])))
            .unwrap();
        // Dropping the tail row changes the id order, so `order` ships.
        assert_eq!(
            f,
            SessionsFrame::Delta {
                seq: 2,
                base_seq: 1,
                upsert: vec![json!({"id": "b", "x": 2})],
                remove: vec!["c".into()],
                order: Some(vec!["a".into(), "b".into()]),
            }
        );
        let f = t
            .frame(&rows(json!([{"id": "a", "x": 9}, {"id": "b", "x": 2}])))
            .unwrap();
        assert_eq!(
            f,
            SessionsFrame::Delta {
                seq: 3,
                base_seq: 2,
                upsert: vec![json!({"id": "a", "x": 9})],
                remove: vec![],
                order: None,
            }
        );
    }

    #[test]
    fn reorder_alone_is_a_frame_with_order_and_no_rows() {
        let mut t = DeltaTracker::new();
        t.frame(&rows(json!([{"id": "a"}, {"id": "b"}])));
        let f = t.frame(&rows(json!([{"id": "b"}, {"id": "a"}]))).unwrap();
        assert_eq!(
            f,
            SessionsFrame::Delta {
                seq: 2,
                base_seq: 1,
                upsert: vec![],
                remove: vec![],
                order: Some(vec!["b".into(), "a".into()]),
            }
        );
    }

    #[test]
    fn snapshot_returns_last_list_at_current_seq_and_reset_forces_full() {
        let mut t = DeltaTracker::new();
        t.frame(&rows(json!([{"id": "a"}])));
        let latest = rows(json!([{"id": "a"}, {"id": "b"}]));
        t.frame(&latest);
        assert_eq!(t.snapshot(), SessionsFrame::Full { seq: 2, sessions: latest.clone() });
        t.reset();
        assert_eq!(
            t.frame(&latest),
            Some(SessionsFrame::Full { seq: 3, sessions: latest })
        );
    }

    #[test]
    fn frames_serialize_with_camel_case_tags() {
        let f = SessionsFrame::Delta {
            seq: 2,
            base_seq: 1,
            upsert: vec![],
            remove: vec![],
            order: None,
        };
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            json!({"kind": "delta", "seq": 2, "baseSeq": 1, "upsert": [], "remove": []})
        );
        let back: SessionsFrame = serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
}
