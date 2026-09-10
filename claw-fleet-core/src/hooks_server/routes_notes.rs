//! Session notes — the read side of `~/.fleet/notes/<session>/`.
//!
//! The write side belongs to the agent alone (the `fleet__notes` MCP tool and
//! the `fleet notes` CLI), so these two routes are deliberately read-only: a
//! reader browsing a session's checkpoint notes must never be able to edit what
//! the agent recorded, or the notes stop being an account of what the run
//! actually knew.
use super::*;

/// `GET /session_notes?session_id=…` — note files visible to that session (its
/// own, then its handoff predecessors'), newest-updated first within each.
pub(crate) fn route_session_notes(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let session_id = decoded(query, "session_id");
    respond_notes_json(
        request,
        json_header,
        crate::session_notes::list(&session_id, None),
    );
}

/// `GET /session_note?session_id=…&path=…` — one note's full text.
///
/// `session_id` is the file's **owner**, as listed by `/session_notes`, not the
/// session being viewed: the two differ for every note inherited from a handoff
/// predecessor, and resolving along the chain here would show the viewer's own
/// `checkpoint.md` in place of the predecessor's.
pub(crate) fn route_session_note(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let session_id = decoded(query, "session_id");
    let path = decoded(query, "path");
    respond_notes_json(
        request,
        json_header,
        crate::session_notes::read_owned(&session_id, &path),
    );
}

/// `GET /session_notes_search?session_id=…&q=…` — literal, case-sensitive
/// substring hits across everything the session can read.
pub(crate) fn route_session_notes_search(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let session_id = decoded(query, "session_id");
    let q = decoded(query, "q");
    respond_notes_json(
        request,
        json_header,
        crate::session_notes::search(
            &session_id,
            &q,
            None,
            crate::session_notes::SEARCH_MAX_FILES,
            crate::session_notes::SEARCH_MAX_MATCHES_PER_FILE,
        ),
    );
}

fn decoded(query: &std::collections::HashMap<String, String>, key: &str) -> String {
    let raw = query.get(key).map(|s| s.as_str()).unwrap_or("");
    percent_decode_str(raw).decode_utf8_lossy().to_string()
}

/// Serialised body → 200; a validation / missing-file error → 400 with
/// `{"error": …}`, which is the shape the frontend's invoke shim rejects on.
fn respond_notes_json<T: serde::Serialize>(
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
