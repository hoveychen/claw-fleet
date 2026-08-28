//! The public file surface for Fleet Cloud.
//!
//! What remains of the OpenAI-Responses surface after ACP replaced it. The
//! agent protocol itself now lives in [`crate::acp`]; ACP carries files inline
//! in the prompt and has no REST file API, so this stays behind to give
//! `resource_link` something to point at and to accept uploads too large to be
//! comfortable as base64 inside a JSON-RPC frame.
//!
//! Confinement is unchanged and still holds by construction: the workspace is
//! bound server-side ([`public_workspace`]), a `file_id` round-trips a
//! workspace-relative path through base64url, and every read canonicalises the
//! joined path and asserts it stayed under the workspace root — so a crafted id
//! or a symlink cannot escape the container.

use serde::Serialize;

/// The workspace this container serves. One customer per container, so it is
/// bound server-side and never taken from a request.
/// `FLEET_PUBLIC_WORKSPACE` overrides the `/workspace` default.
pub fn public_workspace() -> String {
    std::env::var("FLEET_PUBLIC_WORKSPACE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/workspace".to_string())
}

// ─────────────────────────── Files (artifacts) ───────────────────────
//
// A customer's run produces files in the container's single workspace
// ([`public_workspace`]). We expose them OpenAI-file-shaped:
//   GET /v1/responses/{id}/files      → { object: "list", data: [file …] }
//   GET /v1/files/{file_id}/content   → raw bytes
// The `file_id` is an opaque `file_<base64url(rel_path)>`; content reads
// canonicalize the joined path and assert it stays under the canonical
// workspace root, so a crafted id can't escape the container.

use base64::Engine as _;

/// OpenAI-shaped file object. `bytes` is the size; `id` round-trips a
/// workspace-relative path through base64url.
#[derive(Debug, Clone, Serialize)]
struct FileObject {
    id: String,
    object: &'static str, // "file"
    bytes: u64,
    created_at: i64,
    filename: String,
    /// `"output"` for artifacts the run produced; for uploads, whatever the
    /// caller declared (OpenAI's own default for non-fine-tuning use is
    /// `user_data`).
    purpose: String,
}

/// Where `POST /v1/files` puts uploads, relative to the workspace root.
///
/// Inside the workspace on purpose: the agent reads an attachment by path, so
/// the bytes have to be somewhere it can reach. Each upload gets its own
/// subdirectory, so the caller's filename survives verbatim (it shows up in the
/// prompt, and `photo.png` reads better than a hash) without one upload ever
/// overwriting another.
const UPLOADS_DIR: &str = ".fleet-uploads";

/// Cap for a single upload. Same ceiling as a desktop attachment — one number
/// for "how big a blob may a caller hand the agent".
const MAX_UPLOAD_BYTES: u64 = crate::backend::MAX_ATTACHMENT_BYTES;

/// Reduce a caller-supplied filename to a bare, safe basename. Traversal
/// (`../`, absolute paths) and empty/`.`/`..` names collapse to a default, so
/// the join below can only land inside the upload's own directory. Pure.
fn sanitize_upload_filename(raw: &str) -> String {
    let base = std::path::Path::new(raw)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return "upload.bin".to_string();
    }
    cleaned.to_string()
}

/// True when a workspace-relative path lives under the uploads directory —
/// the only region `DELETE /v1/files/{id}` is allowed to touch. Pure.
fn is_upload_rel(rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    norm.starts_with(&format!("{UPLOADS_DIR}/"))
}

/// `file_<base64url(rel)>`. Pure.
fn encode_file_id(rel: &str) -> String {
    format!(
        "file_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rel.as_bytes())
    )
}

/// `file_<base64url(rel)>` → workspace-relative path. `None` if malformed. Pure.
fn decode_file_id(id: &str) -> Option<String> {
    let b64 = id.strip_prefix("file_").filter(|s| !s.is_empty())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Directories skipped when walking the workspace — VCS metadata and
/// regenerable build output that a customer never wants to download. Pure.
fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".worktrees"
            | ".venv"
            | "__pycache__"
    )
}

/// Directories that must never be exposed through `/v1`, even when a deployment
/// places them inside the workspace root.
///
/// This is not hygiene, it is the confinement boundary. A host that hands out a
/// single persistent volume (muvee mounts exactly one, at `/workspace`) pushes
/// an operator to park Fleet's state and the cred store's data inside the very
/// tree `/v1` serves — and then `.fleet-state/token` is Fleet's admin token and
/// `.foxy-switcher/agent-config.json` is the vault device token. Both were
/// listed *and* downloadable with nothing but the scoped public token on the
/// first cloud deployment. Choosing a workspace subdirectory avoids the overlap;
/// this list means getting that choice wrong is not catastrophic.
///
/// `.claude` / `.codex` / `.ssh` / `.aws` / `.gnupg` are here for the same
/// reason — they are credential stores by convention, so a workspace copy of one
/// is never something a scoped caller should fetch.
fn is_internal_dir(name: &str) -> bool {
    matches!(
        name,
        ".fleet"
            | ".fleet-state"
            | ".foxy-switcher"
            | ".claude"
            | ".codex"
            | ".ssh"
            | ".aws"
            | ".gnupg"
    )
}

/// True when a workspace-relative path traverses an [`is_internal_dir`]
/// component. Checked on read as well as on list: a file id is
/// `base64url(rel_path)`, so it is guessable — skipping these on the walk alone
/// would hide them from the listing while still serving them by id. Pure.
fn is_internal_rel(rel: &str) -> bool {
    rel.replace('\\', "/").split('/').any(is_internal_dir)
}

/// Cap the walk so an enormous tree can't blow up memory / the response.
const MAX_LISTED_FILES: usize = 2000;

/// List regular files under the workspace root (recursively, skipping ignored
/// dirs and symlinks), newest first, capped at [`MAX_LISTED_FILES`].
fn list_workspace_files() -> Vec<FileObject> {
    let root = std::path::PathBuf::from(public_workspace());
    let Ok(canon_root) = root.canonicalize() else {
        return Vec::new();
    };
    let mut out: Vec<(FileObject, std::time::SystemTime)> = Vec::new();
    let mut stack = vec![canon_root.clone()];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_LISTED_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // symlink_metadata: never follow links (escape guard + loop guard).
            let Ok(md) = entry.path().symlink_metadata() else {
                continue;
            };
            if md.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if md.is_dir() {
                if !is_ignored_dir(&name) && !is_internal_dir(&name) {
                    stack.push(path);
                }
                continue;
            }
            if !md.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&canon_root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().to_string();
            let mtime = md.modified().unwrap_or(std::time::UNIX_EPOCH);
            let created_at = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push((
                FileObject {
                    id: encode_file_id(&rel_str),
                    object: "file",
                    bytes: md.len(),
                    created_at,
                    filename: rel_str,
                    purpose: "output".to_string(),
                },
                mtime,
            ));
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    out.into_iter().map(|(f, _)| f).collect()
}

/// Read a workspace file by opaque id, confined to the workspace root. Returns
/// `(bytes, mime)`. Errors carry an HTTP status so the handler can map them.
fn read_workspace_file(file_id: &str) -> Result<(Vec<u8>, String), (u16, String)> {
    let rel = decode_file_id(file_id).ok_or((404, "malformed file id".to_string()))?;
    // Confinement, part one: never serve Fleet-internal or credential dirs, even
    // when they sit inside the workspace root. Checked before touching the disk
    // so the answer can't depend on whether the file happens to exist.
    if is_internal_rel(&rel) {
        return Err((403, "path is not exposed".to_string()));
    }
    let root = std::path::PathBuf::from(public_workspace());
    let canon_root = root
        .canonicalize()
        .map_err(|_| (404, "workspace not found".to_string()))?;
    let joined = canon_root.join(&rel);
    let canon_file = joined
        .canonicalize()
        .map_err(|_| (404, "file not found".to_string()))?;
    // Confinement: the resolved path must stay under the workspace root, and it
    // must be a regular file (not a dir / device).
    if !canon_file.starts_with(&canon_root) {
        return Err((403, "path escapes workspace".to_string()));
    }
    let md = canon_file
        .symlink_metadata()
        .map_err(|_| (404, "file not found".to_string()))?;
    if !md.is_file() {
        return Err((404, "not a regular file".to_string()));
    }
    let bytes = std::fs::read(&canon_file).map_err(|e| (500, format!("read: {e}")))?;
    let mime = crate::wiki::mime_for_path(&canon_file).to_string();
    Ok((bytes, mime))
}

fn file_content(request: tiny_http::Request, file_id: &str, json_header: tiny_http::Header) {
    match read_workspace_file(file_id) {
        Ok((bytes, mime)) => {
            let mime_header: tiny_http::Header =
                format!("Content-Type: {mime}").parse().unwrap();
            let _ = request.respond(tiny_http::Response::from_data(bytes).with_header(mime_header));
        }
        Err((status, msg)) => {
            let code = if status == 403 { "forbidden" } else { "not_found" };
            respond_error(request, status, code, msg, json_header);
        }
    }
}

/// `POST /v1/files` — accept a `multipart/form-data` upload (the shape the
/// OpenAI SDKs send) and land it in the workspace so the agent can read it.
///
/// Fields: `file` (required, the bytes + filename) and `purpose` (optional,
/// echoed back; OpenAI's non-fine-tuning default is `user_data`). The response
/// is a file object whose `id` is the same opaque
/// `file_<base64url(workspace-relative path)>` the artifact routes mint, so
/// `GET /v1/files/{id}` / `.../content` / `DELETE` all work on it for free.

fn upload_file(mut request: tiny_http::Request, json_header: tiny_http::Header) {
    let content_type = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let Some(boundary) = super::multipart::boundary_from_content_type(&content_type) else {
        return respond_error(
            request,
            400,
            "invalid_request",
            "expected multipart/form-data with a boundary",
            json_header,
        );
    };

    // Reject on the declared length first, then re-check after reading: a body
    // may lie about (or omit) Content-Length.
    if let Some(len) = request.body_length() {
        if len as u64 > MAX_UPLOAD_BYTES {
            return respond_error(
                request,
                413,
                "file_too_large",
                format!("upload too large: {len} bytes (max {MAX_UPLOAD_BYTES})"),
                json_header,
            );
        }
    }
    let mut body = Vec::new();
    let mut limited = std::io::Read::take(request.as_reader(), MAX_UPLOAD_BYTES + 1);
    let _ = std::io::Read::read_to_end(&mut limited, &mut body);
    if body.len() as u64 > MAX_UPLOAD_BYTES {
        return respond_error(
            request,
            413,
            "file_too_large",
            format!("upload too large: >{MAX_UPLOAD_BYTES} bytes"),
            json_header,
        );
    }

    let parts = match super::multipart::parse_multipart(&body, &boundary) {
        Ok(p) => p,
        Err(e) => return respond_error(request, 400, "invalid_request", e, json_header),
    };
    let Some(file_part) = parts
        .iter()
        .find(|p| p.name == "file" && p.filename.is_some())
        .or_else(|| parts.iter().find(|p| p.filename.is_some()))
    else {
        return respond_error(
            request,
            400,
            "invalid_request",
            "no file part in multipart body (expected a field named \"file\")",
            json_header,
        );
    };
    let purpose = parts
        .iter()
        .find(|p| p.name == "purpose")
        .map(|p| p.text())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "user_data".to_string());

    let filename = sanitize_upload_filename(file_part.filename.as_deref().unwrap_or(""));
    let dir_name = uuid::Uuid::new_v4().simple().to_string();
    let rel = format!("{UPLOADS_DIR}/{dir_name}/{filename}");
    let root = std::path::PathBuf::from(public_workspace());
    let dest_dir = root.join(UPLOADS_DIR).join(&dir_name);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        return respond_error(request, 500, "internal", format!("create upload dir: {e}"), json_header);
    }
    let dest = dest_dir.join(&filename);
    if let Err(e) = std::fs::write(&dest, &file_part.data) {
        return respond_error(request, 500, "internal", format!("write upload: {e}"), json_header);
    }

    let obj = FileObject {
        id: encode_file_id(&rel),
        object: "file",
        bytes: file_part.data.len() as u64,
        created_at: now_unix(),
        filename,
        purpose,
    };
    respond_value(request, 200, &serde_json::to_value(&obj).unwrap_or_default(), json_header);
}

/// Resolve attachment file ids to absolute paths, applying the same confinement
/// as every other file route. A missing id is a 404 and an out-of-bounds one a
/// 403 — the caller's mistake either way, so it must not reach a spawn.

fn stat_workspace_file(file_id: &str) -> Result<(String, std::path::PathBuf, std::fs::Metadata), (u16, String)> {
    let rel = decode_file_id(file_id).ok_or((404, "malformed file id".to_string()))?;
    if is_internal_rel(&rel) {
        return Err((403, "path is not exposed".to_string()));
    }
    let canon_root = std::path::PathBuf::from(public_workspace())
        .canonicalize()
        .map_err(|_| (404, "workspace not found".to_string()))?;
    let canon_file = canon_root
        .join(&rel)
        .canonicalize()
        .map_err(|_| (404, "file not found".to_string()))?;
    if !canon_file.starts_with(&canon_root) {
        return Err((403, "path escapes workspace".to_string()));
    }
    let md = canon_file
        .symlink_metadata()
        .map_err(|_| (404, "file not found".to_string()))?;
    if !md.is_file() {
        return Err((404, "not a regular file".to_string()));
    }
    Ok((rel, canon_file, md))
}

/// `GET /v1/files/{file_id}` — the file object, no bytes.

fn file_meta(request: tiny_http::Request, file_id: &str, json_header: tiny_http::Header) {
    match stat_workspace_file(file_id) {
        Ok((rel, _, md)) => {
            let obj = FileObject {
                id: file_id.to_string(),
                object: "file",
                bytes: md.len(),
                created_at: md
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                filename: rel.clone(),
                purpose: if is_upload_rel(&rel) { "user_data".to_string() } else { "output".to_string() },
            };
            respond_value(request, 200, &serde_json::to_value(&obj).unwrap_or_default(), json_header);
        }
        Err((status, msg)) => {
            let code = if status == 403 { "forbidden" } else { "not_found" };
            respond_error(request, status, code, msg, json_header);
        }
    }
}

/// `DELETE /v1/files/{file_id}` — remove an upload.
///
/// Confined to [`UPLOADS_DIR`] on purpose: the same id space also names the
/// agent's own output files, and a delete route that reached those would hand a
/// scoped caller a way to destroy the run's results. Uploads are the caller's
/// own bytes, so those it may drop.

fn delete_file(request: tiny_http::Request, file_id: &str, json_header: tiny_http::Header) {
    match stat_workspace_file(file_id) {
        Ok((rel, path, _)) => {
            if !is_upload_rel(&rel) {
                return respond_error(
                    request,
                    403,
                    "forbidden",
                    "only uploaded files can be deleted",
                    json_header,
                );
            }
            if let Err(e) = std::fs::remove_file(&path) {
                return respond_error(request, 500, "internal", format!("delete: {e}"), json_header);
            }
            // Each upload owns its directory; drop it once empty so the uploads
            // tree doesn't fill with husks.
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            let body = serde_json::json!({"id": file_id, "object": "file", "deleted": true});
            respond_value(request, 200, &body, json_header);
        }
        Err((status, msg)) => {
            let code = if status == 403 { "forbidden" } else { "not_found" };
            respond_error(request, status, code, msg, json_header);
        }
    }
}


// ─────────────────────────── Routing ────────────────────────────────

/// Parsed `/v1/...` route target.
#[derive(Debug, PartialEq, Eq)]
pub enum V1Route {
    /// `GET /v1/files` — list the workspace's files.
    ListFiles,
    /// `POST /v1/files` — upload a file the agent can read.
    UploadFile,
    /// `GET /v1/files/{id}` — the file object without its bytes.
    FileMeta(String),
    /// `GET /v1/files/{id}/content` — the bytes.
    FileContent(String),
    /// `DELETE /v1/files/{id}` — drop an upload.
    DeleteFile(String),
    NotFound,
}

/// Route a `/v1/...` request. Pure; unit-tested.
pub fn parse_v1_route(method: &str, path: &str) -> V1Route {
    let Some(rest) = path.strip_prefix("/v1/") else { return V1Route::NotFound };
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        ["files"] if method == "GET" => V1Route::ListFiles,
        ["files"] if method == "POST" => V1Route::UploadFile,
        ["files", id] if method == "GET" => V1Route::FileMeta((*id).to_string()),
        ["files", id] if method == "DELETE" => V1Route::DeleteFile((*id).to_string()),
        ["files", id, "content"] if method == "GET" => V1Route::FileContent((*id).to_string()),
        _ => V1Route::NotFound,
    }
}

/// Entry point for every `/v1/...` request.
pub(crate) fn dispatch(
    _ctx: &super::ServeCtx,
    request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let method = request.method().as_str().to_string();
    match parse_v1_route(&method, path) {
        V1Route::ListFiles => list_files(request, json_header),
        V1Route::UploadFile => upload_file(request, json_header),
        V1Route::FileMeta(id) => file_meta(request, &id, json_header),
        V1Route::FileContent(id) => file_content(request, &id, json_header),
        V1Route::DeleteFile(id) => delete_file(request, &id, json_header),
        V1Route::NotFound => {
            respond_error(request, 404, "not_found", "unknown /v1 route", json_header)
        }
    }
}

/// `GET /v1/files` — everything in the workspace the customer may see.
fn list_files(request: tiny_http::Request, json_header: tiny_http::Header) {
    let body = serde_json::json!({"object": "list", "data": list_workspace_files()});
    respond_value(request, 200, &body, json_header);
}

fn respond_value(
    request: tiny_http::Request,
    status: u16,
    body: &serde_json::Value,
    json_header: tiny_http::Header,
) {
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

fn respond_error(
    request: tiny_http::Request,
    status: u16,
    code: &str,
    message: impl AsRef<str>,
    json_header: tiny_http::Header,
) {
    let body = serde_json::json!({"error": {"code": code, "message": message.as_ref()}});
    respond_value(request, status, &body, json_header);
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_file_routes_remain() {
        assert_eq!(parse_v1_route("GET", "/v1/files"), V1Route::ListFiles);
        assert_eq!(parse_v1_route("POST", "/v1/files"), V1Route::UploadFile);
        assert_eq!(parse_v1_route("GET", "/v1/files/abc"), V1Route::FileMeta("abc".into()));
        assert_eq!(parse_v1_route("DELETE", "/v1/files/abc"), V1Route::DeleteFile("abc".into()));
        assert_eq!(
            parse_v1_route("GET", "/v1/files/abc/content"),
            V1Route::FileContent("abc".into())
        );

        // The agent protocol moved to ACP; these must no longer resolve.
        for (m, p) in [
            ("POST", "/v1/responses"),
            ("GET", "/v1/responses/resp_1"),
            ("POST", "/v1/responses/resp_1/cancel"),
            ("GET", "/v1/responses/resp_1/files"),
        ] {
            assert_eq!(parse_v1_route(m, p), V1Route::NotFound, "{m} {p} must be gone");
        }
        assert_eq!(parse_v1_route("GET", "/health"), V1Route::NotFound);
    }

    #[test]
    fn public_workspace_defaults_and_overrides() {
        let prev = std::env::var("FLEET_PUBLIC_WORKSPACE").ok();
        std::env::remove_var("FLEET_PUBLIC_WORKSPACE");
        assert_eq!(public_workspace(), "/workspace");
        std::env::set_var("FLEET_PUBLIC_WORKSPACE", "/srv/ws");
        assert_eq!(public_workspace(), "/srv/ws");
        // Empty is treated as unset rather than as the root of the filesystem.
        std::env::set_var("FLEET_PUBLIC_WORKSPACE", "");
        assert_eq!(public_workspace(), "/workspace");
        match prev {
            Some(v) => std::env::set_var("FLEET_PUBLIC_WORKSPACE", v),
            None => std::env::remove_var("FLEET_PUBLIC_WORKSPACE"),
        }
    }

    #[test]
    fn file_ids_round_trip_and_cannot_escape() {
        let id = encode_file_id("out/report.pdf");
        assert!(id.starts_with("file_"));
        assert_eq!(decode_file_id(&id).as_deref(), Some("out/report.pdf"));
        // A crafted id that decodes to a traversal is refused by the reader.
        let evil = encode_file_id("../../etc/passwd");
        assert!(read_workspace_file(&evil).is_err());
        assert!(decode_file_id("not-a-file-id").is_none());
    }

    #[test]
    fn only_uploads_are_deletable() {
        assert!(is_upload_rel(&format!("{UPLOADS_DIR}/x/a.png")));
        assert!(!is_upload_rel("src/main.rs"));
        assert!(!is_upload_rel("../outside"));
    }

    #[test]
    fn upload_filenames_are_reduced_to_a_safe_basename() {
        assert_eq!(sanitize_upload_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_upload_filename(""), "upload.bin");
        assert_eq!(sanitize_upload_filename("photo.png"), "photo.png");
    }
}
