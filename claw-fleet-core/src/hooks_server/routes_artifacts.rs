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

/// `POST /artifact_update` — patch title / note / starred. An absent field
/// means "leave it alone", which is why every one is an `Option`.
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
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| parse_range_header(h.value.as_str()));

    match crate::artifacts::read_bytes(&id, range) {
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

/// Parse the single-range forms a media element actually sends:
/// `bytes=<start>-<end>` and the open-ended `bytes=<start>-`.
///
/// Multi-range (`bytes=0-99,200-299`) and suffix (`bytes=-500`) are refused
/// rather than approximated — returning the wrong bytes with a confident
/// `Content-Range` is worse than ignoring the header and sending a `200`,
/// which is always a legal answer to a range request. No client Fleet serves
/// uses either form; if one ever does, it degrades to a full download.
fn parse_range_header(value: &str) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => u64::MAX, // open-ended; the store clamps to the last byte
        e => e.parse().ok()?,
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::parse_range_header;

    #[test]
    fn parses_the_range_forms_a_media_element_sends() {
        assert_eq!(parse_range_header("bytes=0-1023"), Some((0, 1023)));
        assert_eq!(parse_range_header(" bytes=100-200 "), Some((100, 200)));
        // The open-ended form WKWebView and every <video> use to stream on.
        assert_eq!(parse_range_header("bytes=4096-"), Some((4096, u64::MAX)));
    }

    #[test]
    fn refuses_forms_it_would_only_be_guessing_at() {
        for bad in [
            "bytes=-500",        // suffix range: last 500 bytes, not supported
            "bytes=0-99,200-299", // multi-range
            "bytes=200-100",     // inverted
            "items=0-10",        // not a byte range
            "0-10",              // no unit
            "bytes=",
            "",
        ] {
            assert_eq!(parse_range_header(bad), None, "must refuse {bad:?}");
        }
    }
}
