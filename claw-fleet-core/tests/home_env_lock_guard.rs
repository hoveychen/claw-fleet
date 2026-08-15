//! Drift-guard for the `FLEET_HOME` / `CODEX_HOME` test lock.
//!
//! `crate::paths::fleet_home_lock()` is a process-wide mutex that serialises
//! every test which repoints Fleet's home directories. It has to exist because
//! those homes are *environment variables*: `real_home_dir()` and
//! `get_codex_dir()` re-read them on every call, so one test's temp home is
//! visible to every other test running concurrently on cargo's thread pool.
//!
//! The lock is a convention, and a convention with no enforcement drifts. Two
//! tests had already stopped taking it, which made the whole suite flaky in a
//! way that pointed at innocent bystanders:
//!
//! * `headless_runtime`'s `isolated_fleet_home()` repointed `FLEET_HOME` with no
//!   lock at all. Caught red-handed: `interaction_mode_test`'s HOME assertion
//!   failed with `left: .../fleet-watchdog-…, right: .../fleet-headless-test-…`
//!   — two *other* tests' temp homes, observed one after the other inside a
//!   single assertion.
//! * `codex_launch::lists_codex_profiles_from_codex_home` repointed
//!   `CODEX_HOME` with no lock. `codex_guidance::reconcile_*` resolves
//!   `CODEX_HOME` on every call but captures its expected path once, so a
//!   mid-test flip sent the write to the other test's directory and left the
//!   assertions reading a stale file.
//!
//! Neither failure named its antagonist, and both landed on a different victim
//! each run — which is exactly why this is a guard test rather than a comment.
//!
//! ## The rule
//!
//! A `#[test]` fn must hold the lock if it perturbs one of these env vars,
//! whether it does so itself or through a same-file helper. A helper that takes
//! the lock *itself* (`with_temp_codex_home`, `with_temp_fleet_home`) is
//! self-guarding, and its callers need nothing — that is the preferred shape,
//! since it cannot be forgotten at the call site.

use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The env vars whose value decides where Fleet reads its state from.
///
/// `HOME` itself is not listed: no test repoints it process-wide (the spawn
/// paths bind it per-`Command`, which is local to that child).
const GUARDED_VARS: &[&str] = &["FLEET_HOME", "CODEX_HOME"];

/// The lock every mutator must be under, directly or through its helper.
const LOCK: &str = "fleet_home_lock";

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Does this snippet repoint one of the guarded homes?
fn mutates_home(body: &str) -> bool {
    GUARDED_VARS.iter().any(|var| {
        body.contains(&format!("set_var(\"{var}\""))
            || body.contains(&format!("remove_var(\"{var}\""))
    })
}

/// One `fn` (or `impl` block) with its brace-matched body.
struct Item {
    /// The bare identifier a caller would mention: a fn name, or the type name
    /// for an `impl` block (so `CodexHomeOverride::new` is matched by
    /// `CodexHomeOverride`).
    name: String,
    body: String,
    /// Present only for `#[test]` fns.
    is_test: bool,
    /// Line span, used to drop methods nested inside an `impl` block. Without
    /// this the guard would register the `fn new` of a `CodexHomeOverride` as a
    /// helper called `new`, and then flag every test that constructs anything.
    start: usize,
    end: usize,
    is_impl: bool,
}

/// Split one file into brace-matched `fn` / `impl` items.
///
/// A hand-rolled scan rather than a parser: the guard has to run in CI with no
/// extra dependency, and the shapes it cares about (a `fn` line, an `impl` line,
/// balanced braces) are unambiguous in this codebase.
fn items_of(src: &str) -> Vec<Item> {
    let fn_re = Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z0-9_]+)")
        .expect("fn regex");
    let impl_re = Regex::new(r"^\s*impl(?:<[^>]*>)?\s+(?:[A-Za-z0-9_:<>]+\s+for\s+)?([A-Za-z0-9_]+)")
        .expect("impl regex");

    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let as_impl = impl_re.captures(line);
        let is_impl = as_impl.is_some();
        let name = fn_re
            .captures(line)
            .or(as_impl)
            .map(|c| c[1].to_string());
        let Some(name) = name else { continue };
        if !line.contains('{') {
            continue;
        }

        // `#[test]` may sit a few attribute lines above the fn.
        let is_test = (i.saturating_sub(5)..i).any(|j| lines[j].trim() == "#[test]");

        let mut depth = 0i32;
        let mut body = String::new();
        let mut end = i;
        for (off, line) in lines[i..].iter().enumerate() {
            depth += line.matches('{').count() as i32;
            depth -= line.matches('}').count() as i32;
            body.push_str(line);
            body.push('\n');
            end = i + off;
            if depth <= 0 {
                break;
            }
        }
        items.push(Item {
            name,
            body,
            is_test,
            start: i,
            end,
            is_impl,
        });
    }

    // Drop methods nested inside an `impl`: the block already stands in for
    // them under its type name, and their bare names (`new`, `drop`) are far
    // too generic to match callers by.
    let impl_spans: Vec<(usize, usize)> = items
        .iter()
        .filter(|it| it.is_impl)
        .map(|it| (it.start, it.end))
        .collect();
    items.retain(|it| {
        it.is_impl
            || !impl_spans
                .iter()
                .any(|(s, e)| it.start > *s && it.end <= *e)
    });
    items
}

#[test]
fn every_test_that_repoints_a_fleet_home_holds_the_lock() {
    let mut files = Vec::new();
    rust_files(&src_dir(), &mut files);
    assert!(!files.is_empty(), "found no sources under {:?}", src_dir());

    let mut offenders: Vec<String> = Vec::new();
    let mut guarded_tests = 0usize;

    for file in &files {
        let Ok(src) = fs::read_to_string(file) else {
            continue;
        };
        if !mutates_home(&src) {
            continue;
        }
        let items = items_of(&src);

        // Helpers that repoint a home without taking the lock themselves. A
        // caller of one of these is on the hook; a caller of a self-guarding
        // helper is not.
        //
        // Grouped by name, because one name routinely spans several items: the
        // canonical shape here is a `TmpHome` struct whose `impl TmpHome::new`
        // takes the lock and parks the guard in a `_lock` field, plus an
        // `impl Drop for TmpHome` that restores the env. The Drop block mutates
        // and holds no lock of its own — judging it alone would condemn the very
        // pattern this guard wants everyone to use.
        let guarded_names: HashSet<&str> = items
            .iter()
            .filter(|it| !it.is_test && it.body.contains(LOCK))
            .map(|it| it.name.as_str())
            .collect();
        let unguarded_helpers: HashSet<&str> = items
            .iter()
            .filter(|it| !it.is_test && mutates_home(&it.body))
            .map(|it| it.name.as_str())
            .filter(|name| !guarded_names.contains(name))
            .collect();

        let name = file.file_name().unwrap_or_default().to_string_lossy();
        for test in items.iter().filter(|it| it.is_test) {
            let direct = mutates_home(&test.body);
            let via_helper = unguarded_helpers
                .iter()
                .any(|h| test.body.contains(&format!("{h}(")) || test.body.contains(&format!("{h}::")));
            if !direct && !via_helper {
                continue;
            }
            if test.body.contains(LOCK) {
                guarded_tests += 1;
                continue;
            }
            let how = if direct {
                "repoints it directly"
            } else {
                "repoints it through an unguarded helper"
            };
            offenders.push(format!("  {name}::{} — {how}", test.name));
        }
    }

    assert!(
        offenders.is_empty(),
        "these tests perturb FLEET_HOME/CODEX_HOME without holding \
         `crate::session::fleet_home_lock()`, so they race every other test that \
         reads a Fleet home:\n{}\n\nFix by taking the lock for the whole test \
         (`let _lock = crate::session::fleet_home_lock();` as the first line), or \
         — better — by moving the mutation into a helper that holds the lock for \
         the caller, the way `codex_guidance::with_temp_codex_home` does.",
        offenders.join("\n")
    );

    // A guard that matches nothing is a guard that has silently stopped
    // working — e.g. if the helper shapes it scans for are refactored away.
    assert!(
        guarded_tests >= 10,
        "only {guarded_tests} guarded tests found; the scanner has probably \
         stopped recognising the tests it is supposed to police"
    );
}
