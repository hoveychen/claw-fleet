//! Guards the one build-profile invariant the mobile relay depends on: the
//! image squeeze loop must be compiled optimized even in a debug build.
//!
//! This is a *profile* test, not a logic test. The loop lives in `fleet-image`
//! precisely so the workspace root can pin that one package to `opt-level = 3`
//! while `[profile.dev]` stays at the cargo default — see the comment above
//! `[profile.dev]` in the root `Cargo.toml`. If someone drops that pin, every
//! `tail` the phone issues goes back to blowing its 15s request timeout, and
//! nothing else in the test suite would notice: the loop still returns the
//! right bytes, just ~6x slower.
//!
//! Measured on this fixture (4x 780x1688, 2026-09-08, 10-core mac), three runs
//! per row on an otherwise idle machine, varying only
//! `[profile.dev.package.fleet-image] opt-level`:
//!
//!   opt-level = 3 (what we ship) .... 1.50 / 1.51 / 1.52 s
//!   opt-level = 1 (the old profile) . 1.64 / 1.66 / 1.74 s
//!   opt-level = 0 (no pin at all) .. 11.3 / 11.3 / 11.4 s
//!
//! Note the middle row: pinning this crate to 3 is *faster* than the
//! `[profile.dev] opt-level = 1` arrangement it replaced, so moving the loop out
//! did not trade phone latency for compile time — it improved both.
//!
//! Measure this on an idle machine. A first run right after a rebuild, or a run
//! with sibling cargo builds competing, reads ~2x high (observed 2.6-3.1 s at
//! opt-level 3) — which is also why the ceiling below is set where it is rather
//! than snug against the shipped number.

use std::time::Instant;

/// Wall-clock ceiling for squeezing the fixture. Deliberately ~5x the shipped
/// 1.5s and ~1.4x under the 11.3s an unoptimized build takes: this asserts "the
/// loop is optimized", not a specific speed. Verified in both directions —
/// opt-level 0 trips it (11.9s under load), opt-level 3 and 1 do not.
const CEILING_SECS: f64 = 8.0;

/// Four 780x1688 images — the shape of the transcript screenshots that produced
/// the original 44s phone stall. Content is a deterministic gradient plus noise
/// so the JPEG ladder has something real to compress and the timing does not
/// depend on a lucky all-one-colour source.
fn fixture() -> Vec<Vec<u8>> {
    (0..4u32)
        .map(|n| {
            let mut buf = image::RgbImage::new(780, 1688);
            for (x, y, px) in buf.enumerate_pixels_mut() {
                let noise = (x
                    .wrapping_mul(2654435761)
                    .wrapping_add(y.wrapping_mul(2246822519))
                    >> 13) as u8;
                *px = image::Rgb([
                    (x / 4) as u8 ^ noise,
                    (y / 8) as u8,
                    ((x + y) / 6) as u8 ^ n.wrapping_mul(40) as u8,
                ]);
            }
            let mut png = Vec::new();
            image::DynamicImage::ImageRgb8(buf)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .expect("encode fixture png");
            png
        })
        .collect()
}

#[test]
fn the_squeeze_loop_is_compiled_optimized_even_in_debug() {
    let images = fixture();

    let started = Instant::now();
    for png in images {
        let (out, mime) = fleet_image::downscale_decision_asset(png, "image/png");
        // Sanity-check we actually did the work the timing is meant to cover —
        // a pass-through (undecodable input) would be fast and meaningless.
        assert_eq!(mime, "image/jpeg", "fixture must be re-encoded, not echoed");
        assert!(
            out.len() <= fleet_image::DECISION_ASSET_HARD_CAP_BYTES,
            "squeezed asset must respect the hard cap, got {} bytes",
            out.len()
        );
    }
    let elapsed = started.elapsed().as_secs_f64();

    assert!(
        elapsed < CEILING_SECS,
        "squeezing 4x 780x1688 took {elapsed:.1}s (ceiling {CEILING_SECS}s). \
         The loop is almost certainly building unoptimized — check that \
         `[profile.dev.package.fleet-image] opt-level = 3` is still in the root \
         Cargo.toml."
    );
    eprintln!("squeeze of 4x 780x1688 took {elapsed:.2}s");
}
