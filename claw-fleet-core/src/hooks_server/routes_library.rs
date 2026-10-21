//! `fleet serve` routes for 资料 (Library): the per-project folder schema and
//! the merged wiki + artifact tree. Every handler is a thin call into
//! [`crate::library`] — the same functions the desktop's Tauri commands use.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

fn decoded(query: &std::collections::HashMap<String, String>, key: &str) -> String {
    query
        .get(key)
        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
        .unwrap_or_default()
}

fn parse_body<T: serde::de::DeserializeOwned>(
    request: &mut tiny_http::Request,
    route: &str,
) -> Result<T, String> {
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut request.as_reader(), &mut body)
        .map_err(|e| format!("read body: {e}"))?;
    serde_json::from_slice::<T>(&body).map_err(|e| format!("bad {route} body: {e}"))
}

/// `200` with the JSON value, or `400 {"error": …}`.
fn respond<T: serde::Serialize>(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
    result: Result<T, String>,
) {
    let (status, body) = match result {
        Ok(v) => (200, serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => (400, serde_json::json!({ "error": e }).to_string()),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// `GET /library_search?q=&workspace=&kind=&limit=` — ranked hits across the
/// wiki and artifact stores.
pub(crate) fn route_library_search(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let opts = crate::library_index::SearchOpts {
        workspace: Some(decoded(query, "workspace")).filter(|s| !s.is_empty()),
        kind: Some(decoded(query, "kind")).filter(|s| !s.is_empty()),
        limit: decoded(query, "limit").parse().unwrap_or(0),
    };
    respond(
        request,
        json_header,
        crate::library_index::search(&decoded(query, "q"), &opts),
    );
}

/// `GET /library_tree[?workspace=…]` — every project's tree, or one project's.
pub(crate) fn route_library_tree(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let workspace = decoded(query, "workspace");
    let ws = (!workspace.is_empty()).then_some(workspace.as_str());
    respond(request, json_header, crate::library::tree(ws));
}

/// `POST /library_folder_create` — `{workspace_path, path, description?}`.
pub(crate) fn route_library_folder_create(
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
        #[serde(default)]
        description: Option<String>,
    }
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_FOLDER_CREATE).and_then(
        |r| crate::library::create_folder(&r.workspace_path, &r.path, r.description.as_deref()),
    );
    respond(request, json_header, result);
}

/// `POST /library_folder_describe` — `{workspace_path, path, description}`.
pub(crate) fn route_library_folder_describe(
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
        description: String,
    }
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_FOLDER_DESCRIBE)
        .and_then(|r| crate::library::describe_folder(&r.workspace_path, &r.path, &r.description));
    respond(request, json_header, result);
}

/// `POST /library_folder_accept` — `{workspace_path, path}`: the user keeps a
/// folder an AI created, which clears its review marker.
pub(crate) fn route_library_folder_accept(
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
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_FOLDER_ACCEPT)
        .and_then(|r| crate::library::accept_folder(&r.workspace_path, &r.path));
    respond(request, json_header, result);
}

/// `POST /library_folder_delete` — `{workspace_path, path}`; answers `{}`.
/// Core refuses a folder that still holds anything.
pub(crate) fn route_library_folder_delete(
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
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_FOLDER_DELETE)
        .and_then(|r| crate::library::delete_folder(&r.workspace_path, &r.path))
        .map(|()| serde_json::json!({}));
    respond(request, json_header, result);
}

/// `POST /library_folder_rename` — `{workspace_path, from, to}`; answers what
/// was re-filed.
pub(crate) fn route_library_folder_rename(
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
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_FOLDER_RENAME)
        .and_then(|r| crate::library::rename_folder(&r.workspace_path, &r.from, &r.to));
    respond(request, json_header, result);
}

/// `POST /library_move` — `{kind, id, folder}`; `folder: ""` unfiles.
pub(crate) fn route_library_move(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        kind: String,
        id: String,
        folder: String,
    }
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_MOVE)
        .and_then(|r| crate::library::move_item(&r.kind, &r.id, &r.folder));
    respond(request, json_header, result);
}

// ── Uploads (`crate::library_ingest`) ───────────────────────────────────────

#[derive(serde::Deserialize)]
struct BatchReq {
    batch_id: String,
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    folder: Option<String>,
}

/// `POST /library_ingest_create` — `{workspace_path}`; answers the new
/// (receiving) batch.
pub(crate) fn route_library_ingest_create(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        workspace_path: String,
    }
    let result = parse_body::<Req>(&mut request, crate::routes::LIBRARY_INGEST_CREATE)
        .and_then(|r| crate::library_ingest::create_batch(&r.workspace_path));
    respond(request, json_header, result);
}

/// `POST /library_ingest_upload?batch=&path=` — the raw body is one file's
/// bytes, streamed to disk; `path` is its path inside the upload. Answers
/// `{"size": n}`.
pub(crate) fn route_library_ingest_upload(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let batch = decoded(query, "batch");
    let rel = decoded(query, "path");
    let result = crate::library_ingest::receive_file(&batch, &rel, &mut request.as_reader())
        .map(|size| serde_json::json!({ "size": size }));
    respond(request, json_header, result);
}

/// `POST /library_ingest_start` — `{batch_id}`; closes the batch and files it
/// in the background.
pub(crate) fn route_library_ingest_start(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let result = parse_body::<BatchReq>(&mut request, crate::routes::LIBRARY_INGEST_START)
        .and_then(|r| crate::library_ingest::start_batch(&r.batch_id));
    respond(request, json_header, result);
}

/// `GET /library_ingest_batches[?workspace=]` — batch summaries, newest first.
pub(crate) fn route_library_ingest_batches(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let workspace = decoded(query, "workspace");
    let ws = (!workspace.is_empty()).then_some(workspace.as_str());
    respond(request, json_header, crate::library_ingest::list_batches(ws));
}

/// `GET /library_ingest_batch?id=` — one batch with its items.
pub(crate) fn route_library_ingest_batch(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let id = decoded(query, "id");
    respond(request, json_header, crate::library_ingest::get_batch(&id));
}

/// `POST /library_ingest_undo` — `{batch_id, item_id?}`: one item, or the
/// whole batch without `item_id`. Answers the batch.
pub(crate) fn route_library_ingest_undo(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let result = parse_body::<BatchReq>(&mut request, crate::routes::LIBRARY_INGEST_UNDO).and_then(
        |r| match r.item_id.as_deref().filter(|i| !i.is_empty()) {
            Some(item) => crate::library_ingest::undo_item(&r.batch_id, item),
            None => crate::library_ingest::undo_batch(&r.batch_id),
        },
    );
    respond(request, json_header, result);
}

/// `POST /library_ingest_move` — `{batch_id, item_id, folder}`; `folder: ""`
/// unfiles. Answers the batch.
pub(crate) fn route_library_ingest_move(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let result = parse_body::<BatchReq>(&mut request, crate::routes::LIBRARY_INGEST_MOVE).and_then(
        |r| {
            crate::library_ingest::move_item(
                &r.batch_id,
                r.item_id.as_deref().unwrap_or(""),
                r.folder.as_deref().unwrap_or(""),
            )
        },
    );
    respond(request, json_header, result);
}

/// `POST /library_ingest_dismiss` — `{batch_id}`; answers `{}`.
pub(crate) fn route_library_ingest_dismiss(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let result = parse_body::<BatchReq>(&mut request, crate::routes::LIBRARY_INGEST_DISMISS)
        .and_then(|r| crate::library_ingest::dismiss_batch(&r.batch_id))
        .map(|()| serde_json::json!({}));
    respond(request, json_header, result);
}
