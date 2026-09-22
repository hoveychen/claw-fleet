//! Live end-to-end checks for [`claw_fleet_core::image_api`].
//!
//! `#[ignore]` on purpose: these spend real image quota. Nothing in CI should
//! pay that. Run by hand when the native generation path changes:
//!
//! ```text
//! cargo test -p claw-fleet-core --test image_api_live -- --ignored --nocapture
//! ```
//!
//! Auth comes from whatever [`image_api::load_auth`] resolves: `OPENAI_API_KEY`
//! if set, otherwise the logged-in Codex ChatGPT session in `~/.codex`.
//!
//! ## What running these established (2026-09-20, ChatGPT plan backend)
//!
//! The request shape was read out of codex-rs and had never been observed on
//! the wire. Sending it revealed that
//! `chatgpt.com/backend-api/codex/images/generations` **accepts `model`,
//! `quality` and `size` and then ignores all three**:
//!
//! - every quality tier — `low`, `xhigh`, `max` — echoed back as `low`;
//! - every requested size echoed back as dimensions of the backend's choosing
//!   (`1347x1167`, `1254x1254`, …), none of them the requested one;
//! - a nonsense model name (`gpt-image-9-does-not-exist`) still returned a
//!   picture, so the field is not even validated.
//!
//! So gpt-image-2.5's two tiers and its `xhigh`/`max` quality levels are **not
//! reachable through the plan quota**, only through a platform `OPENAI_API_KEY`
//! — which was unavailable when this was written, leaving that half unverified.
//!
//! This is why [`image_api::provenance`] reports the backend's echo and names
//! the controls that were dropped: without it, a silently-ignored `max` is
//! indistinguishable from an applied one.

use claw_fleet_core::image_api::{self, ImageRequest};

/// Print a result without dumping base64 into the terminal.
fn report(label: &str, result: &Result<claw_fleet_core::codex_image::GenerateImageResult, String>) {
    match result {
        Ok(r) => {
            println!(
                "[{label}] OK handle={} images={}",
                r.thread_id,
                r.images.len()
            );
            for img in &r.images {
                println!("[{label}]   {} ({} bytes)", img.path, img.bytes);
            }
            for ev in &r.timeline {
                println!("[{label}]   [{}] {}", ev.kind, ev.text);
            }
        }
        Err(e) => println!("[{label}] ERR {e}"),
    }
}

/// The path works end to end: a real request returns a real file on disk.
#[test]
#[ignore = "spends real image quota; run manually"]
fn a_real_request_produces_a_real_file() {
    let auth = image_api::load_auth(None).expect("no usable credential");
    println!("auth backend: {} ({})", auth.label(), auth.base_url());

    let mut req = ImageRequest::new(
        "A solid blue square with a single white letter A centered in it. \
         Flat vector-poster look, no texture, no extra elements.",
    );
    req.model = Some(image_api::MODEL_SUNBURST.to_string());
    req.quality = Some("high".to_string());
    req.size = Some("1024x1024".to_string());

    let result = image_api::run(&req, Some("live-test"));
    report("sunburst/high", &result);
    let ok = result.expect("request failed");
    assert_eq!(ok.images.len(), 1);
    assert!(ok.images[0].bytes > 0, "empty image file");
    assert!(
        std::path::Path::new(&ok.images[0].path).is_file(),
        "reported path does not exist"
    );
}

/// Regression guard on the finding above: while the plan backend ignores the
/// controls, the result line must SAY so. If this test starts failing because
/// nothing was reported as ignored, the backend began honouring them — good
/// news, but re-read the tool schema's "ONLY HONOURED WITH AN OPENAI_API_KEY"
/// warnings before celebrating.
#[test]
#[ignore = "spends real image quota; run manually"]
fn the_plan_backend_drops_the_controls_and_we_say_so() {
    let auth = image_api::load_auth(None).expect("no usable credential");
    if !matches!(auth, image_api::ImageAuth::ChatGpt { .. }) {
        println!("skipped: this documents plan-quota behaviour, and auth is an API key");
        return;
    }

    let mut req = ImageRequest::new("A single orange triangle on a white background.");
    req.model = Some(image_api::MODEL_FLARE.to_string());
    req.quality = Some("max".to_string());
    req.size = Some("2048x1152".to_string());

    let result = image_api::run(&req, Some("live-test"));
    report("flare/max", &result);
    let ok = result.expect("request failed");
    let note = &ok.timeline[0].text;
    assert!(
        note.contains("ignored by this backend"),
        "the dropped controls must be disclosed, got: {note}"
    );
}

/// The field is not validated at all on the plan backend, which is why the
/// schema cannot promise model selection there. Cheap when it rejects; one
/// image when it does not — and that outcome is the answer.
#[test]
#[ignore = "may spend image quota; run manually"]
fn a_nonsense_model_name_tells_us_whether_model_is_validated() {
    let auth = image_api::load_auth(None).expect("no usable credential");
    println!("auth backend: {} ({})", auth.label(), auth.base_url());

    let mut req = ImageRequest::new("A single grey dot.");
    req.model = Some("gpt-image-9-does-not-exist".to_string());
    let result = image_api::run(&req, Some("live-test"));
    report("bogus-model", &result);
    match result {
        Err(e) => println!(">>> model IS validated server-side: {e}"),
        Ok(_) => println!(">>> model is IGNORED server-side: a bogus name still produced an image"),
    }
}

/// Reference-image edits go through a different request encoding per backend
/// (multipart for an API key, JSON data URLs for the plan backend, which
/// rejects multipart with `400 Unsupported content type`). This is the one
/// check that the encoding actually reaches a picture.
#[test]
#[ignore = "spends real image quota; run manually"]
fn a_reference_image_edit_produces_a_real_file() {
    let auth = image_api::load_auth(None).expect("no usable credential");
    println!("auth backend: {} ({})", auth.label(), auth.base_url());

    let mut seed = ImageRequest::new("A single red circle on a white background.");
    seed.quality = Some("low".to_string());
    let seeded = image_api::run(&seed, Some("live-test"));
    report("seed", &seeded);
    let seeded = seeded.expect("seed generation failed");

    let mut req = ImageRequest::new("Same picture, but make the circle blue.");
    req.quality = Some("low".to_string());
    req.images.push(seeded.images[0].path.clone().into());
    let result = image_api::run(&req, Some("live-test"));
    report("edit", &result);
    let ok = result.expect("edit request failed");
    assert_eq!(ok.images.len(), 1);
    assert!(ok.images[0].bytes > 0, "empty image file");
}
