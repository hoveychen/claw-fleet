//! Touches-hook decision logic — Claude Code hook intercepts Edit/Write tool
//! calls; this module decides whether the target path is within the
//! P-item's declared `touches` and, on violation, records a marker file the
//! supervisor polls.
//!
//! Per PRD §5.x patch §4 / TASKS P8:
//! - Hook reads the Edit/Write tool call payload from Claude Code.
//! - Looks up the worker's current P-item (`FLEET_TASK_ID` + `FLEET_P_ITEM_ID`
//!   env vars set by the worker spawn layer).
//! - Compares target path against `touches`; if outside, rejects the tool
//!   call AND drops a JSON marker in
//!   `~/.fleet/touches-violations/<task_id>.<p_item_id>.<ts>.json`.
//! - Supervisor watches that directory and (a) SIGSTOPs the worker session,
//!   (b) appends `[event] P<id> worker tried to modify undeclared file X,
//!   paused` to the master via `event_router::format_event`.
//!
//! This module is pure decision + file-write. The hook *handler binary*
//! lives in `fleet-cli` as a hidden subcommand and the Tauri install path is
//! in `claw-fleet-desktop`.
//!
//! ## Scope boundary — DEC-006 (Read is NOT intercepted) —
//!
//! This touches-boundary enforcement applies **only to the `Edit` and `Write`
//! tool calls** (mutation of the worktree). `Read` (and other read-only tool
//! calls — Grep / Glob / `cat`-style Bash) are deliberately NOT intercepted
//! and NOT subject to the `touches` allow-list: a worker may freely read any
//! file in the repo to understand context, only its *writes* are confined to
//! the declared `touches`. This is a ratified product decision (DEC-006), not
//! an oversight — do not "tighten" this to cover Read without re-opening that
//! decision, because read-confinement would break legitimate cross-file
//! reasoning (e.g. reading a sibling module to mirror its API). The hook
//! handler binary in `fleet-cli` is responsible for only dispatching `Edit`/
//! `Write` payloads into [`check_path_against_touches`]; this module never
//! sees a `Read` event by design.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pitem::PItemId;
use crate::session::get_fleet_dir;

/// Outcome of evaluating a Claude Code Edit/Write tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TouchesDecision {
    /// Target path is inside `touches` (or `touches` is empty — phase
    /// P-items don't declare touches, they don't edit files anyway and
    /// any edit would be a bug we still want surfaced to master).
    Allow,
    /// Target path is **not** in `touches`. The supervisor should
    /// SIGSTOP the worker and notify master, which then chooses one of three
    /// resolutions — extend `touches`, reject the edit, or escalate to the
    /// user.
    Block {
        attempted_path: PathBuf,
        declared: Vec<PathBuf>,
    },
}

/// Decide whether `attempted_path` is within `declared_touches`. Paths are
/// compared after canonicalization-best-effort (lexical normalization
/// only — we don't follow symlinks because the hook runs synchronously and
/// can't afford filesystem stat calls). Matching is suffix-based: any
/// declared path that ends-with the attempted path (or vice versa,
/// modulo separator boundary) counts as a match. This is intentionally
/// permissive — the planner's `touches` are often workspace-relative while
/// the agent's tool call has the full path. Worse to false-positive (block
/// legitimate edit, force user round-trip) than false-negative (silently
/// drift; master will still catch via acceptance audit).
///
/// This is the runtime boundary check invoked for every `Edit`/`Write` tool
/// call (); an out-of-range path returns [`TouchesDecision::Block`],
/// which the hook handler turns into a [`record_violation`] marker + worker
/// SIGSTOP (). `Read` calls never reach this function (DEC-006).
pub fn check_path_against_touches(
    attempted_path: &Path,
    declared_touches: &[PathBuf],
) -> TouchesDecision {
    if declared_touches.is_empty() {
        // Phase P-items / no-touches P-items: any edit is suspicious but
        // also could be a genuine omission. Block and let master decide.
        return TouchesDecision::Block {
            attempted_path: attempted_path.to_path_buf(),
            declared: vec![],
        };
    }
    let attempt = normalise(attempted_path);
    for declared in declared_touches {
        let d = normalise(declared);
        if paths_match(&attempt, &d) {
            return TouchesDecision::Allow;
        }
    }
    TouchesDecision::Block {
        attempted_path: attempted_path.to_path_buf(),
        declared: declared_touches.to_vec(),
    }
}

/// Suffix-aware match: `a` matches `b` if either is a suffix of the other
/// at a path-separator boundary. Examples:
/// - "/abs/repo/src/foo.rs" matches "src/foo.rs" (declared is workspace-relative)
/// - "src/foo.rs" matches "src/foo.rs" (literal)
/// - "src/foo.rs" does NOT match "src/foo_other.rs"
fn paths_match(a: &Path, b: &Path) -> bool {
    let a_s = a.to_string_lossy();
    let b_s = b.to_string_lossy();
    if a_s == b_s {
        return true;
    }
    is_suffix_of(&a_s, &b_s) || is_suffix_of(&b_s, &a_s)
}

fn is_suffix_of(longer: &str, shorter: &str) -> bool {
    if !longer.ends_with(shorter) {
        return false;
    }
    // Ensure the prefix before the match ends at a separator so we don't
    // match `foo.rs` against `bar_foo.rs`.
    let prefix_end = longer.len() - shorter.len();
    if prefix_end == 0 {
        return true;
    }
    let prefix = &longer[..prefix_end];
    prefix.ends_with('/') || prefix.ends_with('\\')
}

fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Persisted marker file the supervisor polls. One marker per out-of-`touches`
/// edit attempt; the append-only `~/.fleet/touches-violations/` log of these is
/// the physical carrier of the touches-violation deviation ledger.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TouchesViolationMarker {
    pub task_id: String,
    pub p_item_id: PItemId,
    pub attempted_path: PathBuf,
    pub declared_touches: Vec<PathBuf>,
    /// Worker session id (Claude Code session UUID) — used by supervisor
    /// to locate the worker subprocess for SIGSTOP.
    pub worker_session_id: Option<String>,
    pub recorded_at: i64,
}

/// `~/.fleet/touches-violations/`. Supervisor polls this directory; hook
/// handler writes one JSON per violation.
pub fn violations_dir() -> Result<PathBuf, String> {
    get_fleet_dir()
        .map(|d| d.join("touches-violations"))
        .ok_or_else(|| "could not resolve fleet home dir".to_string())
}

/// Persist a violation marker. Returns the file path written.
///
/// Append-only semantics: every call produces a **uniquely named** file — the
/// name carries a random uuid segment in addition to `task.p_item.recorded_at`
/// so two violations recorded within the same wall-clock second (same
/// task/p-item) never collide and never overwrite an earlier marker. The write
/// itself is atomic: we serialize to a sibling `.<uuid>.tmp` file then
/// `rename` it into place (atomic on the same filesystem), so the supervisor's
/// `drain_violations` reader never observes a half-written JSON. Any I/O
/// failure is surfaced as an `Err(String)` — this function never panics, so a
/// transient disk error degrades to a logged error at the call site rather
/// than killing the hook handler.
pub fn record_violation(marker: &TouchesViolationMarker) -> Result<PathBuf, String> {
    let dir = violations_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir violations dir: {e}"))?;
    // Unique, non-colliding filename: a uuid segment guarantees no overwrite
    // even when recorded_at (1s granularity) and task/p-item ids repeat.
    let unique = uuid::Uuid::new_v4().simple().to_string();
    let filename = format!(
        "{}.{}.{}.{}.json",
        marker.task_id, marker.p_item_id, marker.recorded_at, unique
    );
    let path = dir.join(&filename);
    let json = serde_json::to_string_pretty(marker).map_err(|e| format!("serialize: {e}"))?;
    // Atomic write: temp-then-rename so a polling drain never reads a partial
    // file. Temp lives in the same dir to keep the rename on one filesystem.
    let tmp_path = dir.join(format!(".{unique}.tmp"));
    std::fs::write(&tmp_path, &json).map_err(|e| format!("write marker tmp: {e}"))?;
    std::fs::rename(&tmp_path, &path).map_err(|e| {
        // Best-effort cleanup of the orphaned temp; ignore its error.
        let _ = std::fs::remove_file(&tmp_path);
        format!("rename marker into place: {e}")
    })?;
    Ok(path)
}

/// Read & remove every pending violation marker. Returns them in
/// timestamp order (oldest first) so the supervisor can replay them into
/// the master in arrival order. This is the supervisor's periodic drain of the
/// append-only violation log — the only operation permitted to delete markers
/// (the recorder never overwrites or deletes).
pub fn drain_violations() -> Result<Vec<TouchesViolationMarker>, String> {
    let Ok(dir) = violations_dir() else { return Ok(vec![]) };
    if !dir.exists() {
        return Ok(vec![]);
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read violations dir: {e}"))?;
    let mut markers: Vec<(PathBuf, TouchesViolationMarker)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                return None;
            }
            let s = std::fs::read_to_string(&p).ok()?;
            let m: TouchesViolationMarker = serde_json::from_str(&s).ok()?;
            Some((p, m))
        })
        .collect();
    markers.sort_by(|a, b| a.1.recorded_at.cmp(&b.1.recorded_at));
    let mut out = Vec::with_capacity(markers.len());
    for (path, marker) in markers {
        let _ = std::fs::remove_file(&path);
        out.push(marker);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }

    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn allow_exact_match() {
        let d = check_path_against_touches(
            Path::new("src/foo.rs"),
            &[pb("src/foo.rs"), pb("src/bar.rs")],
        );
        assert!(matches!(d, TouchesDecision::Allow));
    }

    #[test]
    fn allow_when_declared_is_workspace_relative_and_attempt_is_absolute() {
        let d = check_path_against_touches(
            Path::new("/home/user/repo/src/foo.rs"),
            &[pb("src/foo.rs")],
        );
        assert!(matches!(d, TouchesDecision::Allow));
    }

    #[test]
    fn block_unrelated_file() {
        let d = check_path_against_touches(Path::new("src/forbidden.rs"), &[pb("src/foo.rs")]);
        match d {
            TouchesDecision::Block { attempted_path, .. } => {
                assert_eq!(attempted_path, pb("src/forbidden.rs"));
            }
            _ => panic!("expected block"),
        }
    }

    #[test]
    fn does_not_falsely_match_prefix_string() {
        // `foo.rs` should NOT match `bar_foo.rs`.
        let d = check_path_against_touches(Path::new("src/bar_foo.rs"), &[pb("src/foo.rs")]);
        assert!(matches!(d, TouchesDecision::Block { .. }));
    }

    #[test]
    fn empty_touches_blocks_any_edit() {
        let d = check_path_against_touches(Path::new("src/foo.rs"), &[]);
        match d {
            TouchesDecision::Block { declared, .. } => assert!(declared.is_empty()),
            _ => panic!("expected block when touches list is empty"),
        }
    }

    #[test]
    fn normalise_collapses_dots() {
        let d = check_path_against_touches(
            Path::new("src/./inner/../foo.rs"),
            &[pb("src/foo.rs")],
        );
        assert!(matches!(d, TouchesDecision::Allow));
    }

    #[test]
    fn record_and_drain_roundtrip() {
        let _g = crate::session::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let m1 = TouchesViolationMarker {
            task_id: "t1".into(),
            p_item_id: "p1".into(),
            attempted_path: pb("src/forbidden.rs"),
            declared_touches: vec![pb("src/foo.rs")],
            worker_session_id: Some("sess-1".into()),
            recorded_at: 1,
        };
        let m2 = TouchesViolationMarker {
            task_id: "t1".into(),
            p_item_id: "p2".into(),
            attempted_path: pb("src/other.rs"),
            declared_touches: vec![pb("src/bar.rs")],
            worker_session_id: None,
            recorded_at: 2,
        };
        record_violation(&m1).unwrap();
        record_violation(&m2).unwrap();

        let drained = drain_violations().unwrap();
        assert_eq!(drained.len(), 2);
        // Sorted by recorded_at ascending.
        assert_eq!(drained[0].p_item_id, "p1");
        assert_eq!(drained[1].p_item_id, "p2");

        // Second drain returns nothing.
        assert!(drain_violations().unwrap().is_empty());
    }

    #[test]
    fn drain_returns_empty_when_dir_absent() {
        let _g = crate::session::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        assert!(drain_violations().unwrap().is_empty());
    }

    // acceptance: touches=['src/foo/'], attempt to write src/bar/ → block.
    #[test]
    fn req005_write_outside_declared_subtree_is_blocked() {
        let d = check_path_against_touches(
            Path::new("src/bar/thing.rs"),
            &[pb("src/foo/")],
        );
        match d {
            TouchesDecision::Block { attempted_path, declared } => {
                assert_eq!(attempted_path, pb("src/bar/thing.rs"));
                assert_eq!(declared, vec![pb("src/foo/")]);
            }
            _ => panic!("write to src/bar/ must be blocked when touches=['src/foo/']"),
        }
    }

    // acceptance: touches=['src/'], attempt to edit 'test/' → violation
    // recorded (Block decision, then record_violation persists a marker the
    // supervisor can drain to drive its extend/reject/escalate decision).
    #[test]
    fn req036_edit_outside_touches_records_violation_marker() {
        let _g = crate::session::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let decision = check_path_against_touches(
            Path::new("test/integration.rs"),
            &[pb("src/")],
        );
        let TouchesDecision::Block { attempted_path, declared } = decision else {
            panic!("editing test/ must be blocked when touches=['src/']");
        };

        // The hook handler turns a Block into a persisted marker.
        let marker = TouchesViolationMarker {
            task_id: "task-x".into(),
            p_item_id: "p3".into(),
            attempted_path,
            declared_touches: declared,
            worker_session_id: Some("sess-x".into()),
            recorded_at: 100,
        };
        let written = record_violation(&marker).unwrap();
        assert!(written.exists(), "marker file must be persisted");

        let drained = drain_violations().unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].attempted_path, pb("test/integration.rs"));
        assert_eq!(drained[0].declared_touches, vec![pb("src/")]);
    }

    // acceptance: write 10 violations, drain returns 10 in
    // recorded_at order. Includes same-second / same-task collisions to prove
    // unique-naming (no overwrite) holds even when task/p-item/recorded_at
    // all repeat.
    #[test]
    fn req009_ten_violations_drain_ordered_no_overwrite() {
        let _g = crate::session::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        for i in 0..10i64 {
            let marker = TouchesViolationMarker {
                task_id: "t".into(),
                p_item_id: "p".into(),
                // Two markers share recorded_at=5 to force a same-second
                // collision on the (task,p_item,recorded_at) key.
                attempted_path: pb(&format!("src/v{i}.rs")),
                declared_touches: vec![pb("src/allowed.rs")],
                worker_session_id: None,
                recorded_at: if i == 9 { 5 } else { i },
            };
            record_violation(&marker).unwrap();
        }

        // No marker overwrote another: all 10 files survive on disk.
        let dir = violations_dir().unwrap();
        let json_count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
            .count();
        assert_eq!(json_count, 10, "all 10 markers must persist (no overwrite)");

        let drained = drain_violations().unwrap();
        assert_eq!(drained.len(), 10, "drain returns all 10");
        // Ordered by recorded_at ascending (oldest first).
        let mut prev = i64::MIN;
        for m in &drained {
            assert!(m.recorded_at >= prev, "drain must be recorded_at-ordered");
            prev = m.recorded_at;
        }
        // Drained dir is now empty.
        assert!(drain_violations().unwrap().is_empty());
    }
}
