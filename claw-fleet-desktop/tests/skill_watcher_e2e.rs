//! Live end-to-end check for the cross-runtime skill watcher, against the REAL
//! Codex skills directory (`$CODEX_HOME/skills`, default `~/.codex/skills`).
//!
//! Unit tests in `skill_sync` prove `auto_reconcile` does the right thing when
//! called directly. This closes the plumbing gap: a *real* `notify` watcher on
//! the two skill roots, driven by *real* filesystem events, debounced, actually
//! triggers `auto_reconcile` and mirrors to the Codex root — the same wiring
//! `LocalBackend::new` sets up.
//!
//! Runs entirely inside an isolated `FLEET_HOME` + `CODEX_HOME` temp dir, so it
//! never touches the user's real skills (deletion propagation would otherwise
//! delete real files). It asserts the CORRECT Codex target `~/.codex/skills`,
//! which is what the earlier `~/.agents/skills` bug got wrong.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use claw_fleet_desktop::skill_sync;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

fn write_skill(root: &Path, slug: &str, body: &str) {
    let dir = root.join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

/// Run the P2 debounce loop for up to `deadline`, returning how many times
/// `auto_reconcile` fired (driven only by real watch events under the skill
/// roots). Stops after the first reconcile.
fn pump(
    rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    skills_watch_dirs: &[PathBuf],
    deadline: Duration,
) -> usize {
    let debounce = Duration::from_secs(2);
    let start = Instant::now();
    let mut pending = false;
    let mut last_event = Instant::now();
    let mut reconciles = 0usize;
    while start.elapsed() < deadline {
        if let Ok(Ok(event)) = rx.recv_timeout(Duration::from_millis(150)) {
            let is_create_or_remove =
                matches!(event.kind, EventKind::Create(_) | EventKind::Remove(_));
            if is_create_or_remove
                && event
                    .paths
                    .iter()
                    .any(|p| skills_watch_dirs.iter().any(|d| p.starts_with(d)))
            {
                pending = true;
                last_event = Instant::now();
            }
        }
        if pending && last_event.elapsed() >= debounce {
            let _ = skill_sync::auto_reconcile();
            reconciles += 1;
            break;
        }
    }
    reconciles
}

#[test]
fn watcher_mirrors_to_codex_home_and_propagates_deletion() {
    let temp = tempfile::tempdir().unwrap();
    // Canonicalize: macOS tempdirs live under /var/folders which symlinks to
    // /private/var/folders. FSEvents reports the canonical path, so watching the
    // /var path would never match. Real installs use ~/... with no such symlink.
    let home = fs::canonicalize(temp.path()).unwrap();
    unsafe {
        std::env::set_var("FLEET_HOME", &home);
        // Pin Codex home into the temp dir so get_codex_dir() → <home>/.codex.
        std::env::set_var("CODEX_HOME", home.join(".codex"));
    }

    let claude_skills = home.join(".claude/skills");
    let codex_skills = home.join(".codex/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    fs::create_dir_all(&codex_skills).unwrap();
    skill_sync::set_auto_sync_enabled(true).unwrap();
    assert!(
        skill_sync::both_runtimes_present(),
        "both runtimes present in the isolated home"
    );

    let skills_watch_dirs = vec![claude_skills.clone(), codex_skills.clone()];
    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default()).unwrap();
    watcher
        .watch(&claude_skills, RecursiveMode::Recursive)
        .unwrap();
    watcher
        .watch(&codex_skills, RecursiveMode::Recursive)
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // ── 1) New skill dropped into ~/.claude/skills → mirror to ~/.codex/skills. ──
    write_skill(
        &claude_skills,
        "live",
        "---\nname: live\ndescription: dropped at runtime\n---\nBody",
    );
    assert!(
        pump(&rx, &skills_watch_dirs, Duration::from_secs(15)) >= 1,
        "watch event should have triggered a reconcile"
    );
    assert!(
        home.join(".fleet/skills/live/SKILL.md").is_file(),
        "canonical copy should exist after auto-adopt"
    );
    assert!(
        codex_skills.join("live").exists(),
        "skill should be mirrored into ~/.codex/skills (NOT ~/.agents/skills)"
    );
    assert!(
        !home.join(".agents/skills/live").exists(),
        "must not project into the legacy ~/.agents/skills"
    );

    // ── 2) Delete from ~/.claude/skills → propagate to Codex + canonical. ──
    let claude_live = claude_skills.join("live");
    let _ = fs::remove_file(&claude_live).or_else(|_| fs::remove_dir_all(&claude_live));
    assert!(
        pump(&rx, &skills_watch_dirs, Duration::from_secs(15)) >= 1,
        "deletion should have triggered a reconcile"
    );
    assert!(
        !codex_skills.join("live").exists(),
        "deletion should propagate to ~/.codex/skills"
    );
    assert!(
        !home.join(".fleet/skills/live").exists(),
        "deletion should propagate to the canonical copy"
    );
}
