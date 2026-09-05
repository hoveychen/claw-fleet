//! Live end-to-end check for [`claw_fleet_core::codex_image::generate_image`].
//!
//! `#[ignore]` on purpose: this spawns a real `codex exec` turn and spends the
//! user's ChatGPT image quota (~1 image, and a generation turn reports a few
//! hundred K input tokens because the skill reads its own SKILL.md). Nothing in
//! CI should pay that. Run it by hand when the generation path changes:
//!
//! ```text
//! cargo test -p claw-fleet-core --test codex_image_live -- --ignored --nocapture
//! ```
//!
//! Requires a logged-in `codex` on PATH (ChatGPT plan; no `OPENAI_API_KEY`).

use claw_fleet_core::codex_image;

#[test]
#[ignore = "spends real image quota; run manually"]
fn generates_a_real_image_and_locates_it_by_thread_id() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let result = codex_image::generate_image(
        &workspace.path().to_string_lossy(),
        "A solid blue 1024x1024 square with a single white letter A centered in it. \
         Flat vector-poster look, no texture, no extra elements.",
        &[],
        None,
    )
    .expect("generation must succeed");

    assert!(
        !result.thread_id.trim().is_empty(),
        "thread id must come back so callers can re-locate the output"
    );
    assert!(
        !result.images.is_empty(),
        "at least one image, got {result:?}"
    );

    for img in &result.images {
        let path = std::path::Path::new(&img.path);
        assert!(path.is_file(), "reported path must exist: {}", img.path);
        assert!(img.bytes > 0, "reported size must be real: {img:?}");
        // The whole location contract: output lives under the thread's dir in
        // CODEX_HOME, not in the workspace we passed.
        let dir = codex_image::thread_images_dir(&result.thread_id).expect("thread dir");
        assert!(
            path.starts_with(&dir),
            "{} must sit under {}",
            img.path,
            dir.display()
        );
    }

    println!(
        "thread {} produced {} image(s): {:?}",
        result.thread_id,
        result.images.len(),
        result.images
    );
}

/// The iterate story end to end: generate, then revise in the same thread and
/// confirm round 2 reports only what round 2 made.
///
/// This is the assertion that unit tests cannot reach — `new_images_since` is
/// provably correct on synthetic input, but whether Codex actually writes a
/// follow-up turn's output into the *same* thread directory (rather than
/// minting a new thread) is a fact about Codex, not about our code.
#[test]
#[ignore = "spends real image quota twice; run manually"]
fn revision_in_the_same_thread_reports_only_the_new_image() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let ws = workspace.path().to_string_lossy().to_string();

    let first = codex_image::generate_image(
        &ws,
        "A flat vector poster: a solid orange 1024x1024 square, nothing else in it.",
        &[],
        None,
    )
    .expect("first generation must succeed");
    println!("round 1: thread {} -> {:?}", first.thread_id, first.images);

    let second = codex_image::edit_image(
        &ws,
        &first.thread_id,
        "Make the square deep purple instead of orange. Change nothing else.",
        &[],
        None,
    )
    .expect("revision must succeed");
    println!("round 2: {:?}", second.images);

    assert_eq!(
        second.thread_id, first.thread_id,
        "a revision must stay in the same thread, not mint a new one"
    );
    assert!(!second.images.is_empty(), "round 2 produced nothing");

    // The whole point of the before/after diff.
    let round1: std::collections::HashSet<&str> =
        first.images.iter().map(|i| i.path.as_str()).collect();
    for img in &second.images {
        assert!(
            !round1.contains(img.path.as_str()),
            "round 2 must not re-report round 1's {}",
            img.path
        );
    }

    // The timeline is the "what did it actually do" answer; a turn that ran
    // commands but reports an empty timeline means the parser drifted.
    assert!(
        !second.timeline.is_empty(),
        "expected some timeline events, got none"
    );
    for ev in &second.timeline {
        println!("  [{}] {}", ev.kind, ev.text);
    }
}
