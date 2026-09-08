//! `fleet serve` routes for the artifact store (the 产出 page).
//!
//! Mirrors `routes_wiki` in shape, with one thing none of the other route
//! modules do: [`route_artifact_blob`] honours a `Range` request header and
//! answers `206 Partial Content`. Artifacts are the only thing Fleet serves
//! that can be a 400 MB video, and a `<video>` element seeks by asking for
//! ranges — answer every one of them with the whole file and the viewer has to
//! buffer the lot before it can jump.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

/// `GET /artifacts` — every artifact, newest first.
pub(crate) fn route_artifacts(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::artifacts::list()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// `GET /artifact?id=…` — one artifact's metadata.
pub(crate) fn route_artifact(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let id = decoded(query, "id");
    match crate::artifacts::get(&id) {
        Ok(a) => {
            let body = serde_json::to_string(&a).unwrap_or_default();
            let _ =
                request.respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

/// `GET /artifact_usage` — what the store occupies, for the cleanup UI.
pub(crate) fn route_artifact_usage(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::artifacts::usage()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// `POST /artifact_add` — ingest a file **on this host** into the store.
///
/// The path names a file on the probe's own filesystem, which is the point:
/// the agent that produced the deliverable ran here, so this is where the
/// bytes are. The desktop never uploads.
pub(crate) fn route_artifact_add(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    // snake_case on the wire, matching every other probe request body
    // (`WikiPublishTextReq` &c). The desktop's `ArtifactAddReq` is the only
    // client, and serde's default field naming keeps the two ends identical.
    #[derive(serde::Deserialize)]
    struct Req {
        source_path: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        note: String,
        #[serde(default)]
        workspace_path: String,
        #[serde(default)]
        session_id: Option<String>,
    }
    let added = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b).map_err(|e| format!("bad /artifact_add body: {e}"))
        })
        .and_then(|r| {
            let opt = |s: &str| if s.trim().is_empty() { None } else { Some(s.to_string()) };
            crate::artifacts::add(
                std::path::Path::new(&r.source_path),
                opt(&r.title).as_deref(),
                opt(&r.note).as_deref(),
                std::path::Path::new(&r.workspace_path),
                r.session_id.as_deref(),
            )
        });
    respond_json_result(request, json_header, added);
}

/// `POST /artifact_update` — patch title / note / starred / path. An absent
/// field means "leave it alone", which is why every one is an `Option`;
/// `path: ""` is therefore a real move (to the workspace root), not a no-op.
pub(crate) fn route_artifact_update(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        starred: Option<bool>,
        #[serde(default)]
        path: Option<String>,
    }
    let updated = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b).map_err(|e| format!("bad /artifact_update body: {e}"))
        })
        .and_then(|r| {
            crate::artifacts::update(
                &r.id,
                r.title.as_deref(),
                r.note.as_deref(),
                r.starred,
                r.path.as_deref(),
            )
        });
    respond_json_result(request, json_header, updated);
}

/// `POST /artifact_delete` — remove an artifact and its blob.
pub(crate) fn route_artifact_delete(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        id: String,
    }
    let deleted = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b).map_err(|e| format!("bad /artifact_delete body: {e}"))
        })
        .and_then(|r| crate::artifacts::delete(&r.id));
    match deleted {
        Ok(()) => {
            let _ = request
                .respond(tiny_http::Response::from_string("{}").with_header(json_header));
        }
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}

/// `POST /artifact_rollback` — make an older version current again.
pub(crate) fn route_artifact_rollback(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        id: String,
        version: String,
    }
    let rolled = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b)
                .map_err(|e| format!("bad /artifact_rollback body: {e}"))
        })
        .and_then(|r| crate::artifacts::rollback(&r.id, &r.version));
    respond_json_result(request, json_header, rolled);
}

// ── Folder export ────────────────────────────────────────────────────────────

/// `GET /artifact_folder_zip_plan?workspace_path=…&directory=…`
pub(crate) fn route_artifact_folder_zip_plan(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let workspace_path = decoded(query, "workspace_path");
    let directory = decoded(query, "directory");
    let plan = match crate::artifacts::artifacts_dir() {
        Some(root) => crate::artifacts::folder_zip_plan(&root, &workspace_path, &directory),
        None => crate::artifacts::FolderZip::default(),
    };
    let body = serde_json::to_string(&plan).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// `GET /artifact_folder_zip?workspace_path=…&directory=…` — the archive.
///
/// Packed to a temporary file first, then sent, rather than written straight
/// into the socket: tiny_http needs a length or a reader up front, and a
/// half-sent archive whose length was guessed is worse than a brief pause. The
/// temp file is what keeps peak memory flat — the whole point of the streaming
/// writer — and it is removed as soon as the response is handed off.
pub(crate) fn route_artifact_folder_zip(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let workspace_path = decoded(query, "workspace_path");
    let directory = decoded(query, "directory");
    let Some(root) = crate::artifacts::artifacts_dir() else {
        let _ = request.respond(tiny_http::Response::empty(500));
        return;
    };

    let tmp = std::env::temp_dir().join(format!("fleet-folder-{}.zip", uuid::Uuid::new_v4()));
    let report = match crate::artifacts::export_folder_zip(&root, &workspace_path, &directory, &tmp)
    {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
            return;
        }
    };

    match std::fs::File::open(&tmp).and_then(|f| f.metadata().map(|m| (f, m.len()))) {
        Ok((file, len)) => {
            let resp = tiny_http::Response::new(
                tiny_http::StatusCode(200),
                vec![
                    header("Content-Type", "application/zip"),
                    header(
                        "Content-Disposition",
                        &format!("attachment; filename=\"{}\"", ascii_fallback(&report.filename)),
                    ),
                ],
                file,
                Some(len as usize),
                None,
            );
            let _ = request.respond(resp);
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(500));
        }
    }
    let _ = std::fs::remove_file(&tmp);
}

/// A filename safe inside a quoted header value.
///
/// Folder names are routinely CJK, and a raw non-ASCII byte in a header is
/// not portable — so non-ASCII collapses to `_` here and the browser falls
/// back to the URL's own name. Quotes, backslashes and control characters
/// would let the value break out of its quoting, so they go too.
fn ascii_fallback(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() { c } else { '_' })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "artifacts.zip".to_string()
    } else {
        cleaned
    }
}

// ── Folders ──────────────────────────────────────────────────────────────────

/// `GET /artifact_folders` — every folder the user made, empty ones included.
pub(crate) fn route_artifact_folders(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::artifacts::list_folders()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// `POST /artifact_folder_create` — register a folder (and its ancestors).
pub(crate) fn route_artifact_folder_create(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        workspace_path: String,
        path: String,
    }
    let created = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b)
                .map_err(|e| format!("bad /artifact_folder_create body: {e}"))
        })
        .and_then(|r| {
            crate::artifacts::create_folder(std::path::Path::new(&r.workspace_path), &r.path)
        });
    respond_json_result(request, json_header, created);
}

/// `POST /artifact_folder_delete` — forget an *empty* folder. Core refuses one
/// that still holds anything, so this can never orphan a deliverable.
pub(crate) fn route_artifact_folder_delete(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        workspace_path: String,
        path: String,
    }
    let deleted = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b)
                .map_err(|e| format!("bad /artifact_folder_delete body: {e}"))
        })
        .and_then(|r| {
            crate::artifacts::delete_folder(std::path::Path::new(&r.workspace_path), &r.path)
        });
    // Same `{}`-or-400 shape as `/artifact_delete`, whose client code path this
    // shares.
    match deleted {
        Ok(()) => {
            let _ =
                request.respond(tiny_http::Response::from_string("{}").with_header(json_header));
        }
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}

/// `POST /artifact_folder_rename` — rename or re-nest a folder, carrying its
/// subfolders and everything filed under it. Answers the re-filed count.
pub(crate) fn route_artifact_folder_rename(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        workspace_path: String,
        from: String,
        to: String,
    }
    let moved = read_body(&mut request)
        .and_then(|b| {
            serde_json::from_slice::<Req>(&b)
                .map_err(|e| format!("bad /artifact_folder_rename body: {e}"))
        })
        .and_then(|r| {
            crate::artifacts::rename_folder(
                std::path::Path::new(&r.workspace_path),
                &r.from,
                &r.to,
            )
        });
    respond_json_result(request, json_header, moved);
}

/// `GET /artifact_blob?id=…` — the bytes, whole or ranged.
///
/// With no `Range` header this is a plain `200` carrying the whole blob, plus
/// `Accept-Ranges: bytes` so the client knows it *may* seek. With one, the
/// answer is `206` and a `Content-Range` naming the slice actually served —
/// which may be smaller than what was asked for, since the store caps a single
/// response at `MAX_RANGE_CHUNK`. A start past EOF is `416`, as the spec wants.
pub(crate) fn route_artifact_blob(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let id = decoded(query, "id");
    // `&version=v2` pins the response to one version; absent means current.
    let version = decoded(query, "version");
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| crate::artifacts::parse_range_header(h.value.as_str()));

    let version = if version.is_empty() { None } else { Some(version.as_str()) };
    match crate::artifacts::read_version_bytes(&id, version, range) {
        Ok(blob) => {
            let mut resp = tiny_http::Response::from_data(blob.bytes)
                .with_header(header("Content-Type", &blob.mime))
                .with_header(header("Accept-Ranges", "bytes"));
            if let Some((start, end)) = blob.range {
                resp = resp.with_status_code(206).with_header(header(
                    "Content-Range",
                    &format!("bytes {start}-{end}/{}", blob.total_size),
                ));
            }
            let _ = request.respond(resp);
        }
        // A range that starts past EOF is the one error worth distinguishing:
        // 416 tells the client to re-ask, 404 tells it to give up.
        Err(e) if range.is_some() && e.contains("past end of") => {
            let _ = request.respond(
                tiny_http::Response::empty(416)
                    .with_header(header("Accept-Ranges", "bytes")),
            );
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn header(name: &str, value: &str) -> tiny_http::Header {
    // Both sides are ours (route constants and store-derived mimes), so a
    // parse failure would be a bug, not bad input.
    format!("{name}: {value}").parse().expect("static header")
}

fn decoded(query: &std::collections::HashMap<String, String>, key: &str) -> String {
    query
        .get(key)
        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
        .unwrap_or_default()
}

fn read_body(request: &mut tiny_http::Request) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut request.as_reader(), &mut body)
        .map_err(|e| format!("read body: {e}"))?;
    Ok(body)
}

fn respond_json_result<T: serde::Serialize>(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
    result: Result<T, String>,
) {
    match result {
        Ok(v) => {
            let body = serde_json::to_string(&v).unwrap_or_default();
            let _ =
                request.respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}
