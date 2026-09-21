//! Side questions about a session's transcript (`crate::session_explain`).
//!
//! Three operations on one record store, exposed the way the desktop's Tauri
//! commands and the mobile relay expose them:
//!
//! - `POST /session_explain` with an [`crate::session_explain::ExplainRequest`]
//!   body accepts the question and returns the `running` record at once;
//! - `GET /session_explain?session_id=…&id=…` returns that record as it stands
//!   (clients poll this until `status` leaves `running`);
//! - `GET /session_explains?session_id=…` lists every record of the session,
//!   oldest first.
use super::*;

/// `POST /session_explain` (ask) or `GET /session_explain?session_id=…&id=…`
/// (one record). A GET for an unknown id is a 404, not an empty body, so a
/// poller can tell "not yet written" from "gone".
pub(crate) fn route_session_explain(
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    if *request.method() == tiny_http::Method::Post {
        let mut body_bytes = Vec::new();
        let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
        let parsed = serde_json::from_slice::<crate::session_explain::ExplainRequest>(&body_bytes)
            .map_err(|e| format!("bad request body: {e}"));
        let result = parsed.and_then(crate::session_explain::ask);
        respond_explain_json(request, json_header, result);
        return;
    }
    let session_id = decoded(query, "session_id");
    let id = decoded(query, "id");
    match crate::session_explain::get(&session_id, &id) {
        Some(rec) => respond_explain_json(request, json_header, Ok::<_, String>(rec)),
        None => {
            let body = serde_json::json!({ "error": "no such explanation" }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(404)
                    .with_header(json_header),
            );
        }
    }
}

/// `GET /session_explains?session_id=…` — every record of the session.
pub(crate) fn route_session_explains(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let session_id = decoded(query, "session_id");
    respond_explain_json(
        request,
        json_header,
        Ok::<_, String>(crate::session_explain::list(&session_id)),
    );
}

fn decoded(query: &std::collections::HashMap<String, String>, key: &str) -> String {
    let raw = query.get(key).map(|s| s.as_str()).unwrap_or("");
    percent_decode_str(raw).decode_utf8_lossy().to_string()
}

/// Serialised body → 200; a validation error → 400 with `{"error": …}`, the
/// shape the frontend's invoke shim rejects on.
fn respond_explain_json<T: serde::Serialize>(
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
