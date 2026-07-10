//! End-to-end coverage of `fleet wiki list` / `fleet wiki search` scoping,
//! driving the real binary so arg parsing, workspace resolution and the wiki
//! store on disk are all exercised together.
//!
//! The scoping contract these tests pin down:
//!   * with no flags, both commands show only docs published from the caller's
//!     own workspace — that is what lets an agent ask "what do I already know
//!     about this repo?" without wading through every other project's docs;
//!   * a caller inside `<repo>/.worktrees/<x>` still sees docs published from
//!     `<repo>` (and vice versa), because a worktree is the same workspace;
//!   * `--all` restores the unfiltered view and `--workspace <substr>` keeps
//!     its original substring semantics.
//!
//! Each test points `FLEET_HOME` at its own tempdir, so docs land there instead
//! of the developer's `~/.fleet/wiki`.
//!
//! Drives `fleet-cli`, the real binary. (`target/debug/fleet` is an unrelated
//! 39-byte sidecar placeholder written by claw-fleet-desktop's build.rs.)

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

/// Run `fleet wiki <args...>` from `cwd`, with the wiki store under `fleet_home`.
fn wiki(fleet_home: &Path, cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(bin_path("fleet-cli"))
        .arg("wiki")
        .args(args)
        .current_dir(cwd)
        .env("FLEET_HOME", fleet_home)
        .output()
        .expect("spawn fleet-cli");
    assert!(
        out.status.success(),
        "fleet wiki {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

/// Slugs from `wiki list --json`, in the order the CLI printed them.
fn list_slugs(fleet_home: &Path, cwd: &Path, extra: &[&str]) -> Vec<String> {
    let mut args = vec!["list", "--json"];
    args.extend_from_slice(extra);
    let json: serde_json::Value = serde_json::from_str(&wiki(fleet_home, cwd, &args)).unwrap();
    json.as_array()
        .unwrap()
        .iter()
        .map(|d| d["slug"].as_str().unwrap().to_string())
        .collect()
}

/// Slugs from `wiki search <query> --json`.
fn search_slugs(fleet_home: &Path, cwd: &Path, query: &str, extra: &[&str]) -> Vec<String> {
    let mut args = vec!["search", query, "--json"];
    args.extend_from_slice(extra);
    let json: serde_json::Value = serde_json::from_str(&wiki(fleet_home, cwd, &args)).unwrap();
    json.as_array()
        .unwrap()
        .iter()
        .map(|h| h["slug"].as_str().unwrap().to_string())
        .collect()
}

/// Two sibling workspaces, one doc each, both mentioning "caching". `repo` also
/// carries an empty `.worktrees/feat` checkout-shaped subdirectory.
struct Fixture {
    _home: tempfile::TempDir,
    _root: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    worktree: PathBuf,
    other: PathBuf,
}

fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("myrepo");
    let worktree = repo.join(".worktrees").join("feat");
    let other = root.path().join("other");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&other).unwrap();

    std::fs::write(repo.join("perf.md"), "# Perf notes\n\n吞吐率 improved by caching.\n").unwrap();
    std::fs::write(other.join("misc.md"), "# Other doc\n\nunrelated caching text\n").unwrap();

    wiki(home.path(), &repo, &["publish", "perf.md"]);
    wiki(home.path(), &other, &["publish", "misc.md"]);

    Fixture {
        home: home.path().to_path_buf(),
        repo,
        worktree,
        other,
        _home: home,
        _root: root,
    }
}

#[test]
fn list_defaults_to_the_callers_workspace() {
    let f = fixture();
    assert_eq!(list_slugs(&f.home, &f.repo, &[]), ["perf"]);
    assert_eq!(list_slugs(&f.home, &f.other, &[]), ["misc"]);
}

#[test]
fn list_from_a_worktree_sees_the_repos_docs() {
    let f = fixture();
    // `perf` was published from the repo root; a caller standing in
    // `<repo>/.worktrees/feat` is in the same workspace and must still see it.
    assert_eq!(list_slugs(&f.home, &f.worktree, &[]), ["perf"]);
}

#[test]
fn list_all_spans_every_workspace() {
    let f = fixture();
    let mut slugs = list_slugs(&f.home, &f.repo, &["--all"]);
    slugs.sort();
    assert_eq!(slugs, ["misc", "perf"]);
}

#[test]
fn list_workspace_substring_still_selects_across_workspaces() {
    let f = fixture();
    assert_eq!(list_slugs(&f.home, &f.repo, &["--workspace", "other"]), ["misc"]);
}

#[test]
fn search_defaults_to_the_callers_workspace() {
    let f = fixture();
    // Both docs contain "caching"; only the local one is in scope.
    assert_eq!(search_slugs(&f.home, &f.repo, "caching", &[]), ["perf"]);
    assert_eq!(search_slugs(&f.home, &f.other, "caching", &[]), ["misc"]);
}

#[test]
fn search_all_spans_every_workspace() {
    let f = fixture();
    let mut slugs = search_slugs(&f.home, &f.repo, "caching", &["--all"]);
    slugs.sort();
    assert_eq!(slugs, ["misc", "perf"]);
}

#[test]
fn search_matches_content_case_insensitively_and_in_cjk() {
    let f = fixture();
    assert_eq!(search_slugs(&f.home, &f.repo, "CACHING", &[]), ["perf"]);
    assert_eq!(search_slugs(&f.home, &f.repo, "吞吐率", &[]), ["perf"]);
}

#[test]
fn search_reports_a_miss_without_failing() {
    let f = fixture();
    // Exit 0 (asserted inside `wiki`) with no hits — greppable, not an error.
    assert!(search_slugs(&f.home, &f.repo, "zzz-no-such-term", &["--all"]).is_empty());
    let human = wiki(&f.home, &f.repo, &["search", "zzz-no-such-term"]);
    assert!(human.contains("No doc matches"), "unexpected miss output: {human}");
}
