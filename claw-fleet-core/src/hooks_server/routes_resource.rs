//! `/resources*` — the machine-global resource queue
//! ([`crate::resource_queue`]): read the table, and the hand adjustments the
//! resource panel makes (reorder, cancel, capacity, kick, remove).
//!
//! Every write answers with the whole table afterwards, so a client redraws
//! from the response instead of racing a second read. A refused write (a
//! request that is not queued, a resource still in use) is a 409 with
//! `{"error": …}` — the message is meant to be shown to the user as is.
use crate::resource_queue::{self as rq, MoveTarget, ResourceSnapshot};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveBody {
    id: String,
    target: MoveTarget,
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

#[derive(Deserialize)]
struct CapacityBody {
    name: String,
    capacity: u32,
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

fn respond(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
    status: u16,
    body: String,
) {
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(json_header),
    );
}

fn answer(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
    result: Result<ResourceSnapshot, String>,
) {
    match result {
        Ok(snap) => {
            let body = serde_json::to_string(&snap).unwrap_or_default();
            respond(request, json_header, 200, body);
        }
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            respond(request, json_header, 409, body);
        }
    }
}

/// Read and parse the JSON body.
fn body<T: serde::de::DeserializeOwned>(
    request: &mut tiny_http::Request,
) -> Result<T, String> {
    let mut bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut bytes);
    serde_json::from_slice(&bytes).map_err(|e| format!("bad request: {e}"))
}

fn with_body<T: serde::de::DeserializeOwned>(
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
    run: impl FnOnce(T) -> Result<ResourceSnapshot, String>,
) {
    match body::<T>(&mut request) {
        Ok(b) => answer(request, json_header, run(b)),
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            respond(request, json_header, 400, body);
        }
    }
}

/// `GET /resources` — the table. A read error (unreadable state file, lock
/// failure) is a 500, not a 409: nothing was refused.
pub(crate) fn route_resources(request: tiny_http::Request, json_header: tiny_http::Header) {
    match rq::snapshot() {
        Ok(snap) => {
            let body = serde_json::to_string(&snap).unwrap_or_default();
            respond(request, json_header, 200, body);
        }
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            respond(request, json_header, 500, body);
        }
    }
}

/// `POST /resources/move` `{id, target: {index: n} | {before: id}}`.
pub(crate) fn route_resources_move(request: tiny_http::Request, json_header: tiny_http::Header) {
    with_body(request, json_header, |b: MoveBody| rq::move_request(&b.id, &b.target));
}

/// `POST /resources/cancel` `{id}` — queued requests only.
pub(crate) fn route_resources_cancel(request: tiny_http::Request, json_header: tiny_http::Header) {
    with_body(request, json_header, |b: IdBody| rq::cancel_request(&b.id));
}

/// `POST /resources/capacity` `{name, capacity}`.
pub(crate) fn route_resources_capacity(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    with_body(request, json_header, |b: CapacityBody| rq::set_capacity(&b.name, b.capacity));
}

/// `POST /resources/kick` `{id}` — evict a holder. Blocks this worker for up
/// to twice [`rq::KICK_GRACE`] while the holder lets go; the server has a
/// worker pool, so other requests keep flowing.
pub(crate) fn route_resources_kick(request: tiny_http::Request, json_header: tiny_http::Header) {
    with_body(request, json_header, |b: IdBody| rq::kick(&b.id));
}

/// `POST /resources/remove` `{name}` — idle resources only.
pub(crate) fn route_resources_remove(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    with_body(request, json_header, |b: NameBody| rq::remove_resource(&b.name));
}
