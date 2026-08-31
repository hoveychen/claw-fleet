use super::*;

/// Turn a store read into the HTTP response `fleet-artifact://` answers with.
///
/// Split out of the protocol closure in `gui/mod.rs` because the status/header
/// branching is the part that can be wrong: a `200` where a `206` belongs makes
/// a `<video>` re-download from zero on every seek, and a `404` where a `416`
/// belongs stops playback for good instead of prompting a re-ask. Inside the
/// closure none of that is reachable without launching the app; out here it is
/// three assertions.
///
/// `had_range` is whether the *request* carried a `Range` header, which is what
/// separates "seek past the end" (416, recoverable) from "no such artifact"
/// (404, not).
pub(crate) fn artifact_response(
    result: Result<claw_fleet_core::artifacts::ArtifactBytes, String>,
    had_range: bool,
) -> tauri::http::Response<Vec<u8>> {
    match result {
        Ok(blob) => {
            let mut builder = tauri::http::Response::builder()
                .header("Content-Type", blob.mime)
                // Without this a media element will not attempt to seek at all.
                .header("Accept-Ranges", "bytes")
                .header("Access-Control-Allow-Origin", "*");
            builder = match blob.range {
                Some((start, end)) => builder.status(206).header(
                    "Content-Range",
                    format!("bytes {start}-{end}/{}", blob.total_size),
                ),
                None => builder.status(200),
            };
            builder.body(blob.bytes).unwrap()
        }
        Err(e) if had_range && e.contains("past end of") => tauri::http::Response::builder()
            .status(416)
            .header("Accept-Ranges", "bytes")
            .body(Vec::new())
            .unwrap(),
        Err(e) => tauri::http::Response::builder()
            .status(404)
            .header("Content-Type", "text/plain")
            .body(e.into_bytes())
            .unwrap(),
    }
}

// ── Artifact store (产出) ─────────────────────────────────────────────────────
//
// Every command routes through `state.backend`, so a remote workspace's
// artifacts list, preview and export the same as a local one. The bytes
// themselves reach the webview through the `fleet-artifact://` protocol
// registered in `gui/mod.rs`, which is the only surface here that speaks
// `Range`.

#[tauri::command(async)]
pub(crate) fn list_artifacts(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::artifacts::Artifact> {
    state.backend.read().unwrap().list_artifacts()
}

#[tauri::command(async)]
pub(crate) fn get_artifact(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::artifacts::Artifact, String> {
    state.backend.read().unwrap().get_artifact(&id)
}

/// Ingest a file into the store.
///
/// `source_path` names a file on whichever host serves this session — the
/// probe's filesystem for a remote workspace, this machine's for a local one.
/// That is deliberate: the agent that produced the deliverable ran there, so
/// that is where the bytes already are and no upload is involved.
#[tauri::command(async)]
pub(crate) fn add_artifact(
    source_path: String,
    title: String,
    note: String,
    workspace_path: String,
    session_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::artifacts::Artifact, String> {
    state.backend.read().unwrap().add_artifact(
        &source_path,
        &title,
        &note,
        &workspace_path,
        session_id.as_deref(),
    )
}

/// Patch title / note / starred. An omitted field is left alone, so the card's
/// star toggle cannot blank the title beside it.
#[tauri::command(async)]
pub(crate) fn update_artifact(
    id: String,
    title: Option<String>,
    note: Option<String>,
    starred: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::artifacts::Artifact, String> {
    state.backend.read().unwrap().update_artifact(
        &id,
        title.as_deref(),
        note.as_deref(),
        starred,
    )
}

#[tauri::command(async)]
pub(crate) fn delete_artifact(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.write().unwrap().delete_artifact(&id)
}

#[tauri::command(async)]
pub(crate) fn artifact_usage(
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::artifacts::StoreUsage {
    state.backend.read().unwrap().artifact_usage()
}

/// Copy an artifact to `dest` on **this** machine — the 导出 / 另存为 action.
///
/// Bytes come through the backend, so a remote artifact downloads
/// transparently; the save dialog runs on the frontend (plugin-dialog) and
/// hands us the chosen path. Mirrors `export_wiki_doc`.
///
/// Streamed in [`claw_fleet_core::artifacts::MAX_RANGE_CHUNK`] slices rather
/// than one `read_artifact_bytes(id, None)`: exporting a multi-gigabyte render
/// must not need a copy of it in memory first, and over a remote probe the
/// whole-file form would be one enormous HTTP response.
#[tauri::command(async)]
pub(crate) fn export_artifact(
    id: String,
    dest: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use std::io::Write;

    const CHUNK: u64 = claw_fleet_core::artifacts::MAX_RANGE_CHUNK;
    // Size first, because a ranged read of an empty blob is an error, not an
    // empty answer: `start >= total` is how "you seeked past the end" is
    // reported, and for a 0-byte artifact even `start = 0` satisfies that.
    // Exporting an empty deliverable must still produce an empty file.
    let size = state.backend.read().unwrap().get_artifact(&id)?.size_bytes;
    let mut file =
        std::fs::File::create(&dest).map_err(|e| format!("create '{dest}': {e}"))?;
    let mut offset: u64 = 0;
    while offset < size {
        let slice = {
            let backend = state.backend.read().unwrap();
            backend.read_artifact_bytes(&id, Some((offset, offset + CHUNK - 1)))?
        };
        let read = slice.bytes.len() as u64;
        // A backend that answers a range with nothing would otherwise spin
        // here forever rather than failing.
        if read == 0 {
            return Err(format!("artifact '{id}' returned no bytes at offset {offset}"));
        }
        file.write_all(&slice.bytes)
            .map_err(|e| format!("write '{dest}': {e}"))?;
        offset += read;
    }
    Ok(())
}

/// Absolute path of an artifact's blob on the host that serves it.
///
/// `None` for a remote workspace: the path would name a file on the probe's
/// machine, and handing that to "reveal in Finder" or "open with the system
/// app" would silently fail or, worse, open some unrelated local file at the
/// same path. The frontend hides both actions when this is `None` and offers
/// 导出 instead.
#[tauri::command(async)]
pub(crate) fn artifact_local_path(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    if state.backend.read().unwrap().is_remote() {
        return Ok(None);
    }
    let artifact = state.backend.read().unwrap().get_artifact(&id)?;
    let root = claw_fleet_core::artifacts::artifacts_dir()
        .ok_or_else(|| "cannot determine home dir".to_string())?;
    let path = claw_fleet_core::artifacts::blob_path(&root, &artifact);
    Ok(Some(path.display().to_string()))
}

/// Hand an artifact's blob to whatever application the OS opens it with.
///
/// Deliberately a backend command instead of the frontend's
/// `@tauri-apps/plugin-opener` `openPath`: that plugin command is scope-checked,
/// `opener:default` does not include `allow-open-path`, and even granting it
/// needs a non-empty path scope (`scope.rs` ANDs the fs scope with
/// `allowed.iter().any(..)`). Without that the command answers `ForbiddenPath`,
/// and a frontend that does not await the promise shows nothing — which is how
/// 「用系统应用打开」shipped as a dead button next to a working 「在访达中显示」
/// (`reveal_item_in_dir` has no scope check). `OpenerExt::open_path` on this
/// side is not scope-checked, and resolving the path here means the frontend
/// hands over an artifact id rather than an arbitrary path.
///
/// Like `artifact_local_path` this is a shell action, not a data-fetching
/// capability, so it stays off the Backend trait (same reasoning as
/// `reveal_path`) — and it is local-only, because a remote workspace's blob
/// lives on the probe's machine.
#[tauri::command(async)]
pub(crate) fn open_artifact_external(
    app: tauri::AppHandle,
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if state.backend.read().unwrap().is_remote() {
        return Err("artifact lives on the remote host".to_string());
    }
    let artifact = state.backend.read().unwrap().get_artifact(&id)?;
    let root = claw_fleet_core::artifacts::artifacts_dir()
        .ok_or_else(|| "cannot determine home dir".to_string())?;
    let path = claw_fleet_core::artifacts::blob_path(&root, &artifact);
    if !path.exists() {
        return Err(format!("file no longer exists: {}", path.display()));
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::artifact_response;
    use claw_fleet_core::artifacts::ArtifactBytes;

    fn blob(range: Option<(u64, u64)>, len: usize, total: u64) -> ArtifactBytes {
        ArtifactBytes {
            bytes: vec![7u8; len],
            mime: "video/mp4".into(),
            total_size: total,
            range,
        }
    }

    fn header<'a>(resp: &'a tauri::http::Response<Vec<u8>>, name: &str) -> Option<&'a str> {
        resp.headers().get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn a_whole_blob_is_200_but_still_advertises_seeking() {
        let resp = artifact_response(Ok(blob(None, 1024, 1024)), false);
        assert_eq!(resp.status(), 200);
        // The header a media element checks before it will try to seek at all.
        assert_eq!(header(&resp, "Accept-Ranges"), Some("bytes"));
        assert_eq!(header(&resp, "Content-Type"), Some("video/mp4"));
        assert!(resp.headers().get("Content-Range").is_none(), "a 200 must not claim a range");
        assert_eq!(resp.body().len(), 1024);
    }

    #[test]
    fn a_served_range_is_206_and_reports_the_full_size_not_the_slice() {
        // The total is what a player derives duration and scrub extent from; a
        // 206 that reported the slice length would make the timeline collapse
        // to whatever chunk arrived first.
        let resp = artifact_response(Ok(blob(Some((1000, 1099)), 100, 5_000_000)), true);
        assert_eq!(resp.status(), 206);
        assert_eq!(header(&resp, "Content-Range"), Some("bytes 1000-1099/5000000"));
        assert_eq!(resp.body().len(), 100);
    }

    #[test]
    fn seeking_past_the_end_is_416_while_a_missing_artifact_stays_404() {
        // Same error string, different meaning depending on whether the client
        // asked for a range: 416 says "re-ask", 404 says "give up".
        let past_eof = Err("range start 900 is past end of 'clip.mp4' (800)".to_string());
        assert_eq!(artifact_response(past_eof.clone(), true).status(), 416);
        assert_eq!(artifact_response(past_eof, false).status(), 404);

        let missing = Err("invalid artifact id '../escape'".to_string());
        assert_eq!(artifact_response(missing, true).status(), 404);
    }
}
