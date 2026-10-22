//! `fleet serve` routes for the artifact store (the artifacts page).
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
            let opt = |s: &str| {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            };
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

/// `GET /artifact_folder_zip?workspace_path=…&directory=…` — the archive,
/// streamed while it is packed (see [`send_zip`]).
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
    // The only request error the writer can hit; caught here so it is still
    // a JSON 400 rather than a 200 that ends early.
    let normalized = match crate::artifacts::normalize_dir_path(&directory) {
        Ok(d) => d,
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
            return;
        }
    };
    let filename = crate::artifacts::zip_filename(&normalized, &workspace_path);
    let headers = vec![
        header("Content-Type", "application/zip"),
        header(
            "Content-Disposition",
            &format!("attachment; filename=\"{}\"", ascii_fallback(&filename)),
        ),
    ];
    send_zip(request, headers, "artifact-folder-zip", move |sink| {
        crate::artifacts::write_folder_zip(&root, &workspace_path, &directory, sink).map(|_| ())
    });
}

/// Whether tiny_http will send a body of unknown length as it is produced.
///
/// It chunks one for an HTTP/1.1 client, but for an HTTP/1.0 client — or one
/// whose `TE` asks for identity — it needs a `Content-Length` and gets it by
/// `read_to_end` of the whole body into memory first
/// (`tiny_http::Response::raw_print`). For a multi-gigabyte archive that is
/// far worse than a temporary file, so those clients keep the old path.
/// Browsers and curl speak 1.1; the case this guards is a proxy in front of a
/// cloud container that talks 1.0 upstream (nginx's default).
pub(super) fn can_stream(request: &tiny_http::Request) -> bool {
    if *request.http_version() <= (1, 0) {
        return false;
    }
    !request
        .headers()
        .iter()
        .any(|h| h.field.equiv("TE") && h.value.as_str().to_ascii_lowercase().contains("identity"))
}

/// Answer `200` with `headers` and a body written by `produce`.
///
/// Streamed where the client allows it ([`can_stream`]): `produce` runs on its
/// own thread writing into a bounded [`crate::stream_pipe`], and tiny_http
/// sends the read half chunked, so nothing touches the disk, memory stays at a
/// few 64 KiB chunks however large the archive, the first byte leaves at
/// once, and a slow client slows the packer down rather than piling bytes up.
/// The price, accepted when this replaced the temp file: no `Content-Length`
/// (the browser shows no percentage), and an error after the first byte can
/// only truncate the download — the archive then lacks its central directory
/// and no extractor will open it.
///
/// Every request error must be caught *before* calling this: once it is
/// called the status is already `200`.
pub(super) fn send_zip<F>(request: tiny_http::Request, headers: Vec<tiny_http::Header>, label: &str, produce: F)
where
    F: FnOnce(&mut dyn std::io::Write) -> Result<(), String> + Send + 'static,
{
    if !can_stream(&request) {
        send_zip_via_temp_file(request, headers, label, produce);
        return;
    }
    let (reader, handle) = match crate::stream_pipe::spawn(label, move |w| produce(w)) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[fleet serve] {label}: cannot start packer thread: {e}");
            let _ = request.respond(tiny_http::Response::empty(500));
            return;
        }
    };
    let resp = tiny_http::Response::new(tiny_http::StatusCode(200), headers, reader, None, None);
    // Returns once the body is sent, the client is gone, or the packer failed.
    // Either way the reader is dropped by now, so the packer is finished or
    // fails on its next write — the join cannot wait on the client.
    let sent = request.respond(resp);
    match handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("[fleet serve] {label} stopped early: {e}"),
        Err(_) => eprintln!("[fleet serve] {label}: packer thread panicked"),
    }
    if let Err(e) = sent {
        eprintln!("[fleet serve] {label}: response not completed: {e}");
    }
}

/// The pre-streaming path, kept for clients [`can_stream`] rules out: pack
/// into a temporary file, then send it with its length.
fn send_zip_via_temp_file<F>(request: tiny_http::Request, headers: Vec<tiny_http::Header>, label: &str, produce: F)
where
    F: FnOnce(&mut dyn std::io::Write) -> Result<(), String>,
{
    let tmp = std::env::temp_dir().join(format!("fleet-{label}-{}.zip", uuid::Uuid::new_v4()));
    let written = std::fs::File::create(&tmp)
        .map_err(|e| format!("create '{}': {e}", tmp.display()))
        .and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            produce(&mut w)?;
            std::io::Write::flush(&mut w).map_err(|e| e.to_string())
        })
        .and_then(|()| {
            std::fs::File::open(&tmp)
                .and_then(|f| f.metadata().map(|m| (f, m.len())))
                .map_err(|e| e.to_string())
        });
    match written {
        Ok((file, len)) => {
            let resp = tiny_http::Response::new(tiny_http::StatusCode(200), headers, file, Some(len as usize), None);
            let _ = request.respond(resp);
        }
        Err(e) => {
            eprintln!("[fleet serve] {label} failed: {e}");
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request.respond(tiny_http::Response::from_string(body).with_status_code(500));
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
pub(super) fn ascii_fallback(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() {
                c
            } else {
                '_'
            }
        })
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
            crate::artifacts::rename_folder(std::path::Path::new(&r.workspace_path), &r.from, &r.to)
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
///
/// Conditional either way. `ArtifactsView` renders image artifacts as plain
/// `<img src=…>` against the full-size blob, so every pane switch and remount
/// re-fetched megabytes: 78 requests averaging 952KB — 76MB in 45 minutes on
/// the muvee host, 2026-09-21, all of it bytes the browser already had. The
/// `ETag` turns the repeats into empty 304s. `no-cache` keeps the round trip
/// rather than a freshness window, because a new version can land under the
/// same id and a stale image is worse than a conditional GET.
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
    let if_none_match = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("If-None-Match"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();

    let version = if version.is_empty() {
        None
    } else {
        Some(version.as_str())
    };
    // `&poster=1` answers a video's host-extracted poster frame (JPEG) instead
    // of its bytes; 404 when there is none, so the card keeps its icon.
    let result = if decoded(query, "poster") == "1" {
        crate::video_poster::read_poster(&id)
    } else {
        crate::artifacts::read_version_bytes(&id, version, range)
    };
    match result {
        Ok(blob) => {
            // Ahead of the range branch: RFC 9110 has If-None-Match win over
            // Range, and a client holding the current bytes wants neither.
            if etag_matches(&if_none_match, &blob.etag) {
                let _ = request.respond(
                    tiny_http::Response::empty(304)
                        .with_header(header("ETag", &blob.etag))
                        .with_header(header("Cache-Control", "private, no-cache"))
                        .with_header(header("Accept-Ranges", "bytes")),
                );
                return;
            }
            let mut resp = tiny_http::Response::from_data(blob.bytes)
                .with_header(header("Content-Type", &blob.mime))
                .with_header(header("Accept-Ranges", "bytes"))
                .with_header(header("ETag", &blob.etag))
                .with_header(header("Cache-Control", "private, no-cache"));
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
                tiny_http::Response::empty(416).with_header(header("Accept-Ranges", "bytes")),
            );
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

pub(super) fn header(name: &str, value: &str) -> tiny_http::Header {
    // Both sides are ours (route constants and store-derived mimes), so a
    // parse failure would be a bug, not bad input.
    format!("{name}: {value}").parse().expect("static header")
}

/// RFC 9110 If-None-Match evaluation against one strong tag: a comma-separated
/// candidate list, `*`, and the `W/` prefix an intermediary may have added.
fn etag_matches(header_value: &str, etag: &str) -> bool {
    if header_value.is_empty() || etag.is_empty() {
        return false;
    }
    header_value.split(',').any(|cand| {
        let cand = cand.trim();
        cand == "*" || cand == etag || cand.strip_prefix("W/") == Some(etag)
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn server() -> (Arc<tiny_http::Server>, u16) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        (Arc::new(server), port)
    }

    fn get(port: u16, version: &str) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(s, "GET /zip {version}\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        s
    }

    /// Status line and headers (lower-cased names), leaving the body unread.
    fn read_head(r: &mut BufReader<TcpStream>) -> (String, Vec<(String, String)>) {
        let mut status = String::new();
        r.read_line(&mut status).unwrap();
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let (k, v) = line.split_once(':').unwrap();
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
        (status, headers)
    }

    fn read_chunked(r: &mut BufReader<TcpStream>) -> Vec<u8> {
        let mut body = Vec::new();
        loop {
            let mut size = String::new();
            r.read_line(&mut size).unwrap();
            let n = usize::from_str_radix(size.trim(), 16).unwrap();
            if n == 0 {
                return body;
            }
            let mut chunk = vec![0u8; n + 2];
            r.read_exact(&mut chunk).unwrap();
            body.extend_from_slice(&chunk[..n]);
        }
    }

    /// Several members, the big one spanning many pipe chunks.
    fn pack(sink: &mut dyn Write) -> Result<(), String> {
        let big: Vec<u8> = (0..crate::stream_pipe::CHUNK * 10 + 123).map(|i| (i % 241) as u8).collect();
        let mut zip = crate::zip_stream::ZipStream::new(sink);
        zip.add("big.bin", big.len() as u64, &mut &big[..]).map_err(|e| e.to_string())?;
        zip.add("small.txt", 5, &mut &b"hello"[..]).map_err(|e| e.to_string())?;
        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn packed() -> Vec<u8> {
        let mut v = Vec::new();
        pack(&mut v).unwrap();
        v
    }

    #[test]
    fn an_http11_client_gets_the_archive_chunked_byte_for_byte() {
        let (srv, port) = server();
        let s2 = srv.clone();
        let t = std::thread::spawn(move || {
            let rq = s2.recv().unwrap();
            send_zip(rq, vec![header("Content-Type", "application/zip")], "t", pack);
        });
        let mut r = BufReader::new(get(port, "HTTP/1.1"));
        let (status, headers) = read_head(&mut r);
        assert!(status.contains(" 200 "), "{status}");
        assert!(headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked"), "{headers:?}");
        assert!(!headers.iter().any(|(k, _)| k == "content-length"), "{headers:?}");
        assert_eq!(read_chunked(&mut r), packed());
        t.join().unwrap();
    }

    #[test]
    fn an_http10_client_falls_back_to_a_sized_body() {
        let (srv, port) = server();
        let s2 = srv.clone();
        let t = std::thread::spawn(move || {
            let rq = s2.recv().unwrap();
            send_zip(rq, vec![header("Content-Type", "application/zip")], "t", pack);
        });
        let mut r = BufReader::new(get(port, "HTTP/1.0"));
        let (_, headers) = read_head(&mut r);
        let expected = packed();
        assert!(
            headers.iter().any(|(k, v)| k == "content-length" && *v == expected.len().to_string()),
            "{headers:?}"
        );
        let mut body = Vec::new();
        r.read_to_end(&mut body).unwrap();
        assert_eq!(body, expected);
        t.join().unwrap();
    }

    /// The case the bounded pipe is for: the client goes away mid-download.
    /// The packer must notice and stop, the handler must return, and the
    /// server must go on answering.
    #[test]
    fn a_client_that_disconnects_stops_the_packer_and_the_server_carries_on() {
        let (srv, port) = server();
        let stopped = Arc::new(AtomicBool::new(false));
        let s2 = srv.clone();
        let flag = stopped.clone();
        let t = std::thread::spawn(move || {
            let rq = s2.recv().unwrap();
            send_zip(rq, vec![], "t", move |sink| {
                // An endless member: only a failed write ends it.
                let chunk = vec![7u8; 64 * 1024];
                let res = loop {
                    if let Err(e) = sink.write_all(&chunk) {
                        break Err(e.to_string());
                    }
                };
                flag.store(true, Ordering::SeqCst);
                res
            });
            // Still serving.
            let rq = s2.recv().unwrap();
            let _ = rq.respond(tiny_http::Response::from_string("ok"));
        });

        let mut r = BufReader::new(get(port, "HTTP/1.1"));
        let _ = read_head(&mut r);
        let mut some = vec![0u8; 256 * 1024];
        r.read_exact(&mut some).unwrap();
        drop(r);

        let deadline = Instant::now() + Duration::from_secs(10);
        while !stopped.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "packer kept running after the client left");
            std::thread::sleep(Duration::from_millis(20));
        }

        let mut r = BufReader::new(get(port, "HTTP/1.1"));
        let (status, _) = read_head(&mut r);
        assert!(status.contains(" 200 "), "{status}");
        t.join().unwrap();
    }
}
