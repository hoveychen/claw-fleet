//! Live validation of dsh's multimodal input, end to end through Fleet's own
//! code path.
//!
//! Ignored by default: it runs a real turn against a vision model, which costs
//! real credits. Point `DSH_HOME` at a scratch copy of `~/.dsh` (credentials +
//! `profiles/` + `cordis.patch.yml`) so the probe session does not land in the
//! home a desktop is watching:
//!
//!   DSH_HOME=/tmp/dsh-home-probe \
//!   cargo test -p claw-fleet-core --test dsh_attachments_live -- --ignored --nocapture
//!
//! What it proves that the unit tests cannot: that an image attached the way a
//! composer attaches one — a store path in a trailing `Context files:` block —
//! actually reaches the model's eyes, and comes back out of `get_messages` as
//! something the transcript can render.

use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::{AgentSource, SpawnSpec};
use claw_fleet_core::dsh_source::{DshSource, DSH_URI_PREFIX};

/// A vision route: an image sent to a text-only model is admitted and stored,
/// but proves nothing about reaching the model.
const VISION_MODEL: &str = "deepseek-official/deepseek-v4-flash-vision-exp";

/// Fleet's process-global `dsh web` outlives every `DshSource`, so a test binary
/// has to reclaim it itself.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

/// A 96×96 PNG, left half red and right half blue. Asymmetric on purpose: the
/// model's answer can only be right by having actually seen it, which is the
/// whole point of the assertion below.
fn probe_png() -> Vec<u8> {
    let mut buf = image::RgbImage::new(96, 96);
    for (x, _, px) in buf.enumerate_pixels_mut() {
        *px = if x < 48 {
            image::Rgb([0xff, 0x00, 0x00])
        } else {
            image::Rgb([0x00, 0x00, 0xff])
        };
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(buf)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode the probe PNG");
    out.into_inner()
}

fn wait_for<T>(budget: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    None
}

#[test]
#[ignore = "runs a real vision turn (costs model credits); run manually with --ignored"]
fn live_an_attached_image_reaches_the_model_and_renders_from_the_store() {
    let _guard = ServerGuard;

    // 1. Stage the image the way a paste does: into the content-addressed store,
    //    whose path is what the composer then splices into the prompt.
    let bytes = probe_png();
    let stored = claw_fleet_core::user_attachments::ingest_bytes(&bytes, "probe.png")
        .expect("ingest the probe image");

    let source = DshSource::new();
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: format!(
                "这张图分成左右两半，各是什么颜色？只回答两个颜色词，不要用任何工具。\
                 \n\nContext files:\n- {}",
                stored.display()
            ),
            model: Some(VISION_MODEL.into()),
            ..Default::default()
        })
        .expect("spawn a dsh session");
    let session_id = spawned.session_id.expect("spawn must report an id");
    let uri = format!("{DSH_URI_PREFIX}{session_id}");

    // 2. Wait for the answer, then assert on it: only a model that saw the image
    //    can name both halves.
    let answer = wait_for(Duration::from_secs(180), || {
        let records = source.get_messages_tail(&uri, 50).ok()?;
        records.iter().rev().find_map(|r| {
            (r.get("type").and_then(|t| t.as_str()) == Some("assistant")).then(|| {
                r["message"]["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default()
            })
        }).filter(|s| !s.trim().is_empty())
    })
    .expect("the vision turn must produce an assistant answer");
    println!("model answered: {answer:?}");
    assert!(
        answer.contains('红') && answer.contains('蓝'),
        "the model must name both halves — it can only do that by seeing the \
         image, which means the attachment reached it as an image part, not a \
         path: {answer:?}"
    );

    // 3. The same history must render: the durable reference dsh logged comes
    //    back as a store path whose file is really there.
    let records = source.get_messages(&uri).expect("read the history back");
    let image = records
        .iter()
        .flat_map(|r| r["message"]["content"].as_array().into_iter().flatten())
        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("image"))
        .expect("the prompt's image must appear in the history");
    println!("rendered image block: {image}");
    assert_eq!(
        image["source"]["type"], "path",
        "the durable reference must be resolved to a store path"
    );
    let path = image["source"]["path"].as_str().expect("a path");
    assert_eq!(
        std::fs::read(path).expect("the rendered path must be readable"),
        bytes,
        "the rendered path must hold the exact bytes that were sent"
    );
    assert_eq!(
        path,
        stored.to_string_lossy(),
        "dsh's digest and Fleet's store key agree, so no second copy was made"
    );
}

/// The same attachment against a **text-only** route. Measured: dsh refuses the
/// whole call (`attachment-error` / `MODEL_DOES_NOT_SUPPORT_IMAGES`), prose
/// included — so without the fallback the user's message would simply be lost.
#[test]
#[ignore = "runs a real turn (costs model credits); run manually with --ignored"]
fn live_a_text_only_model_still_gets_the_prompt() {
    let _guard = ServerGuard;

    let stored = claw_fleet_core::user_attachments::ingest_bytes(&probe_png(), "probe.png")
        .expect("ingest the probe image");

    let source = DshSource::new();
    let prompt = format!(
        "只回答两个字：收到。不要用任何工具。\n\nContext files:\n- {}",
        stored.display()
    );
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: prompt.clone(),
            model: Some("deepseek-official/deepseek-v4-flash".into()),
            ..Default::default()
        })
        .expect("spawn must succeed despite the refused image");
    let uri = format!(
        "{DSH_URI_PREFIX}{}",
        spawned.session_id.expect("spawn must report an id")
    );

    let answer = wait_for(Duration::from_secs(180), || {
        let records = source.get_messages_tail(&uri, 50).ok()?;
        records
            .iter()
            .rev()
            .find_map(|r| {
                (r.get("type").and_then(|t| t.as_str()) == Some("assistant")).then(|| {
                    r["message"]["content"]
                        .as_array()
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter(|b| {
                                    b.get("type").and_then(|t| t.as_str()) == Some("text")
                                })
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default()
                })
            })
            .filter(|s| !s.trim().is_empty())
    })
    .expect("the degraded turn must still produce an answer");
    println!("text-only model answered: {answer:?}");

    // The prose survived, and the attachment stayed a path in the text — which
    // is exactly the pre-feature behaviour, not a lost message.
    let records = source.get_messages(&uri).expect("read the history back");
    let first_user_text = records
        .iter()
        .find(|r| r.get("type").and_then(|t| t.as_str()) == Some("user"))
        .and_then(|r| r["message"]["content"][0]["text"].as_str())
        .expect("the human prompt must be in the history");
    assert!(
        first_user_text.contains("Context files:") && first_user_text.contains("probe.png"),
        "the degraded prompt keeps its block: {first_user_text:?}"
    );
}
