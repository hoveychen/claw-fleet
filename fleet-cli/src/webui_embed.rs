//! The web UI bundle compiled into this binary, if the build embedded one.
//!
//! `build.rs` always generates the table: with the `embed-webui` feature off it
//! is empty, so "this build has no UI in it" is `EMBEDDED_WEBUI.is_empty()`
//! rather than a `cfg` the callers have to mirror. Files are stored gzipped and
//! inflated per request.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use claw_fleet_core::web_assets::{AssetSource, StaticAsset};
use claw_fleet_core::wiki::mime_for_path;

include!(concat!(env!("OUT_DIR"), "/webui_embed.rs"));

/// An [`AssetSource`] over the embedded bundle, or `None` when this build has
/// no bundle in it.
pub(crate) fn asset_source() -> Option<AssetSource> {
    if EMBEDDED_WEBUI.is_empty() {
        return None;
    }
    let files: HashMap<&'static str, &'static [u8]> = EMBEDDED_WEBUI.iter().copied().collect();
    Some(Arc::new(move |path: &str| {
        // build.rs keys the table on bundle-relative paths, which is a request
        // path minus its leading slash. Traversal needs no special handling
        // here the way it does for `from_dir`: a miss is a miss, and there is
        // no filesystem behind the map to escape into.
        let rel = path.trim_start_matches('/');
        let gz = files.get(rel)?;
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(*gz)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(StaticAsset {
            bytes,
            mime: mime_for_path(Path::new(rel)).to_string(),
        })
    }))
}

/// How many files the build embedded — for the startup line, so an operator can
/// tell "serving the built-in UI" apart from "serving an empty built-in UI".
pub(crate) fn embedded_file_count() -> usize {
    EMBEDDED_WEBUI.len()
}
