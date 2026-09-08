//! Drift-guard for the zip browser's mime table.
//!
//! The 产出 page can open a .zip and preview a member inside it. The member
//! never passes through the store, so nothing derives its mime in Rust — the
//! frontend types it from its own copy of the extension table, in
//! `shared-ts/zipDir.ts`. If that copy drifts from `wiki::mime_for_path`, the
//! same `report.md` renders as markdown when it is an artifact and as
//! something else when it is a member of one, which is exactly the kind of
//! divergence nobody notices until a deliverable looks wrong.
//!
//! Parses the current TypeScript source rather than a frozen snapshot, like
//! `mobile_relay_drift_guard` does for the relay method names.

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

const ZIP_DIR_TS: &str = "../shared-ts/zipDir.ts";

fn manifest_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The `MIME_BY_EXT` literal, as `(extension, mime)` pairs.
fn ts_mime_table() -> Vec<(String, String)> {
    let p = manifest_path(ZIP_DIR_TS);
    let src = fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("drift-guard: cannot read {}: {e}", p.display()));
    let start = src
        .find("const MIME_BY_EXT")
        .expect("drift-guard: MIME_BY_EXT not found — was it renamed?");
    let body = &src[start..];
    let end = body.find("\n};").expect("drift-guard: MIME_BY_EXT is not closed");
    let entry = Regex::new(r#"(?m)^\s*"?([A-Za-z0-9_]+)"?:\s*"([^"]+)","#).unwrap();
    let out: Vec<(String, String)> = entry
        .captures_iter(&body[..end])
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    assert!(out.len() > 30, "drift-guard: parsed only {} entries", out.len());
    out
}

#[test]
fn zip_mime_table_matches_the_backend() {
    let mut wrong = Vec::new();
    for (ext, mime) in ts_mime_table() {
        let name = format!("sample.{ext}");
        let expected = claw_fleet_core::wiki::mime_for_path(Path::new(&name));
        if expected != mime {
            wrong.push(format!(".{ext}: TS says '{mime}', mime_for_path says '{expected}'"));
        }
    }
    assert!(wrong.is_empty(), "shared-ts/zipDir.ts drifted from wiki::mime_for_path:\n{}", wrong.join("\n"));
}

#[test]
fn zip_kind_table_matches_the_backend() {
    // `zipEntryKind` mirrors `artifacts::kind_for`; check the buckets that are
    // decided by extension rather than by mime prefix, which is where the two
    // could silently disagree.
    let cases = [
        ("a.docx", "doc"),
        ("a.doc", "doc"),
        ("a.odt", "doc"),
        ("a.rtf", "doc"),
        ("a.epub", "doc"),
        ("a.pages", "doc"),
        ("a.xlsx", "sheet"),
        ("a.xls", "sheet"),
        ("a.ods", "sheet"),
        ("a.numbers", "sheet"),
        ("a.pptx", "slides"),
        ("a.ppt", "slides"),
        ("a.odp", "slides"),
        ("a.key", "slides"),
        ("a.zip", "archive"),
        ("a.md", "text"),
        ("a.json", "text"),
        ("a.pdf", "pdf"),
        ("a.png", "image"),
        ("a.mp4", "video"),
        ("a.mp3", "audio"),
        ("a.bin", "other"),
    ];
    for (name, expected) in cases {
        let mime = claw_fleet_core::wiki::mime_for_path(Path::new(name));
        assert_eq!(
            claw_fleet_core::artifacts::kind_for(mime, name),
            expected,
            "kind_for({name}) — the TS mirror in shared-ts/zipDir.ts expects '{expected}'"
        );
    }
}
