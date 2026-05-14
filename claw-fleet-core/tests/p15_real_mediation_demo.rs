//! P15 — Real-Sonnet mediation walkthrough.
//!
//! Excluded from `cargo test` by default (`#[ignore]`) because it spawns
//! the actual Claude CLI and burns tokens. Run on demand:
//!
//! ```sh
//! cargo test -p claw-fleet-core --test p15_real_mediation_demo -- \
//!     --ignored --nocapture
//! ```
//!
//! Walks the full V2 flow against a real Claude CLI install:
//! 1. Fresh project workspace + git repo.
//! 2. Two diverging commits on the same line (worker vs. main).
//! 3. `merge_back` returns `Conflict { files, .. }` with 3-way specs.
//! 4. `merge_mediator::mediate(...)` calls Sonnet — real network round-trip.
//! 5. `apply_resolutions` writes the LLM's output, commits the merge.
//! 6. Verify: the merged file is sensible (not empty, no conflict markers),
//!    HEAD has two parents.

use std::path::Path;
use std::process::Command;

use claw_fleet_core::merge_mediator::{self, MediationError};
use claw_fleet_core::worktree::{apply_resolutions, merge_back, provision, MergeOutcome};

fn fresh_tmp(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "fleet-p15-{}-{}-{}",
        tag,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run_git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(path: &Path) {
    run_git(path, &["init", "-q", "-b", "main"]);
    run_git(path, &["config", "user.email", "demo@test"]);
    run_git(path, &["config", "user.name", "Demo"]);
}

#[test]
#[ignore = "requires Claude CLI; burns tokens — run manually"]
fn p15_real_sonnet_mediation_walkthrough() {
    let fleet_home = fresh_tmp("home");
    unsafe { std::env::set_var("FLEET_HOME", &fleet_home) };
    let workspace = fresh_tmp("workspace");
    init_repo(&workspace);

    // Seed shared.txt so we have a 3-way ancestor.
    std::fs::write(workspace.join("shared.txt"), "fn greet() {\n    \"hello world\"\n}\n").unwrap();
    run_git(&workspace, &["add", "shared.txt"]);
    run_git(&workspace, &["commit", "-q", "-m", "seed"]);

    // Worker branch — different greeting + slightly refactored.
    let task_id = "p15-task";
    let p_item_id = "p1";
    let wt = provision(&workspace, "main", task_id, p_item_id).unwrap();
    println!("[demo] worktree provisioned at {}", wt.display());
    std::fs::write(
        wt.join("shared.txt"),
        "fn greet() {\n    \"hello, friend\"\n}\n",
    )
    .unwrap();
    run_git(&wt, &["config", "user.email", "worker@test"]);
    run_git(&wt, &["config", "user.name", "Worker"]);
    run_git(&wt, &["add", "shared.txt"]);
    run_git(&wt, &["commit", "-q", "-m", "worker: friendlier greeting"]);

    // Main branch — different change on the same line.
    std::fs::write(
        workspace.join("shared.txt"),
        "fn greet() {\n    \"HELLO WORLD\"\n}\n",
    )
    .unwrap();
    run_git(&workspace, &["add", "shared.txt"]);
    run_git(&workspace, &["commit", "-q", "-m", "main: upper-case"]);

    // Step 1 — merge_back surfaces the conflict.
    println!("[demo] merge_back...");
    let outcome = merge_back(&workspace, "main", task_id, p_item_id).unwrap();
    let files = match outcome {
        MergeOutcome::Conflict { files, reason } => {
            println!("[demo] Conflict: {reason}");
            files
        }
        other => panic!("expected Conflict, got {other:?}"),
    };
    println!("[demo] {} conflicted file(s)", files.len());
    for f in &files {
        println!("  - {}", f.path.display());
        println!("    base: {:?}", f.base);
        println!("    ours: {:?}", f.ours);
        println!("    theirs: {:?}", f.theirs);
    }

    // Step 2 — real mediation via Sonnet.
    println!("[demo] calling Sonnet via claude CLI...");
    let resolutions = match merge_mediator::mediate(&files) {
        Ok(r) => r,
        Err(MediationError::ProviderUnavailable) => {
            eprintln!("[demo] Claude CLI not available — skipping rest");
            return;
        }
        Err(e) => panic!("mediation failed: {e}"),
    };
    println!("[demo] Sonnet returned {} resolution(s)", resolutions.len());
    for r in &resolutions {
        println!("  - {} →\n{}", r.path.display(), r.resolved_content);
    }

    // Step 3 — apply.
    let pairs: Vec<_> = resolutions
        .into_iter()
        .map(|r| (r.path, r.resolved_content))
        .collect();
    let final_outcome = apply_resolutions(&workspace, task_id, p_item_id, &pairs).unwrap();
    println!("[demo] final outcome: {final_outcome:?}");
    assert!(matches!(final_outcome, MergeOutcome::AutoMerged { .. }));

    // Sanity: file has no conflict markers, HEAD is a merge commit.
    let content = std::fs::read_to_string(workspace.join("shared.txt")).unwrap();
    println!("[demo] final shared.txt:\n{content}");
    assert!(!content.contains("<<<<<<<"));
    assert!(!content.contains("======="));
    assert!(!content.contains(">>>>>>>"));

    let parents = Command::new("git")
        .arg("-C")
        .arg(&workspace)
        .args(["log", "-1", "--format=%P"])
        .output()
        .unwrap();
    let parent_count = String::from_utf8_lossy(&parents.stdout)
        .trim()
        .split_whitespace()
        .count();
    assert_eq!(parent_count, 2, "should be a merge commit");
    println!("[demo] ✓ merge commit has 2 parents — Phase 2 mediation worked end-to-end");
}
