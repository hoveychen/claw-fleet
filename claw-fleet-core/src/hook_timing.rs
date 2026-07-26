//! Per-invocation timing for Fleet's synchronous hooks.
//!
//! Claude Code gives a hook a fixed wall-clock budget (5s for the Stop hook that
//! runs `fleet session idle`) and **SIGKILLs it on overrun**. Nothing in
//! `~/.fleet/hooks.jsonl` records a hook's exit code or how long it took — that
//! file is written by a *sibling* `cat >>` hook and only ever holds the inbound
//! payload. So when a hook silently fails to do its job, there is no way after
//! the fact to tell "it ran and decided not to act" from "it was killed
//! mid-flight".
//!
//! That gap left a real incident undiagnosable: session `f5c27989`'s Stop
//! payload carried `session_crons=1` + `background_tasks=1` +
//! `stop_hook_active=false` — every condition `bg_guard::block_reason` blocks on
//! — yet the turn ended anyway and the session sat idle for 26h47m. A later
//! isolation experiment showed the same guard blocking correctly on a fresh
//! headless session, so the mechanism works; the one-off cause could not be
//! recovered.
//!
//! This module closes that gap. A hook calls [`HookTiming::begin`] on entry and
//! [`HookTiming::end`] on every exit path, marking phases in between. Both a
//! `begin` and an `end` record land in `~/.fleet/hook-timing.jsonl`:
//!
//! - **`begin` with no matching `end`** ⇒ the hook was killed (timeout, or the
//!   session died under it). This is the signal that was missing.
//! - **`end` with per-phase `ms`** ⇒ which phase ate the budget. The prime
//!   suspect is `is_headless`, which scans every process on the machine.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};

/// Cap and post-truncation retention, mirroring [`crate::hooks::hooks_events_path`]'s
/// own truncation policy so this file can't grow without bound either.
const MAX_LINES: usize = 10_000;
const KEEP_LINES: usize = 2_000;

pub fn hook_timing_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("hook-timing.jsonl"))
}

/// An in-flight hook invocation. Create with [`HookTiming::begin`], mark phases
/// as they complete, and finish with [`HookTiming::end`].
///
/// Deliberately **not** a `Drop` impl: the path that matters most
/// (`std::process::exit(2)` when the guard blocks a turn) skips destructors, so
/// relying on `Drop` would silently lose exactly the records we came for. Every
/// exit path calls `end` explicitly.
pub struct HookTiming {
    hook: String,
    session_id: String,
    /// Monotonic start, for total elapsed.
    started: Instant,
    /// Start of the phase currently being measured.
    phase_started: Instant,
    /// `(phase name, elapsed ms)` in completion order.
    phases: Vec<(String, u128)>,
    /// A unique-per-invocation id so a `begin` can be paired with its `end` even
    /// when several hooks for the same session interleave in the file.
    invocation: String,
}

impl HookTiming {
    /// Record the start of a hook invocation. Never fails loudly: an
    /// unwritable log must not break the hook it is measuring.
    pub fn begin(hook: &str, session_id: &str) -> Self {
        let now = Instant::now();
        // Pid + a monotonic nanos suffix: unique per invocation without needing
        // a uuid dependency, and stable enough to pair begin↔end.
        let invocation = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let timing = HookTiming {
            hook: hook.to_string(),
            session_id: session_id.to_string(),
            started: now,
            phase_started: now,
            phases: Vec::new(),
            invocation,
        };
        timing.append(json!({
            "phase": "begin",
            "hook": timing.hook,
            "session_id": timing.session_id,
            "invocation": timing.invocation,
            "pid": std::process::id(),
        }));
        timing
    }

    /// Close the phase that just finished, attributing the time since the
    /// previous mark (or since `begin`) to `name`.
    pub fn phase(&mut self, name: &str) {
        let elapsed = self.phase_started.elapsed().as_millis();
        self.phases.push((name.to_string(), elapsed));
        self.phase_started = Instant::now();
    }

    /// Record the end of the invocation. `extra` is merged into the record —
    /// use it for the decision inputs worth correlating (cron/background-task
    /// counts, whether the turn was blocked).
    ///
    /// Consumes `self` so a second `end` for the same invocation can't compile.
    pub fn end(mut self, exit_code: i32, extra: Value) {
        self.phase("end");
        let phases: Value = self
            .phases
            .iter()
            .map(|(name, ms)| json!({"phase": name, "ms": ms}))
            .collect();
        let mut record = json!({
            "phase": "end",
            "hook": self.hook,
            "session_id": self.session_id,
            "invocation": self.invocation,
            "pid": std::process::id(),
            "exit_code": exit_code,
            "total_ms": self.started.elapsed().as_millis(),
            "phases": phases,
        });
        if let (Some(obj), Some(extra_obj)) = (record.as_object_mut(), extra.as_object()) {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
        self.append(record);
    }

    /// Best-effort append. Errors are swallowed by design — see [`begin`].
    fn append(&self, record: Value) {
        let Some(path) = hook_timing_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        use std::io::Write as _;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{record}");
        }
    }
}

/// Trim the timing log when it grows past [`MAX_LINES`], keeping the newest
/// [`KEEP_LINES`]. Same policy as [`crate::hooks::maybe_truncate_events_file`].
pub fn maybe_truncate_timing_file() {
    let Some(path) = hook_timing_path() else {
        return;
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return;
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() > MAX_LINES {
        let keep = &lines[lines.len() - KEEP_LINES..];
        let _ = fs::write(&path, keep.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Redirect `real_home_dir()` at a temp dir for the duration of a test.
    fn with_temp_home<T>(body: impl FnOnce(&std::path::Path) -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-hooktiming-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by the fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };
        let out = body(&tmp);
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);
        out
    }

    fn read_records(home: &std::path::Path) -> Vec<Value> {
        let path = home.join(".fleet").join("hook-timing.jsonl");
        let content = fs::read_to_string(path).unwrap_or_default();
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("each line must be valid JSON"))
            .collect()
    }

    #[test]
    fn begin_and_end_are_pairable_and_carry_phases() {
        let records = with_temp_home(|home| {
            let mut t = HookTiming::begin("session-idle", "sess-1");
            t.phase("is_headless");
            t.phase("mark_idle");
            t.end(0, json!({"crons": 1, "blocked": false}));
            read_records(home)
        });

        assert_eq!(records.len(), 2, "one begin + one end");
        let (begin, end) = (&records[0], &records[1]);
        assert_eq!(begin["phase"], "begin");
        assert_eq!(end["phase"], "end");
        assert_eq!(
            begin["invocation"], end["invocation"],
            "invocation id must pair the two records"
        );
        assert_eq!(end["hook"], "session-idle");
        assert_eq!(end["session_id"], "sess-1");
        assert_eq!(end["exit_code"], 0);
        // Extra fields are merged, not nested.
        assert_eq!(end["crons"], 1);
        assert_eq!(end["blocked"], false);
        // Phases recorded in order, plus the implicit trailing "end" phase.
        let names: Vec<&str> = end["phases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["phase"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["is_headless", "mark_idle", "end"]);
        assert!(end["total_ms"].is_number());
    }

    #[test]
    fn a_killed_hook_leaves_a_begin_with_no_end() {
        // The whole point of the module: this is what a SIGKILLed hook looks
        // like on disk, and it must be distinguishable from a clean run.
        let records = with_temp_home(|home| {
            let t = HookTiming::begin("session-idle", "sess-doomed");
            std::mem::forget(t); // simulate exec dying before `end`
            read_records(home)
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["phase"], "begin");
    }

    #[test]
    fn blocked_turn_records_its_nonzero_exit_code() {
        // bg_guard's block path exits 2; the record must survive it (which is
        // why `end` is called explicitly rather than via Drop).
        let records = with_temp_home(|home| {
            let t = HookTiming::begin("session-idle", "sess-blocked");
            t.end(2, json!({"blocked": true, "crons": 1, "background_tasks": 1}));
            read_records(home)
        });
        let end = &records[1];
        assert_eq!(end["exit_code"], 2);
        assert_eq!(end["blocked"], true);
    }

    #[test]
    fn truncation_keeps_the_newest_lines() {
        with_temp_home(|home| {
            let dir = home.join(".fleet");
            let _ = fs::create_dir_all(&dir);
            let path = dir.join("hook-timing.jsonl");
            let body: String = (0..MAX_LINES + 500)
                .map(|i| format!("{{\"n\":{i}}}\n"))
                .collect();
            fs::write(&path, body).unwrap();

            maybe_truncate_timing_file();

            let after = fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = after.lines().collect();
            assert_eq!(lines.len(), KEEP_LINES);
            // Newest kept, oldest dropped.
            let last: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
            assert_eq!(last["n"], (MAX_LINES + 500 - 1) as i64);
        });
    }

    #[test]
    fn under_the_cap_nothing_is_trimmed() {
        with_temp_home(|home| {
            let dir = home.join(".fleet");
            let _ = fs::create_dir_all(&dir);
            let path = dir.join("hook-timing.jsonl");
            fs::write(&path, "{\"n\":1}\n{\"n\":2}\n").unwrap();
            maybe_truncate_timing_file();
            assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
        });
    }
}
