//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_wiki_docs(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let body = serde_json::to_string(&crate::wiki::list_docs()).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_wiki_doc(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw = query.get("slug").map(|s| s.as_str()).unwrap_or("");
                let slug = percent_decode_str(raw).decode_utf8_lossy().to_string();
                match crate::wiki::get_doc(&slug) {
                    Ok(doc) => {
                        let body = serde_json::to_string(&doc).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

pub(crate) fn route_wiki_file(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                let (slug, version, rel) = (dec("slug"), dec("version"), dec("path"));
                match crate::wiki::get_file(&slug, &version, &rel) {
                    Ok(f) => {
                        let mime_header: tiny_http::Header =
                            format!("Content-Type: {}", f.mime).parse().unwrap();
                        let _ = request.respond(
                            tiny_http::Response::from_data(f.bytes).with_header(mime_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

/// `/wiki_asset/<slug>/<version>/<relpath…>` — the same bytes
/// [`route_wiki_file`] answers, addressed as a path so relative refs inside a
/// published `index.html` resolve. The browser build's `<iframe src>` points
/// here because `fleet-wiki://` only exists on the Tauri webview.
///
/// The split mirrors the desktop protocol handler (`gui/mod.rs`) exactly:
/// `splitn(3, '/')` over the tail, decoding each part. Three parts, not more —
/// `rel` keeps its own slashes, which is the whole point, and the slug's `/`
/// arrives percent-encoded so it stays inside the first part.
///
/// No path-traversal check of its own: `wiki::get_file` already rejects `..`
/// and absolute paths before touching the fs, then canonicalizes and requires
/// the result to sit under the version dir. Re-normalizing the tail here would
/// only add a second, divergent opinion about what is safe.
pub(crate) fn route_wiki_asset_prefix(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let dec = |s: &str| percent_decode_str(s).decode_utf8_lossy().to_string();
    let tail = &path[crate::routes::WIKI_ASSET_PREFIX.len()..];
    let mut segs = tail.splitn(3, '/');
    let slug = dec(segs.next().unwrap_or(""));
    let version = dec(segs.next().unwrap_or(""));
    let rel = dec(segs.next().unwrap_or(""));
    match crate::wiki::get_file(&slug, &version, &rel) {
        Ok(f) => {
            let mime_header: tiny_http::Header =
                format!("Content-Type: {}", f.mime).parse().unwrap();
            let _ = request
                .respond(tiny_http::Response::from_data(f.bytes).with_header(mime_header));
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

pub(crate) fn route_wiki_search(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw = query.get("q").map(|s| s.as_str()).unwrap_or("");
                let q = percent_decode_str(raw).decode_utf8_lossy().to_string();
                let body =
                    serde_json::to_string(&crate::wiki::search_docs(&q)).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_wiki_export(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                match crate::wiki::export_doc(&dec("slug"), &dec("version")) {
                    Ok(e) => {
                        let mime_header: tiny_http::Header =
                            format!("Content-Type: {}", e.mime).parse().unwrap();
                        let _ = request.respond(
                            tiny_http::Response::from_data(e.bytes).with_header(mime_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

pub(crate) fn route_wiki_delete(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                let (slug, version) = (dec("slug"), dec("version"));
                let result = if version.is_empty() {
                    crate::wiki::delete_doc(&slug)
                } else {
                    crate::wiki::delete_version(&slug, &version)
                };
                match result {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
                        );
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

pub(crate) fn route_wiki_move(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req {
                    from: String,
                    to: String,
                }
                let moved = serde_json::from_slice::<Req>(&body_bytes)
                    .map_err(|e| format!("bad /wiki_move body: {e}"))
                    .and_then(|r| crate::wiki::move_doc(&r.from, &r.to));
                match moved {
                    Ok(doc) => {
                        let body = serde_json::to_string(&doc).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
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

pub(crate) fn route_wiki_move_folder(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req {
                    from: String,
                    to: String,
                }
                let moved = serde_json::from_slice::<Req>(&body_bytes)
                    .map_err(|e| format!("bad /wiki_move_folder body: {e}"))
                    .and_then(|r| crate::wiki::move_folder(&r.from, &r.to));
                match moved {
                    Ok(docs) => {
                        let body = serde_json::to_string(&docs).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
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

pub(crate) fn route_wiki_delete_folder(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req {
                    prefix: String,
                }
                let deleted = serde_json::from_slice::<Req>(&body_bytes)
                    .map_err(|e| format!("bad /wiki_delete_folder body: {e}"))
                    .and_then(|r| crate::wiki::delete_folder(&r.prefix));
                match deleted {
                    Ok(count) => {
                        let body = serde_json::json!({ "deleted": count }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
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

/// Publish markdown carried in the request body. The desktop reader has the
/// text, not a path, and on a remote workspace the file would be on the wrong
/// host — so content travels over the wire and the server owns the write.
pub(crate) fn route_wiki_publish_text(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Req {
        slug: String,
        /// Empty derives the title from the body.
        #[serde(default)]
        title: String,
        text: String,
        /// Tags the doc with its origin; empty falls back to the server's cwd.
        #[serde(default)]
        workspace_path: String,
        #[serde(default)]
        mode: crate::wiki::TextPublishMode,
    }
    let published = serde_json::from_slice::<Req>(&body_bytes)
        .map_err(|e| format!("bad /wiki_publish_text body: {e}"))
        .and_then(|r| {
            let workspace = if r.workspace_path.trim().is_empty() {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            } else {
                std::path::PathBuf::from(&r.workspace_path)
            };
            let title = (!r.title.trim().is_empty()).then_some(r.title.as_str());
            crate::wiki::publish_text(&r.slug, title, &r.text, &workspace, r.mode)
        });
    match published {
        Ok(doc) => {
            let body = serde_json::to_string(&doc).unwrap_or_default();
            let _ = request
                .respond(tiny_http::Response::from_string(body).with_header(json_header));
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
