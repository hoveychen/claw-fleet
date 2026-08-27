//! Compiles the web UI bundle into the binary when the `embed-webui` feature
//! is on.
//!
//! Why a build script rather than a runtime directory: Linux has no desktop
//! bundle, so `fleet webui` *is* the app there. Asking every Linux user to
//! download a second file and keep it next to the binary is a step that only
//! exists because of how we build, not because of anything they want.
//!
//! Why it is a feature and not the default: this same crate produces the
//! `fleet` probe that every macOS/Windows desktop bundle embeds twice (x64 +
//! arm64) as a resource for remote SSH deployment. That probe serves the HTTP
//! API to a desktop app that already has its own UI, so embedding the bundle
//! there would add ~8 MB × 2 to every installer to ship a UI nothing loads.
//! Default off, on only for the Linux release binary.
//!
//! Assets are stored gzipped (`Compression::best()`) and inflated per request:
//! the bundle is ~17 MB raw and ~7.7 MB compressed, and a local UI can afford
//! the few milliseconds an inflate costs far more easily than the binary can
//! afford the other 9 MB.

use std::io::Write;
use std::path::{Path, PathBuf};

/// What the generated file says when there is nothing embedded. The table is
/// generated either way so the runtime code compiles identically with and
/// without the feature — "no embedded bundle" is an empty slice, not a `cfg`.
const EMPTY_TABLE: &str = "pub static EMBEDDED_WEBUI: &[(&str, &[u8])] = &[];\n";

fn main() {
    println!("cargo:rerun-if-env-changed=FLEET_WEBUI_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is always set by cargo"));
    let generated = out_dir.join("webui_embed.rs");

    if std::env::var_os("CARGO_FEATURE_EMBED_WEBUI").is_none() {
        std::fs::write(&generated, EMPTY_TABLE).expect("write empty asset table");
        return;
    }

    let src = std::env::var("FLEET_WEBUI_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "feature `embed-webui` is enabled but FLEET_WEBUI_DIR is unset.\n\
                 Point it at a built bundle directory, e.g.\n  \
                 (cd claw-fleet-desktop && pnpm build) && \\\n  \
                 (cd mobile-web && pnpm run build:webui) && \\\n  \
                 mkdir -p webui && cp -R claw-fleet-desktop/dist/. webui/ && \\\n  \
                 cp -R mobile-web/dist-webui webui/m && \\\n  \
                 FLEET_WEBUI_DIR=$PWD/webui cargo build -p fleet-cli --features embed-webui"
            )
        });
    if !src.is_dir() {
        panic!("FLEET_WEBUI_DIR={} is not a directory", src.display());
    }

    let stage = out_dir.join("webui");
    // A stale file left by a previous bundle would otherwise keep being
    // included: the table is regenerated, but nothing prunes the staging dir.
    let _ = std::fs::remove_dir_all(&stage);

    let mut rels = Vec::new();
    collect(&src, String::new(), &mut rels);
    rels.sort();
    if rels.is_empty() {
        panic!("FLEET_WEBUI_DIR={} holds no files", src.display());
    }

    let mut table = String::from("pub static EMBEDDED_WEBUI: &[(&str, &[u8])] = &[\n");
    for rel in &rels {
        let from = src.join(rel);
        let to = stage.join(format!("{rel}.gz"));
        std::fs::create_dir_all(to.parent().expect("staged path always has a parent"))
            .expect("create staging dir");

        let raw = std::fs::read(&from).unwrap_or_else(|e| panic!("read {}: {e}", from.display()));
        let mut enc =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(&raw).expect("gzip write");
        let gz = enc.finish().expect("gzip finish");
        std::fs::write(&to, gz).unwrap_or_else(|e| panic!("write {}: {e}", to.display()));

        table.push_str(&format!("    ({rel:?}, include_bytes!({to:?})),\n"));
        println!("cargo:rerun-if-changed={}", from.display());
    }
    table.push_str("];\n");

    std::fs::write(&generated, table).expect("write asset table");
    println!("cargo:rerun-if-changed={}", src.display());
}

/// Collect every file under `dir` as a `/`-joined path relative to the bundle
/// root — the same shape a request path has once its leading slash is trimmed,
/// so lookup is a plain map hit with no per-request path juggling.
fn collect(dir: &Path, prefix: String, out: &mut Vec<String>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("read dir entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect(&path, rel, out);
        } else {
            out.push(rel);
        }
    }
}
