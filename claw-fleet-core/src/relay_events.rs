//! Live events for the desktop web bundle opened from a relay origin.
//!
//! The desktop UI is driven by a fixed set of named events — `sessions-frame`
//! deltas plus the decision-card lifecycle (`guard-request`, …). The desktop
//! webview gets them as Tauri events; a browser on `fleet serve` gets them as
//! SSE from `/events`, under the very same names and payloads. A browser that
//! reaches this host over the relay (`relay_http`) has neither: the tunnel is
//! one request, one reply. This module carries those same events over the
//! relay socket as unsolicited `msg` payloads:
//!
//! ```json
//! { "event": "desktop_event", "name": "sessions-frame", "data": { … } }
//! ```
//!
//! **Sources.** Nothing here produces events; it taps the two places that
//! already do, so the stream is exactly what the desktop UI is built against:
//!
//! - the desktop app forwards its own Tauri emits ([`publish`] from a
//!   `listen_any` per name), and registers its session tracker as the answer to
//!   `/sessions_snapshot` ([`set_sessions_snapshot_provider`]) so the resync
//!   endpoint and the deltas share one seq space;
//! - `fleet serve` / `fleet webui` forward every `SseBroadcaster::broadcast`;
//!   their `/sessions_snapshot` route already shares the broadcaster's tracker.
//!
//! **Subscription.** The relay hands every agent frame to every client in the
//! channel, phones included, and the session deltas here are full desktop rows,
//! not the phone's slim ones. So nothing is sent unless a desktop tab holds a
//! lease: it calls the `desktop_events` relay method on connect and renews it
//! well inside [`LEASE`]. A tab that goes away stops costing anyone bandwidth
//! within one lease; a phone, which never subscribes, only pays while a desktop
//! tab is actually open, and drops these frames unread (unknown `event`).

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::session_delta::SessionsFrame;

/// The events forwarded, by name. Mirrors `FORWARDED_SSE_EVENTS` in the desktop
/// front end's `app/mock/liveProxy.ts`, which is what consumes them; anything
/// else broadcast on SSE (`waiting-alert`, …) is not consumed there and would
/// only cost bandwidth.
pub const FORWARDED_EVENTS: &[&str] = &[
    "guard-request",
    "guard-dismissed",
    "elicitation-request",
    "elicitation-dismissed",
    "fleet-ask-request",
    "fleet-ask-dismissed",
    "a2ui-render-request",
    "a2ui-render-dismissed",
    "plan-approval-request",
    "plan-approval-dismissed",
    "permission-prompt-request",
    "permission-prompt-dismissed",
    "decision-parked",
    "sessions-frame",
];

/// How long one subscription lasts without renewal. The tab renews every
/// 30s, so a lost renewal or two is tolerated before the stream stops.
pub const LEASE: Duration = Duration::from_secs(90);

struct Lease {
    until: Instant,
    /// The subscriber can inflate a `z:true` payload.
    gzip: bool,
}

static LEASE_STATE: Mutex<Option<Lease>> = Mutex::new(None);

type SnapshotProvider = Box<dyn Fn() -> SessionsFrame + Send + Sync>;
static SNAPSHOT_PROVIDER: OnceLock<SnapshotProvider> = OnceLock::new();

/// The `desktop_events` relay method: start or renew the lease.
///
/// Params: `{ supportsGzip?: bool }`. Answers `{ leaseMs, events }` so the tab
/// knows how often to renew and which names will arrive.
pub fn serve_subscribe(params: &Value) -> Result<Value, String> {
    let gzip = params
        .get("supportsGzip")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    *LEASE_STATE.lock().unwrap() = Some(Lease {
        until: Instant::now() + LEASE,
        gzip,
    });
    Ok(json!({
        "leaseMs": LEASE.as_millis() as u64,
        "events": FORWARDED_EVENTS,
    }))
}

/// Whether a desktop tab currently holds a lease, and if so whether it takes
/// gzip. `None` when nobody is subscribed (or the lease ran out).
fn live_lease() -> Option<bool> {
    let mut state = LEASE_STATE.lock().unwrap();
    match state.as_ref() {
        Some(l) if l.until > Instant::now() => Some(l.gzip),
        Some(_) => {
            *state = None;
            None
        }
        None => None,
    }
}

/// The payload pushed for one event. `data` is the event's JSON payload, or
/// the raw string when it does not parse (the SSE consumer does the same).
pub fn build_event_payload(name: &str, data: &str) -> Value {
    let data = serde_json::from_str::<Value>(data).unwrap_or_else(|_| Value::String(data.into()));
    json!({ "event": "desktop_event", "name": name, "data": data })
}

/// Forward one event to the subscribed desktop tab, if any. `data` is the
/// event's payload already serialized as JSON — what both SSE and Tauri carry.
/// Cheap when nobody is subscribed: a name check and a mutex read.
pub fn publish(name: &str, data: &str) {
    if !FORWARDED_EVENTS.contains(&name) {
        return;
    }
    let Some(gzip) = live_lease() else {
        return;
    };
    if !crate::mobile_relay::is_connected() {
        return;
    }
    crate::mobile_relay::publish_desktop_event(&build_event_payload(name, data), gzip);
}

/// Register this process's session tracker as the answer to a tunnelled
/// `/sessions_snapshot`. Set by the desktop app, whose deltas ([`publish`]ed
/// from its Tauri emits) are numbered by its own tracker rather than by the
/// embedded request server's — answering the resync from anywhere else would
/// hand the tab a seq its next delta does not follow, and it would resync on
/// every frame. First registration wins.
pub fn set_sessions_snapshot_provider(f: impl Fn() -> SessionsFrame + Send + Sync + 'static) {
    let _ = SNAPSHOT_PROVIDER.set(Box::new(f));
}

/// The registered provider's current frame, if one is registered.
pub(crate) fn local_sessions_snapshot() -> Option<SessionsFrame> {
    SNAPSHOT_PROVIDER.get().map(|f| f())
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    *LEASE_STATE.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test touches the process-global lease, so they are folded together
    // rather than racing each other on it.
    #[test]
    fn lease_gates_publishing_and_reports_gzip() {
        reset_for_test();
        assert_eq!(live_lease(), None, "no lease before a subscribe");
        let reply = serve_subscribe(&json!({ "supportsGzip": true })).unwrap();
        assert_eq!(reply["leaseMs"], LEASE.as_millis() as u64);
        assert!(reply["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "sessions-frame"));
        assert_eq!(live_lease(), Some(true));

        serve_subscribe(&json!({})).unwrap();
        assert_eq!(live_lease(), Some(false), "a renewal replaces the gzip flag");

        // Expired lease is dropped on read.
        LEASE_STATE.lock().unwrap().as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
        assert_eq!(live_lease(), None);
        assert!(LEASE_STATE.lock().unwrap().is_none());
        reset_for_test();
    }

    #[test]
    fn payload_keeps_json_and_falls_back_to_string() {
        let p = build_event_payload("sessions-frame", r#"{"kind":"delta","seq":3}"#);
        assert_eq!(p["event"], "desktop_event");
        assert_eq!(p["name"], "sessions-frame");
        assert_eq!(p["data"]["seq"], 3);
        let p = build_event_payload("decision-parked", r#""abc""#);
        assert_eq!(p["data"], "abc");
        let p = build_event_payload("decision-parked", "not json");
        assert_eq!(p["data"], "not json");
    }

    #[test]
    fn forwarded_names_are_unique() {
        let mut names = FORWARDED_EVENTS.to_vec();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), FORWARDED_EVENTS.len());
    }
}
