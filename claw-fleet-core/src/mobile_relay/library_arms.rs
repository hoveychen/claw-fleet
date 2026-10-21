//! The phone's 资料 (library) methods: the per-project folder tree, index
//! search, folder review and moves, and uploads into the ingest pipeline.
//!
//! Every handler is a thin call into the same core functions the desktop's
//! Tauri commands and `fleet serve`'s `/library_*` routes use
//! (`hooks_server/routes_library.rs`); only the upload transport is the
//! relay's own, because the relay has no streamed request body.
//!
//! ## Uploads over the relay
//!
//! `fleet serve` streams one file per `POST /library_ingest_upload`. A relay
//! request is one JSON frame instead, and `upload_attachment` caps that at
//! [`super::MAX_UPLOAD_BYTES`] — too small for a scanned PDF, and a cap a
//! phone user cannot do anything about. So a file travels in
//! [`MAX_CHUNK_BYTES`] pieces (`library_ingest_chunk`), appended to a part file
//! here, and the last piece hands the whole part to
//! [`crate::library_ingest::receive_file`] — the same call the HTTP route
//! makes, with the same per-file cap
//! ([`crate::library_ingest::MAX_UPLOAD_FILE_BYTES`]).
//!
//! A chunk names its `offset`. One that starts at the part's current length
//! is appended; one the part already ends with (a resend after a lost reply)
//! is acknowledged without writing; anything else is refused, so a gap or an
//! overlap can never be stored as a file.

use serde_json::{json, Value};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Most raw bytes one `library_ingest_chunk` may carry. Base64 and the relay's
/// envelope inflate it by about a half; 4 MiB stays well inside the frame
/// budget `upload_attachment` already lives with.
pub const MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// A part file nobody has appended to for this long is an abandoned upload.
const STALE_PART: Duration = Duration::from_secs(24 * 3600);

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    str_param(params, key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

// ── Tree, search ────────────────────────────────────────────────────────────

/// Every project's tree, or one project's (`workspace`). With `outline: true`
/// only the folders, their counts and the newest `recent` items of each
/// (default [`crate::library::OUTLINE_RECENT`]) — see
/// [`crate::library::outline`]; the phone opens a folder with
/// `library_folder_items`.
pub(super) fn serve_library_tree(params: &Value) -> Result<Value, String> {
    let ws = str_param(params, "workspace").filter(|s| !s.trim().is_empty());
    if params.get("outline").and_then(Value::as_bool).unwrap_or(false) {
        let recent = params
            .get("recent")
            .and_then(Value::as_u64)
            .map_or(crate::library::OUTLINE_RECENT, |n| n as usize);
        return to_value(crate::library::outline(ws, recent)?);
    }
    to_value(crate::library::tree(ws)?)
}

/// One folder's items and everything beneath it, with summaries and tags
/// (`folder: ""` = the unfiled ones).
pub(super) fn serve_library_folder_items(params: &Value) -> Result<Value, String> {
    to_value(crate::library::folder_items(
        required(params, "workspacePath")?,
        str_param(params, "folder").unwrap_or(""),
    )?)
}

/// One wiki doc, so opening an item does not need the whole `wiki_list`.
pub(super) fn serve_wiki_doc(params: &Value) -> Result<Value, String> {
    to_value(crate::wiki::get_doc(required(params, "slug")?)?)
}

/// One artifact's record, so opening an item does not need the whole
/// `artifact_list`.
pub(super) fn serve_artifact_get(params: &Value) -> Result<Value, String> {
    to_value(crate::artifacts::get(required(params, "id")?)?)
}

/// A list-sized JPEG of an image artifact, re-encoded here so a folder of
/// photographed receipts costs the phone ~12 KB a card instead of each full
/// photo. `{thumb: null}` for anything that is not an image or will not
/// decode — the card keeps its icon.
pub(super) fn serve_library_thumb(params: &Value) -> Result<Value, String> {
    let a = crate::artifacts::get(required(params, "id")?)?;
    if a.kind != "image" {
        return Ok(json!({ "thumb": null }));
    }
    // Keyed on the version as well, so a re-published image is re-encoded.
    let ident = format!("library-thumb:{}:{}:{}", a.id, a.current_version, a.size_bytes);
    let thumb = super::tail_thumb_cached(&ident, &a.mime, || {
        crate::artifacts::read_bytes(&a.id, None).ok().map(|b| b.bytes)
    });
    Ok(json!({ "thumb": thumb.map(|b64| format!("data:image/jpeg;base64,{b64}")) }))
}

/// Ranked hits across both stores, with `<mark>` snippets.
pub(super) fn serve_library_search(params: &Value) -> Result<Value, String> {
    let q = str_param(params, "query").unwrap_or("");
    if q.trim().chars().count() < 2 {
        return Ok(Value::Array(Vec::new()));
    }
    let opts = crate::library_index::SearchOpts {
        workspace: str_param(params, "workspace")
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        kind: str_param(params, "kind")
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        limit: params.get("limit").and_then(Value::as_u64).unwrap_or(0) as usize,
    };
    to_value(crate::library_index::search(q, &opts)?)
}

// ── Folder review and moves ─────────────────────────────────────────────────

/// Keep a folder an AI created (clears its review marker).
pub(super) fn serve_library_folder_accept(params: &Value) -> Result<Value, String> {
    to_value(crate::library::accept_folder(
        required(params, "workspacePath")?,
        required(params, "path")?,
    )?)
}

/// Rename / move a folder; answers what was re-filed.
pub(super) fn serve_library_folder_rename(params: &Value) -> Result<Value, String> {
    to_value(crate::library::rename_folder(
        required(params, "workspacePath")?,
        required(params, "from")?,
        required(params, "to")?,
    )?)
}

/// Delete an empty folder; core refuses one that still holds anything.
pub(super) fn serve_library_folder_delete(params: &Value) -> Result<Value, String> {
    crate::library::delete_folder(required(params, "workspacePath")?, required(params, "path")?)?;
    Ok(json!({}))
}

/// File a doc / artifact into a folder of its project (`folder: ""` unfiles).
pub(super) fn serve_library_move(params: &Value) -> Result<Value, String> {
    to_value(crate::library::move_item(
        required(params, "kind")?,
        required(params, "id")?,
        str_param(params, "folder").unwrap_or(""),
    )?)
}

// ── Upload batches ──────────────────────────────────────────────────────────

/// Open a receiving batch in `workspacePath`.
pub(super) fn serve_library_ingest_create(params: &Value) -> Result<Value, String> {
    to_value(crate::library_ingest::create_batch(required(params, "workspacePath")?)?)
}

fn parts_root() -> Result<PathBuf, String> {
    Ok(crate::library::Roots::home()?.library.join("relay-parts"))
}

/// Upload ids and batch ids name files here, so both stay in a safe alphabet.
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Drop part files of uploads abandoned mid-way (the phone went away).
fn sweep_stale_parts(root: &std::path::Path) {
    let Ok(batches) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for batch in batches.flatten() {
        let Ok(parts) = fs::read_dir(batch.path()) else {
            continue;
        };
        let mut left = 0;
        for part in parts.flatten() {
            let stale = part
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > STALE_PART);
            if stale {
                let _ = fs::remove_file(part.path());
            } else {
                left += 1;
            }
        }
        if left == 0 {
            let _ = fs::remove_dir(batch.path());
        }
    }
}

/// One piece of one file of a receiving batch.
///
/// Params: `batchId`, `uploadId` (the client's id for this file), `path` (its
/// path inside the upload — the bare name for a picked file), `offset`,
/// `base64`, and `last: true` on the final piece. Answers `{received}` (bytes
/// held so far) and, on the last piece, `{received, size, done: true}` once
/// the file is in the batch.
pub(super) fn serve_library_ingest_chunk(params: &Value) -> Result<Value, String> {
    use base64::Engine as _;
    let batch_id = required(params, "batchId")?;
    let upload_id = required(params, "uploadId")?;
    let rel_path = required(params, "path")?;
    if !safe_id(batch_id) || !safe_id(upload_id) {
        return Err("invalid upload id".to_string());
    }
    let offset = params
        .get("offset")
        .and_then(Value::as_u64)
        .ok_or("missing offset")?;
    let last = params.get("last").and_then(Value::as_bool).unwrap_or(false);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(str_param(params, "base64").ok_or("missing base64")?)
        .map_err(|e| format!("invalid base64: {e}"))?;
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(format!(
            "upload chunk too large: {} bytes (max {MAX_CHUNK_BYTES})",
            bytes.len()
        ));
    }
    let batch = crate::library_ingest::get_batch(batch_id)?;
    if batch.status != crate::library_ingest::BatchStatus::Receiving {
        return Err("this upload has already started".to_string());
    }

    let root = parts_root()?;
    if offset == 0 {
        sweep_stale_parts(&root);
    }
    let dir = root.join(batch_id);
    fs::create_dir_all(&dir).map_err(|e| format!("store upload: {e}"))?;
    let part = dir.join(format!("{upload_id}.part"));
    let held = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let end = offset + bytes.len() as u64;
    if end > crate::library_ingest::MAX_UPLOAD_FILE_BYTES {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "'{rel_path}' is larger than {} MiB",
            crate::library_ingest::MAX_UPLOAD_FILE_BYTES >> 20
        ));
    }
    if offset == held {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .map_err(|e| format!("store upload: {e}"))?;
        f.write_all(&bytes).map_err(|e| format!("store upload: {e}"))?;
    } else if !(offset < held && end == held) {
        // Not the next piece and not a resend of the last one.
        return Err(format!(
            "upload piece out of order: offset {offset}, {held} bytes held"
        ));
    }
    if !last {
        return Ok(json!({ "received": end }));
    }

    let mut file = fs::File::open(&part).map_err(|e| format!("store upload: {e}"))?;
    let result = crate::library_ingest::receive_file(batch_id, rel_path, &mut file);
    drop(file);
    let _ = fs::remove_file(&part);
    let _ = fs::remove_dir(&dir);
    let size = result?;
    Ok(json!({ "received": end, "size": size, "done": true }))
}

/// Close a receiving batch and file it in the background.
pub(super) fn serve_library_ingest_start(params: &Value) -> Result<Value, String> {
    to_value(crate::library_ingest::start_batch(required(params, "batchId")?)?)
}

/// Batch summaries, newest first — every project's, or one project's.
pub(super) fn serve_library_ingest_batches(params: &Value) -> Result<Value, String> {
    let ws = str_param(params, "workspace").filter(|s| !s.trim().is_empty());
    to_value(crate::library_ingest::list_batches(ws)?)
}

/// One batch with its items.
pub(super) fn serve_library_ingest_batch(params: &Value) -> Result<Value, String> {
    to_value(crate::library_ingest::get_batch(required(params, "batchId")?)?)
}

/// Undo one item (`itemId`) or, without it, the whole batch. Answers the batch.
pub(super) fn serve_library_ingest_undo(params: &Value) -> Result<Value, String> {
    let batch_id = required(params, "batchId")?;
    let batch = match str_param(params, "itemId").filter(|s| !s.is_empty()) {
        Some(item) => crate::library_ingest::undo_item(batch_id, item)?,
        None => crate::library_ingest::undo_batch(batch_id)?,
    };
    to_value(batch)
}

/// Re-file one item of a batch (`folder: ""` unfiles). Answers the batch.
pub(super) fn serve_library_ingest_move(params: &Value) -> Result<Value, String> {
    to_value(crate::library_ingest::move_item(
        required(params, "batchId")?,
        required(params, "itemId")?,
        str_param(params, "folder").unwrap_or(""),
    )?)
}

/// Forget a finished (or never-started) batch; what it filed stays filed.
pub(super) fn serve_library_ingest_dismiss(params: &Value) -> Result<Value, String> {
    let batch_id = required(params, "batchId")?;
    crate::library_ingest::dismiss_batch(batch_id)?;
    if safe_id(batch_id) {
        if let Ok(root) = parts_root() {
            let _ = fs::remove_dir_all(root.join(batch_id));
        }
    }
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::super::serve_request;
    use super::*;
    use base64::Engine as _;

    fn with_home<F: FnOnce(&std::path::Path)>(f: F) {
        let guard = crate::paths::fleet_home_guard_with(|| {
            let tmp = std::env::temp_dir().join(format!(
                "fleet-relay-library-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let _ = fs::create_dir_all(&tmp);
            tmp
        });
        let home = guard.home().to_path_buf();
        let ws = home.join("ws-receipts");
        fs::create_dir_all(&ws).unwrap();
        f(&ws);
        drop(guard);
        let _ = fs::remove_dir_all(&home);
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn chunk(batch: &str, upload: &str, offset: u64, bytes: &[u8], last: bool) -> Result<Value, String> {
        serve_request(
            "library_ingest_chunk",
            &json!({
                "batchId": batch, "uploadId": upload, "path": "receipt.jpg",
                "offset": offset, "base64": b64(bytes), "last": last,
            }),
        )
    }

    fn new_batch(ws: &std::path::Path) -> String {
        let b = serve_request(
            "library_ingest_create",
            &json!({ "workspacePath": ws.to_string_lossy() }),
        )
        .unwrap();
        assert_eq!(b["status"], "receiving");
        b["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn chunks_reassemble_into_one_received_file() {
        with_home(|ws| {
            let id = new_batch(ws);
            let body: Vec<u8> = (0..10u8).collect();
            assert_eq!(chunk(&id, "u1", 0, &body[..4], false).unwrap()["received"], 4);
            // A resend of the piece just stored is acknowledged, not appended.
            assert_eq!(chunk(&id, "u1", 0, &body[..4], false).unwrap()["received"], 4);
            // A gap is refused rather than stored as a corrupt file.
            let gap = chunk(&id, "u1", 6, &body[6..], true).unwrap_err();
            assert!(gap.contains("out of order"), "{gap}");
            assert_eq!(chunk(&id, "u1", 4, &body[4..8], false).unwrap()["received"], 8);
            let done = chunk(&id, "u1", 8, &body[8..], true).unwrap();
            assert_eq!((done["done"].clone(), done["size"].clone()), (json!(true), json!(10)));

            let batch = crate::library_ingest::get_batch(&id).unwrap();
            assert_eq!(batch.incoming.len(), 1);
            assert_eq!(batch.incoming[0].rel_path, "receipt.jpg");
            let roots = crate::library::Roots::home().unwrap();
            let stored = roots.library.join("ingest").join(&id).join(&batch.incoming[0].stored);
            assert_eq!(fs::read(stored).unwrap(), body);
            // The part file is gone once the file is in the batch.
            assert!(!parts_root().unwrap().join(&id).join("u1.part").exists());
        });
    }

    #[test]
    fn a_single_piece_file_and_an_oversized_piece() {
        with_home(|ws| {
            let id = new_batch(ws);
            let done = chunk(&id, "solo", 0, b"%PDF-1.4", true).unwrap();
            assert_eq!(done["size"], 8);
            let big = vec![0u8; MAX_CHUNK_BYTES + 1];
            let err = chunk(&id, "big", 0, &big, false).unwrap_err();
            assert!(err.contains("too large"), "{err}");
        });
    }

    #[test]
    fn chunks_are_refused_once_the_batch_started_and_ids_are_checked() {
        with_home(|ws| {
            let id = new_batch(ws);
            assert!(chunk(&id, "../x", 0, b"x", true).unwrap_err().contains("invalid"));
            chunk(&id, "a", 0, b"hello", true).unwrap();
            crate::library_ingest::queue_batch_in(&crate::library::Roots::home().unwrap(), &id)
                .unwrap();
            let err = chunk(&id, "b", 0, b"late", true).unwrap_err();
            assert!(err.contains("already started"), "{err}");
        });
    }

    #[test]
    fn stale_parts_are_swept() {
        let root = std::env::temp_dir().join(format!("fleet-relay-parts-{}", std::process::id()));
        let dir = root.join("b1");
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.part");
        fs::write(&old, b"x").unwrap();
        let f = fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - STALE_PART - Duration::from_secs(60)).unwrap();
        drop(f);
        sweep_stale_parts(&root);
        assert!(!dir.exists(), "an emptied batch dir goes too");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn outline_folder_items_open_and_thumb_arms_reach_core() {
        with_home(|ws| {
            let wsp = ws.to_string_lossy().to_string();
            crate::library::create_folder(&wsp, "发票", None).unwrap();
            let png = ws.join("receipt.png");
            image::RgbImage::from_pixel(900, 600, image::Rgb([200, 120, 40]))
                .save(&png)
                .unwrap();
            let shot = crate::artifacts::add(&png, None, None, ws, None).unwrap();
            crate::library::move_item("artifact", &shot.id, "发票").unwrap();
            let pdf = ws.join("loose.pdf");
            fs::write(&pdf, b"%PDF-1.4").unwrap();
            let loose = crate::artifacts::add(&pdf, None, None, ws, None).unwrap();

            let outline = serve_request(
                "library_tree",
                &json!({ "workspace": wsp, "outline": true, "recent": 0 }),
            )
            .unwrap();
            assert_eq!(outline[0]["counts"], json!({ "": 1, "发票": 1 }), "{outline}");
            assert_eq!(outline[0]["items"], json!([]));

            let items = serve_request(
                "library_folder_items",
                &json!({ "workspacePath": wsp, "folder": "发票" }),
            )
            .unwrap();
            assert_eq!(items.as_array().unwrap().len(), 1);
            assert_eq!(items[0]["id"], json!(shot.id));
            assert!(serve_request("library_folder_items", &json!({ "folder": "发票" })).is_err());

            let got = serve_request("artifact_get", &json!({ "id": loose.id })).unwrap();
            assert_eq!(got["name"], "loose.pdf");
            assert!(serve_request("wiki_doc", &json!({ "slug": "no/such" })).is_err());

            let thumb = serve_request("library_thumb", &json!({ "id": shot.id })).unwrap();
            let url = thumb["thumb"].as_str().expect("an image gets a thumbnail");
            assert!(url.starts_with("data:image/jpeg;base64,"), "{url}");
            assert!(url.len() < 30 * 1024, "thumbnail is list-sized: {}", url.len());
            let none = serve_request("library_thumb", &json!({ "id": loose.id })).unwrap();
            assert_eq!(none["thumb"], Value::Null);
        });
    }

    #[test]
    fn tree_search_and_folder_arms_reach_core() {
        with_home(|ws| {
            let wsp = ws.to_string_lossy().to_string();
            crate::library::create_folder(&wsp, "发票/2026", None).unwrap();
            let tree = serve_request("library_tree", &json!({ "workspace": wsp })).unwrap();
            let folders = tree[0]["folders"].as_array().unwrap();
            assert!(folders.iter().any(|f| f["path"] == "发票/2026"), "{tree}");

            let renamed = serve_request(
                "library_folder_rename",
                &json!({ "workspacePath": wsp, "from": "发票/2026", "to": "发票/二〇二六" }),
            );
            assert!(renamed.is_ok(), "{renamed:?}");
            serve_request(
                "library_folder_delete",
                &json!({ "workspacePath": wsp, "path": "发票/二〇二六" }),
            )
            .unwrap();
            let missing = serve_request("library_folder_accept", &json!({ "path": "x" }));
            assert!(missing.unwrap_err().contains("workspacePath"));
            // One-character queries answer nothing rather than everything.
            assert_eq!(
                serve_request("library_search", &json!({ "query": "x" })).unwrap(),
                json!([])
            );
        });
    }
}
