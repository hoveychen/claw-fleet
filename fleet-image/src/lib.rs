//! The image-squeezing hot path, carved out of `claw-fleet-core` so it can be
//! optimized on its own.
//!
//! Everything here used to live in `claw_fleet_core::mobile_relay`. It moved out
//! for one reason: `image`'s decode/resize/encode code is generic, so it is
//! monomorphized into whichever crate calls it and therefore compiles at *that*
//! crate's `opt-level` — raising only `[profile.dev.package."*"]` never reached
//! it. Keeping the loop inside claw-fleet-core meant the whole workspace had to
//! build at `opt-level = 1` just so the mobile relay could answer a `tail`
//! inside the phone's 15s request timeout.
//!
//! With the loop in its own crate, the workspace root pins *this* package to
//! `opt-level = 3` in the dev profile and leaves `[profile.dev]` at 0. See the
//! comment above `[profile.dev]` in the root `Cargo.toml` for the measurements.

/// Byte budget a squeezed decision-asset image aims for so it survives the
/// mobile relay hop.
pub const DECISION_ASSET_TARGET_BYTES: usize = 50 * 1024;
/// Hard ceiling we never want a re-encoded asset to exceed. In practice the
/// shrink loop reaches [`DECISION_ASSET_TARGET_BYTES`] long before this; it only
/// documents the worst-case guarantee for a pathologically incompressible image.
pub const DECISION_ASSET_HARD_CAP_BYTES: usize = 100 * 1024;
/// JPEG quality steps, tried high→low at each resolution before shrinking.
pub const DECISION_ASSET_QUALITY_LADDER: [u8; 7] = [90, 80, 70, 60, 50, 40, 30];
/// Never shrink the longest edge below this — past it the asset is too small to
/// be worth showing and further shrinking buys almost nothing.
pub const DECISION_ASSET_MIN_DIM: u32 = 320;

/// Squeeze a decision-asset image toward [`DECISION_ASSET_TARGET_BYTES`] so it
/// survives the mobile relay hop, returning `(bytes, mime)`. Unlike a simple
/// cap, *every* decodable image is re-encoded — small ones too — because the
/// relay frame budget cares about absolute size, not how large the source was.
///
/// Strategy: at the source resolution, step JPEG quality high→low until an
/// encode fits the target; if even the lowest quality overshoots, shrink the
/// longest edge ~20% and retry, down to [`DECISION_ASSET_MIN_DIM`]. If we bottom
/// out without hitting the target, the smallest encode produced is returned
/// (best effort — a 320px JPEG at the lowest quality is far under the hard cap).
/// Anything that fails to decode (unknown/vector format) is returned untouched
/// so the caller's frame guard still applies.
pub fn downscale_decision_asset(bytes: Vec<u8>, mime: &str) -> (Vec<u8>, String) {
    downscale_image(
        bytes,
        mime,
        DECISION_ASSET_TARGET_BYTES,
        DECISION_ASSET_HARD_CAP_BYTES,
        DECISION_ASSET_MIN_DIM,
    )
}

/// The shared shrink loop behind [`downscale_decision_asset`] and the
/// transcript thumbnails: step JPEG quality high→low at the current
/// resolution, shrink the longest edge ~20% when even the lowest quality
/// overshoots `target`, and never return over `hard_cap` while `min_dim`
/// still allows shrinking. Undecodable input is returned untouched.
pub fn downscale_image(
    bytes: Vec<u8>,
    mime: &str,
    target: usize,
    hard_cap: usize,
    min_dim: u32,
) -> (Vec<u8>, String) {
    let Ok(img) = image::load_from_memory(&bytes) else {
        // Undecodable (e.g. a vector/unknown format) — nothing to re-encode; hand
        // the original back and let the caller's frame guard decide.
        return (bytes, mime.to_string());
    };
    let mut work = img;
    // Smallest encode seen so far — returned if no resolution/quality combo
    // reaches the target. Never stays None: the ladder runs at least once.
    let mut best: Option<Vec<u8>> = None;
    loop {
        for &quality in &DECISION_ASSET_QUALITY_LADDER {
            let Some(encoded) = encode_jpeg(&work, quality) else { continue };
            if best.as_ref().map_or(true, |b| encoded.len() < b.len()) {
                best = Some(encoded.clone());
            }
            if encoded.len() <= target {
                return (encoded, "image/jpeg".to_string());
            }
        }
        // Even the lowest quality overshot the target at this resolution. Stop
        // once we're below the min dimension *and* under the hard cap; but if a
        // pathological image is still over the hard cap, keep shrinking past the
        // floor until it fits, so the cap guarantee always holds.
        let longest = work.width().max(work.height());
        let under_cap = best.as_ref().map_or(false, |b| b.len() <= hard_cap);
        if (longest <= min_dim && under_cap) || longest <= 1 {
            break;
        }
        let nw = (work.width() * 4 / 5).max(1);
        let nh = (work.height() * 4 / 5).max(1);
        work = work.resize(nw, nh, image::imageops::FilterType::Lanczos3);
    }
    match best {
        Some(out) => (out, "image/jpeg".to_string()),
        // encode_jpeg never succeeded (should not happen) — fall back to original.
        None => (bytes, mime.to_string()),
    }
}

/// Re-encode `img` as an opaque RGB JPEG at `quality` (0-100). JPEG can't carry
/// alpha, so the image is flattened to RGB first. Returns None only on an
/// encoder error.
fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        std::io::Cursor::new(&mut out),
        quality,
    );
    encoder
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}
