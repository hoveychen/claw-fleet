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
