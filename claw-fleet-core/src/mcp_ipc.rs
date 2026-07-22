//! IPC protocol between the `fleet mcp` server (a Claude Code child process)
//! and the local `fleet` process (Fleet desktop or `fleet serve`).
//!
//! Wire format: file-based IPC at `~/.fleet/fleet-ask/<id>.json` (request)
//! and `~/.fleet/fleet-ask/<id>.response.json` (response). Mirrors the
//! existing `elicitation` / `guard` / `plan_approval` Decision Card patterns
//! so all four channels can share the same watcher / cleanup / orphan-filter
//! logic and the desktop polls a single style of directory.
//!
//! These types are the single source of truth for the `fleet__ask` schema
//! advertised over MCP; [`fleet_ask_input_schema`] mirrors them in JSONSchema
//! form for the MCP `tools/list` response.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FleetAskRequest {
    pub id: String,
    /// Originating Claude Code session id (passed via env var
    /// `CLAUDE_CODE_SESSION_ID` when the MCP server is launched). Used by the
    /// desktop watcher to resolve workspace + AI title for the Decision
    /// Card header, exactly like `ElicitationRequest::session_id`.
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default)]
    pub timestamp: String,
    /// True once the card has been through [`crate::parked`]: the wait timed
    /// out, the turn that asked was interrupted, and the question is now sitting
    /// in `~/.fleet/parked/` until the user gets to it. Answering a parked card
    /// resumes the session instead of unblocking a (long-gone) MCP poll.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
    pub questions: Vec<FleetAskQuestion>,
    /// Documents the agent wants the user to review alongside the card — the
    /// `.md` files or wiki entries it just produced, so the user can read them
    /// in-panel instead of hunting them down. Card-level (not per-question):
    /// the desktop renders one tab per doc in the card's side column. Unlike
    /// `images`, the body is NOT snapshotted here; the frontend fetches it live
    /// through the backend when the card renders (see `ReviewDoc`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_docs: Vec<ReviewDoc>,
}

/// One document attached to a `fleet__ask` card for the user to review.
/// Rendered as a tab in the card's side panel; the body is fetched live by the
/// frontend (via the backend `read_review_doc`) rather than snapshotted into the
/// request, so the user always sees the current on-disk content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReviewDoc {
    pub kind: ReviewDocKind,
    /// For `wiki`: the published wiki slug. For `file`: an absolute path on the
    /// agent host. Relative paths the agent supplies are resolved against
    /// `CLAUDE_PROJECT_DIR` (its cwd) at write time by [`resolve_review_docs`],
    /// so this is always absolute by the time it reaches the frontend.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Tab label. Falls back to the slug / file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ReviewDocKind {
    /// A wiki doc, addressed by slug (`fleet wiki publish` output).
    Wiki,
    /// A file on disk, addressed by path relative to the agent cwd (or absolute).
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FleetAskQuestion {
    pub question: String,
    pub header: String,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FleetAskOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub form_fields: Vec<FleetAskFormField>,
    /// Local image files the agent wants shown WITHOUT base64-inlining them
    /// into `html`. Each entry names a file on the agent's host; on the way in
    /// [`ingest_images`] copies it once into Fleet's persistent decision-asset
    /// store (`~/.fleet/decision-assets/<id>/q<idx>/<name>`) and blanks the
    /// `path`. The card then loads a served `index.html` (the `html` field, or
    /// a synthesized gallery when `html` is absent) through the
    /// `fleet-decision://` protocol, so `<img src="<name>">` resolves without a
    /// single base64 byte crossing the tool-call boundary — and the copies
    /// survive for later decision-history review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<FleetAskImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FleetAskImage {
    /// Filename the image is served as inside the question's asset dir; also
    /// the string the agent references in `html` (e.g. `<img src="chart.png">`).
    /// Must be a bare filename — no path separators, no `..`.
    pub name: String,
    /// Absolute (or agent-cwd-relative) path to the source file. Consumed by
    /// [`ingest_images`] and blanked afterwards, so it never lands in the
    /// persisted request or decision history.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Optional caption rendered beneath the image in the synthesized gallery
    /// (ignored when the agent supplies its own `html`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Raw bytes + mime for a decision-asset file, served into the webview through
/// the `fleet-decision://` protocol or over the probe HTTP API. Mirrors
/// [`crate::wiki::WikiFileBytes`].
#[derive(Clone, Debug)]
pub struct DecisionAssetBytes {
    pub bytes: Vec<u8>,
    pub mime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct FleetAskOption {
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FleetAskFormField {
    pub name: String,
    pub kind: FormFieldKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(type = "unknown"))]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(rename = "FleetAskFormFieldKind"))]
#[serde(rename_all = "lowercase")]
pub enum FormFieldKind {
    Text,
    Textarea,
    Number,
    Select,
    Radio,
    Checkbox,
    Date,
    Datetime,
    Time,
    Range,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAskResponse {
    pub id: String,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    #[serde(default)]
    pub cancelled: bool,
}

// ── File-based IPC (mirror of `elicitation` module) ──────────────────────────

fn fleet_ask_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-ask"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    fleet_ask_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    fleet_ask_dir().map(|d| d.join(format!("{id}.response.json")))
}

/// Write a fleet_ask request file. Called by the MCP server when it handles
/// a `tools/call` for `fleet__ask`.
pub fn write_request(req: &FleetAskRequest) -> Result<(), String> {
    let dir = fleet_ask_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create fleet-ask dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-ask request: {e}"))
}

/// Read a pending request file. Called by the desktop watcher when it
/// notices a new id in the directory listing.
pub fn read_request(id: &str) -> Option<FleetAskRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Non-blocking response read.
pub fn try_read_response(id: &str) -> Option<FleetAskResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<FleetAskResponse>(&content).ok()
}

/// Blocking poll for a response. Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<FleetAskResponse> {
    let start = std::time::Instant::now();
    let interval = Duration::from_millis(200);
    loop {
        if let Some(r) = try_read_response(id) {
            return Some(r);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(interval);
    }
}

/// Write a response file. Called by the desktop / fleet serve after the
/// user resolves the Decision Card.
///
/// Normally the blocked MCP producer consumes the file within its 200ms poll
/// loop. Codex can, however, end the outer code-mode `exec` while the nested
/// MCP call is still waiting. In that state the request survives but its
/// producer and turn are gone. Detect that exact condition synchronously after
/// writing and resume the Fleet-owned session with the answer instead of
/// leaving an unconsumed response forever.
pub fn write_response(resp: &FleetAskResponse) -> Result<(), String> {
    // Only a live pending request may be answered — see guard::write_response.
    if !request_path(&resp.id).map(|p| p.exists()).unwrap_or(false) {
        return Err(format!("no pending request for id {}", resp.id));
    }
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-ask response: {e}"))?;
    recover_orphaned_response(&resp.id).map(|_| ())
}

/// Recover an answered request whose MCP producer no longer exists.
///
/// `Ok(false)` means the live producer still owns the response (or this was
/// not a Fleet-owned session). `Ok(true)` means the orphan was consumed here:
/// cancellation cleans it up, while a real answer resumes the session through
/// the source-aware parked path.
pub fn recover_orphaned_response(id: &str) -> Result<bool, String> {
    recover_orphaned_response_with(
        id,
        crate::parked::session_alive,
        |session_id, workspace, prompt, model, effort, permission_mode| {
            crate::parked::resume_session(
                session_id,
                workspace,
                prompt,
                model,
                effort,
                permission_mode,
            )
        },
    )
}

fn recover_orphaned_response_with<A, S>(id: &str, alive: A, spawn: S) -> Result<bool, String>
where
    A: FnOnce(&str) -> bool,
    S: FnOnce(&str, &str, &str, Option<&str>, Option<&str>, Option<&str>) -> Result<(), String>,
{
    let Some(req) = read_request(id) else {
        return Ok(false);
    };
    let Some(resp) = try_read_response(id) else {
        return Ok(false);
    };

    // This is the load-bearing duplicate-resume gate. A live producer owns the
    // response even if it has not reached its next 200ms poll yet.
    if alive(&req.session_id) {
        return Ok(false);
    }
    let Some(workspace) = crate::parked::parkable_workspace(&req.session_id) else {
        return Ok(false);
    };

    let outcome = if resp.cancelled {
        crate::decision_history::FleetAskOutcome::Cancelled
    } else {
        crate::decision_history::FleetAskOutcome::Answered
    };

    if resp.cancelled {
        cleanup(id);
    } else {
        let model = crate::session::resolve_session_model_spec(&req.session_id);
        let effort = crate::launch_spec::effort_of(&req.session_id);
        crate::parked::park_with(
            id,
            crate::parked::ParkedKind::FleetAsk,
            &req.session_id,
            &workspace,
            &req,
            model.as_deref(),
            effort.as_deref(),
        )?;
        // From here the parked copy is authoritative. Removing the live pair
        // prevents the stale response from being rediscovered after restart.
        cleanup(id);
        let payload = serde_json::to_value(&resp)
            .map_err(|e| format!("serialize orphaned fleet-ask response: {e}"))?;
        crate::parked::answer_with(id, &payload, spawn)?;
    }

    let answers = if resp.cancelled {
        BTreeMap::new()
    } else {
        resp.answers.clone()
    };
    let record = crate::decision_history::build_fleet_ask_record(
        &req,
        outcome,
        answers,
        chrono::Utc::now().to_rfc3339(),
    );
    if let Err(e) = crate::decision_history::append_record(
        &crate::decision_history::DecisionHistoryRecord::FleetAsk(record),
    ) {
        crate::log_debug(&format!(
            "fleet-ask orphan recovery: append history for {id}: {e}"
        ));
    }
    crate::log_debug(&format!(
        "fleet-ask orphan recovery: consumed {id} for session {} (cancelled={})",
        req.session_id, resp.cancelled
    ));
    Ok(true)
}

/// Remove the request + response pair. Called by the MCP server once it has
/// consumed the response.
pub fn cleanup(id: &str) {
    if let Some(p) = request_path(id) {
        let _ = fs::remove_file(p);
    }
    if let Some(p) = response_path(id) {
        let _ = fs::remove_file(p);
    }
}

/// Soft list of pending request ids; returns empty on any failure.
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict list — distinguishes "no requests / missing dir" (`Ok(vec![])`)
/// from "I/O error reading the directory" (`Err`). The watcher uses the
/// `Err` to skip the dismissal-emit step so a transient APFS hiccup
/// doesn't take every active panel down with it.
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = fleet_ask_dir() else {
        return Ok(Vec::new());
    };
    list_pending_in_dir(&dir)
}

fn list_pending_in_dir(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut request_ids: Vec<String> = Vec::new();
    let mut response_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".response.json") {
            response_ids.insert(id.to_string());
        } else if let Some(id) = name.strip_suffix(".json") {
            request_ids.push(id.to_string());
        }
    }
    Ok(request_ids
        .into_iter()
        .filter(|id| !response_ids.contains(id))
        .collect())
}

// ── Decision-asset store (fleet-decision:// protocol backing) ────────────────
//
// Lives at `~/.fleet/decision-assets/<id>/q<idx>/`. Each question that carries
// `images` gets its own subdir holding an `index.html` (the question's `html`,
// or a synthesized gallery) plus one file per image. The desktop loads
// `fleet-decision://localhost/<id>/q<idx>/index.html` into the card's iframe;
// relative `<img src="name">` references resolve to sibling files. The dir is
// deliberately NOT removed by `cleanup` — it persists so DecisionHistory can
// re-render the exact card the user saw.

/// `~/.fleet/decision-assets` (None when the home dir can't be determined).
pub fn decision_assets_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("decision-assets"))
}

fn question_asset_dir(id: &str, qidx: usize) -> Option<PathBuf> {
    decision_assets_dir().map(|d| d.join(id).join(format!("q{qidx}")))
}

/// Reject anything that isn't a bare filename (path separators, `..`, absolute).
fn valid_asset_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !Path::new(name).is_absolute()
}

fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn html_escape_attr(s: &str) -> String {
    html_escape_text(s).replace('"', "&quot;")
}

/// Marker the injected script carries; doubles as the idempotency probe so a
/// document already carrying the script is never given a second copy.
pub const AUTOHEIGHT_MARKER: &str = "__fleetAskHeight";

/// Script appended to every document a decision card renders in its iframe.
///
/// The card's iframe is `sandbox="allow-scripts"` **without** `allow-same-origin`,
/// so the document stays on an opaque origin: this script cannot read the app's
/// DOM, cookies or storage — the only thing it can do is `postMessage` its own
/// height up. That's the whole point: an iframe has the 300x150 intrinsic size of
/// a replaced element and no auto-sizing, so without a measurement handshake the
/// card has to hard-code a height and every taller preview renders squashed into
/// an inner scroll box. Cross-origin, the parent cannot read `contentDocument`,
/// so the measurement has to originate inside.
///
/// The `last` guard keeps the child from re-posting an unchanged height, and the
/// parent applies a clamp + dead-band (see `decisionFrame.ts`), so a document
/// whose own layout depends on the viewport (`100vh`, `height: 100%`) settles
/// instead of oscillating.
pub const AUTOHEIGHT_SCRIPT: &str = "\n<script>(function(){var last=0;function send(){var d=document.documentElement,b=document.body;\
var h=Math.ceil(Math.max(d.scrollHeight,b?b.scrollHeight:0,d.getBoundingClientRect().height));\
if(h&&h!==last){last=h;parent.postMessage({__fleetAskHeight:h},'*');}}\
addEventListener('load',send);addEventListener('resize',send);\
if(window.ResizeObserver){new ResizeObserver(send).observe(document.documentElement);}\
setTimeout(send,0);setTimeout(send,120);setTimeout(send,400);})();</script>";

/// Append [`AUTOHEIGHT_SCRIPT`] unless the document already carries it.
pub fn with_autoheight(html: &str) -> String {
    if html.contains(AUTOHEIGHT_MARKER) {
        return html.to_string();
    }
    format!("{html}{AUTOHEIGHT_SCRIPT}")
}

/// Whether a document has anything at all to paint.
///
/// Only whitespace and comments survive removal, so a `true` here means the
/// iframe would render a blank rectangle — no heuristics, no guessing at what
/// "looks empty": markup that produces no boxes (`<img>`, `<hr>`, `<canvas>`)
/// still counts as content and is left alone.
fn html_renders_nothing(html: &str) -> bool {
    let mut rest = html;
    loop {
        let Some(open) = rest.find("<!--") else {
            return rest.trim().is_empty();
        };
        if !rest[..open].trim().is_empty() {
            return false;
        }
        // An unterminated comment swallows the remainder of the document, which
        // is also how a browser parses it.
        match rest[open + 4..].find("-->") {
            Some(close) => rest = &rest[open + 4 + close + 3..],
            None => return true,
        }
    }
}

/// Normalize every question's `html` preview before it is staged anywhere.
///
/// Two passes, in order:
///
/// 1. **Drop content-free previews.** An agent that stubs the field with a
///    placeholder (seen in the wild: `"html": "<!--HTML-->"`) hands the card a
///    truthy string that paints an empty 200px sandbox iframe across the
///    question body. `None` is what "no preview" means; normalize to it here so
///    every consumer — desktop card, decision history, mobile — is spared the
///    blank frame, and [`ingest_images`] falls back to its synthesized gallery.
/// 2. **Inject the height-reporting script into the rest**, so both iframe paths
///    carry it: the `srcDoc` one (html, no images) reads `q.html` straight from
///    this request, and the served one (images) writes the same string into
///    `index.html` in [`ingest_images`]. Doing it here — once, at request time —
///    keeps a single copy of the script in the tree instead of one per path.
pub fn normalize_html(req: &mut FleetAskRequest) {
    for q in &mut req.questions {
        let Some(h) = &q.html else { continue };
        q.html = if html_renders_nothing(h) {
            None
        } else {
            Some(with_autoheight(h))
        };
    }
}

/// Resolve every `file` review-doc reference to an absolute path so the desktop
/// (a different process, possibly a different cwd) can read it back live.
///
/// Runs inside the `fleet mcp` child — the same host and cwd as the agent — so
/// a relative `ref` is joined against `project_dir` (the agent's
/// `CLAUDE_PROJECT_DIR`), then the whole thing is canonicalized. Best-effort:
/// a ref that can't be canonicalized (file since moved, bad path) is left as the
/// joined absolute path so the card can still show a "couldn't read" tab rather
/// than silently dropping it. `wiki` refs are slugs, not paths — left untouched.
pub fn resolve_review_docs(req: &mut FleetAskRequest, project_dir: Option<&Path>) {
    for doc in &mut req.review_docs {
        if doc.kind != ReviewDocKind::File {
            continue;
        }
        let raw = Path::new(&doc.reference);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else if let Some(base) = project_dir {
            base.join(raw)
        } else {
            raw.to_path_buf()
        };
        let abs = joined.canonicalize().unwrap_or(joined);
        doc.reference = abs.to_string_lossy().into_owned();
    }
}

/// Build a minimal stacked-image gallery document for questions that ship
/// `images` but no `html` of their own.
fn synthesize_gallery(images: &[FleetAskImage]) -> String {
    let mut body = String::from(
        "<!doctype html><meta charset=\"utf-8\"><style>\
         body{margin:0;padding:8px;font:13px/1.5 system-ui,-apple-system,sans-serif;background:#fff;color:#222}\
         figure{margin:0 0 12px}img{max-width:100%;height:auto;display:block;border-radius:6px}\
         figcaption{margin-top:4px;color:#666;font-size:12px}</style>",
    );
    for img in images {
        body.push_str("<figure><img src=\"");
        body.push_str(&html_escape_attr(&img.name));
        body.push_str("\" alt=\"");
        body.push_str(&html_escape_attr(img.caption.as_deref().unwrap_or(&img.name)));
        body.push_str("\">");
        if let Some(cap) = &img.caption {
            body.push_str("<figcaption>");
            body.push_str(&html_escape_text(cap));
            body.push_str("</figcaption>");
        }
        body.push_str("</figure>");
    }
    body
}

/// Copy every referenced image into the persistent decision-asset store and
/// materialize a served `index.html` per image-bearing question. Mutates `req`
/// in place: blanks each surviving image `path` (so no absolute local path
/// leaks into the persisted request / history) and keeps `name` + `caption`.
///
/// Runs inside the `fleet mcp` child — the same host as the agent's files — so
/// the copy is a plain local `fs::copy` regardless of Local vs Remote backend.
/// Best-effort per image: an unsafe name or missing/unreadable source is
/// dropped from the question rather than failing the whole ask.
pub fn ingest_images(req: &mut FleetAskRequest) -> Result<(), String> {
    let id = req.id.clone();
    for (qidx, q) in req.questions.iter_mut().enumerate() {
        if q.images.is_empty() {
            continue;
        }
        let dir = question_asset_dir(&id, qidx).ok_or("cannot determine home dir")?;
        fs::create_dir_all(&dir).map_err(|e| format!("create decision-asset dir: {e}"))?;

        let mut kept: Vec<FleetAskImage> = Vec::with_capacity(q.images.len());
        for img in std::mem::take(&mut q.images) {
            if !valid_asset_name(&img.name) {
                continue;
            }
            let dest = dir.join(&img.name);
            if fs::copy(PathBuf::from(&img.path), &dest).is_ok() {
                kept.push(FleetAskImage {
                    name: img.name,
                    path: String::new(),
                    caption: img.caption,
                });
            }
        }
        q.images = kept;

        // Materialize the served entry document: the agent's own html when it
        // provided one (referencing images relatively), else a gallery.
        let index = dir.join("index.html");
        let html = match &q.html {
            Some(h) if !h.trim().is_empty() => with_autoheight(h),
            _ => with_autoheight(&synthesize_gallery(&q.images)),
        };
        fs::write(&index, html).map_err(|e| format!("write decision-asset index.html: {e}"))?;
    }
    Ok(())
}

/// Read one file from a question's decision-asset dir, with the same
/// path-traversal defense as [`crate::wiki::get_file_in`]. `rel` is typically
/// `index.html` or a bare image name.
pub fn read_decision_asset(id: &str, qidx: &str, rel: &str) -> Result<DecisionAssetBytes, String> {
    let base = decision_assets_dir().ok_or("cannot determine home dir")?;
    for seg in [id, qidx] {
        if seg.is_empty() || seg.contains('/') || seg.contains('\\') || seg.contains("..") {
            return Err("invalid decision-asset id/qidx".to_string());
        }
    }
    let dir = base.join(id).join(qidx);

    let relp = Path::new(rel);
    if relp.is_absolute()
        || rel.contains('\\')
        || relp.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
            )
        })
    {
        return Err(format!("invalid path '{rel}'"));
    }

    let joined = dir.join(relp);
    let canon_dir = dir
        .canonicalize()
        .map_err(|_| "decision-asset dir not found".to_string())?;
    let canon_file = joined
        .canonicalize()
        .map_err(|_| format!("file '{rel}' not found"))?;
    if !canon_file.starts_with(&canon_dir) {
        return Err(format!("invalid path '{rel}'"));
    }

    let bytes = fs::read(&canon_file).map_err(|e| format!("read '{rel}': {e}"))?;
    Ok(DecisionAssetBytes {
        bytes,
        mime: crate::wiki::mime_for_path(&canon_file).to_string(),
    })
}

/// JSONSchema describing `fleet__ask` arguments. The MCP `tools/list` response
/// embeds this verbatim so the agent sees the same field shape the Rust types
/// would deserialize.
pub fn fleet_ask_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "header": { "type": "string", "maxLength": 12 },
                        "multiSelect": { "type": "boolean" },
                        "options": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 4,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string" },
                                    "description": { "type": "string" },
                                    "preview": { "type": "string" }
                                },
                                "required": ["label", "description"]
                            }
                        },
                        "html": {
                            "type": "string",
                            "description": "OPTIONAL HTML preview body; rendered in a sandboxed iframe (no scripts, no same-origin). Omit the field entirely when the card has no preview to show — never pass a placeholder or a comment-only stub like \"<!--HTML-->\", which paints an empty box over the question. To show images, DO NOT base64-inline them here — that burns output tokens. Instead list the files in `images` and reference them by their `name` with a relative path, e.g. <img src=\"chart.png\">."
                        },
                        "images": {
                            "type": "array",
                            "description": "Local image files to display WITHOUT base64-inlining them into `html`. Fleet copies each file once into its persistent decision-asset store and serves it to the card; reference them from `html` by `name` (e.g. <img src=\"chart.png\">). If you omit `html`, Fleet renders the images as a simple captioned gallery. Prefer this over data:/base64 URLs for every image.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "Bare filename the image is served as and referenced by in html (no path separators, no ..)" },
                                    "path": { "type": "string", "description": "Absolute path (or path relative to the agent's cwd) to the source file on the agent's host" },
                                    "caption": { "type": "string", "description": "Optional caption shown under the image in the auto gallery (ignored when you supply your own html)" }
                                },
                                "required": ["name", "path"]
                            }
                        },
                        "formFields": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "kind": {
                                        "type": "string",
                                        "enum": ["text", "textarea", "number", "select", "radio", "checkbox", "date", "datetime", "time", "range"]
                                    },
                                    "label": { "type": "string" },
                                    "placeholder": { "type": "string" },
                                    "options": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    },
                                    "required": { "type": "boolean" },
                                    "default": {},
                                    "min": { "type": "number", "description": "Range slider lower bound (range kind only)" },
                                    "max": { "type": "number", "description": "Range slider upper bound (range kind only)" },
                                    "step": { "type": "number", "description": "Range slider step (range kind only)" }
                                },
                                "required": ["name", "kind", "label"]
                            }
                        }
                    },
                    "required": ["question", "header", "multiSelect"]
                }
            },
            "reviewDocs": {
                "type": "array",
                "description": "OPTIONAL documents for the user to review alongside this card — the `.md` files or wiki entries you just produced. Fleet shows one tab per doc in a side panel next to the card, so the user reads them in place instead of hunting the path down in your prose. Card-level, not per-question. The body is fetched live when the card opens (not snapshotted), so the user always sees the current on-disk content. Use this whenever your answer references a design doc / report / plan file you want signed off.",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["wiki", "file"],
                            "description": "`wiki` = a published wiki doc (ref is its slug); `file` = a file on disk (ref is its path)."
                        },
                        "ref": {
                            "type": "string",
                            "description": "For kind=wiki: the wiki slug (e.g. `arch/overview`). For kind=file: a path to the file, relative to your cwd or absolute (e.g. `docs/design.md`)."
                        },
                        "title": {
                            "type": "string",
                            "description": "Optional tab label. Defaults to the slug / file name."
                        }
                    },
                    "required": ["kind", "ref"]
                }
            }
        },
        "required": ["questions"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TmpHome {
        dir: std::path::PathBuf,
        previous_fleet: Option<std::ffi::OsString>,
        previous_codex: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = fresh_tmp_dir(tag);
            std::fs::create_dir_all(&dir).unwrap();
            let previous_fleet = std::env::var_os("FLEET_HOME");
            let previous_codex = std::env::var_os("CODEX_HOME");
            unsafe {
                std::env::set_var("FLEET_HOME", &dir);
                std::env::set_var("CODEX_HOME", dir.join(".codex"));
            }
            Self { dir, previous_fleet, previous_codex, _lock: lock }
        }

        fn plant_codex_session(&self, id: &str, workspace: &std::path::Path) {
            let sessions = self.dir.join(".codex/sessions/2026/07/16");
            std::fs::create_dir_all(&sessions).unwrap();
            let record = json!({
                "type": "session_meta",
                "payload": {
                    "id": id,
                    "session_id": id,
                    "originator": "fleet",
                    "cwd": workspace,
                    "source": "exec"
                }
            });
            std::fs::write(
                sessions.join(format!("rollout-2026-07-16T00-00-00-{id}.jsonl")),
                format!("{record}\n"),
            )
            .unwrap();
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.previous_fleet.take() {
                    Some(value) => std::env::set_var("FLEET_HOME", value),
                    None => std::env::remove_var("FLEET_HOME"),
                }
                match self.previous_codex.take() {
                    Some(value) => std::env::set_var("CODEX_HOME", value),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn empty_request(id: &str) -> FleetAskRequest {
        FleetAskRequest {
            parked: false,
            id: id.into(),
            session_id: String::new(),
            workspace_name: String::new(),
            ai_title: None,
            timestamp: String::new(),
            review_docs: vec![],
            questions: vec![FleetAskQuestion {
                question: "q".into(),
                header: "h".into(),
                multi_select: false,
                options: vec![],
                html: None,
                form_fields: vec![],
                images: vec![],
            }],
        }
    }

    #[test]
    fn round_trip_minimal_request() {
        let req = FleetAskRequest {
            parked: false,
            id: "abc".into(),
            session_id: String::new(),
            workspace_name: String::new(),
            ai_title: None,
            timestamp: String::new(),
            review_docs: vec![],
            questions: vec![FleetAskQuestion {
                question: "Pick one".into(),
                header: "Choice".into(),
                multi_select: false,
                options: vec![],
                html: None,
                form_fields: vec![],
                images: vec![],
            }],
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: FleetAskRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.questions[0].question, "Pick one");
        // Empty/None fields are skipped on the wire.
        assert!(!s.contains("\"options\""));
        assert!(!s.contains("\"html\""));
        assert!(!s.contains("\"formFields\""));
    }

    #[test]
    fn resolve_review_docs_joins_relative_files_and_leaves_wiki() {
        let mut req = empty_request("rd1");
        req.review_docs = vec![
            ReviewDoc {
                kind: ReviewDocKind::File,
                reference: "docs/design.md".into(),
                title: None,
            },
            ReviewDoc {
                kind: ReviewDocKind::Wiki,
                reference: "arch/overview".into(),
                title: Some("Overview".into()),
            },
        ];
        // A cwd that surely exists but under which the file does not — so
        // canonicalize falls back to the plain join instead of erroring.
        let base = std::env::temp_dir();
        resolve_review_docs(&mut req, Some(&base));

        // Relative file ref became absolute (joined onto the base dir).
        assert!(Path::new(&req.review_docs[0].reference).is_absolute());
        assert!(req.review_docs[0].reference.ends_with("docs/design.md"));
        // Wiki slug is left verbatim — it is not a path.
        assert_eq!(req.review_docs[1].reference, "arch/overview");
    }

    #[test]
    fn review_docs_round_trip_uses_ref_key() {
        let mut req = empty_request("rd2");
        req.review_docs = vec![ReviewDoc {
            kind: ReviewDocKind::File,
            reference: "/abs/x.md".into(),
            title: None,
        }];
        let s = serde_json::to_string(&req).unwrap();
        // Agent-facing key is `ref` / `reviewDocs`, not the Rust field names.
        assert!(s.contains("\"reviewDocs\""));
        assert!(s.contains("\"ref\":\"/abs/x.md\""));
        assert!(s.contains("\"kind\":\"file\""));
        let back: FleetAskRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.review_docs[0].reference, "/abs/x.md");
    }

    #[test]
    fn request_carries_session_envelope() {
        let mut req = empty_request("e1");
        req.session_id = "sess-123".into();
        req.workspace_name = "claude-fleet".into();
        req.ai_title = Some("Implement P2".into());
        req.timestamp = "2026-05-25T12:00:00Z".into();
        let s = serde_json::to_string(&req).unwrap();
        let back: FleetAskRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.session_id, "sess-123");
        assert_eq!(back.workspace_name, "claude-fleet");
        assert_eq!(back.ai_title.as_deref(), Some("Implement P2"));
        assert_eq!(back.timestamp, "2026-05-25T12:00:00Z");
        // camelCase on the wire — matches elicitation pattern.
        assert!(s.contains("\"sessionId\""));
        assert!(s.contains("\"workspaceName\""));
        assert!(s.contains("\"aiTitle\""));
    }

    #[test]
    fn camel_case_field_names() {
        let q = FleetAskQuestion {
            question: "q".into(),
            header: "h".into(),
            multi_select: true,
            options: vec![],
            html: None,
            form_fields: vec![FleetAskFormField {
                name: "commit_msg".into(),
                kind: FormFieldKind::Textarea,
                label: "Message".into(),
                placeholder: None,
                options: vec![],
                required: true,
                default: None,
                min: None,
                max: None,
                step: None,
            }],
            images: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&q).unwrap();
        assert!(v.get("multiSelect").is_some(), "multiSelect (camelCase)");
        assert!(v.get("formFields").is_some(), "formFields (camelCase)");
        assert!(v.get("multi_select").is_none(), "no snake_case bleed");
        assert!(v.get("form_fields").is_none(), "no snake_case bleed");
    }

    // ── File-IPC tests (mirror of elicitation::tests::list_pending_in_dir_*) ─

    fn fresh_tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-ask-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn list_pending_in_dir_missing_returns_ok_empty() {
        let dir = fresh_tmp_dir("missing");
        assert!(!dir.exists());
        let result = list_pending_in_dir(&dir);
        assert!(matches!(&result, Ok(v) if v.is_empty()), "got {result:?}");
    }

    #[test]
    fn list_pending_in_dir_filters_responses() {
        let dir = fresh_tmp_dir("filter");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("req-a.json"), "{}").unwrap();
        std::fs::write(dir.join("req-b.json"), "{}").unwrap();
        std::fs::write(dir.join("req-a.response.json"), "{}").unwrap();
        std::fs::write(dir.join("ignored.txt"), "ignore").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert_eq!(ids, vec!["req-b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_pending_in_dir_excludes_already_answered() {
        // Regression: orphaned `<id>.json` paired with `<id>.response.json`
        // must not re-surface, exactly like the elicitation orphan filter.
        let dir = fresh_tmp_dir("answered");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("answered.json"), "{}").unwrap();
        std::fs::write(dir.join("answered.response.json"), "{}").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert!(ids.is_empty(), "answered orphan leaked: {ids:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn answered_orphan_resumes_codex_and_cleans_live_pair() {
        let home = TmpHome::new("orphan-resume");
        let workspace = home.dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session_id = "019f6aba-6dd8-7c90-a70e-c969ea896bd2";
        home.plant_codex_session(session_id, &workspace);

        let mut req = empty_request("orphan-1");
        req.session_id = session_id.into();
        req.timestamp = "2026-07-16T11:41:31Z".into();
        write_request(&req).unwrap();
        let response = FleetAskResponse {
            id: req.id.clone(),
            answers: BTreeMap::from([("q".into(), "继续".into())]),
            cancelled: false,
        };
        std::fs::write(
            response_path(&req.id).unwrap(),
            serde_json::to_vec(&response).unwrap(),
        )
        .unwrap();

        let mut resumed = None;
        let recovered = recover_orphaned_response_with(
            &req.id,
            |_| false,
            |id, cwd, prompt, model, effort, _| {
                resumed = Some((
                    id.to_string(),
                    cwd.to_string(),
                    prompt.to_string(),
                    model.map(str::to_string),
                    effort.map(str::to_string),
                ));
                Ok(())
            },
        )
        .unwrap();

        assert!(recovered);
        let (id, cwd, prompt, _, _) = resumed.expect("orphan must resume");
        assert_eq!(id, session_id);
        assert_eq!(cwd, workspace.to_string_lossy());
        assert!(prompt.contains("【老板的选择】继续"), "{prompt}");
        assert!(!request_path(&req.id).unwrap().exists());
        assert!(!response_path(&req.id).unwrap().exists());
        assert!(!crate::parked::is_parked(&req.id));
    }

    #[test]
    fn live_producer_keeps_ownership_of_response() {
        let home = TmpHome::new("live-owner");
        let mut req = empty_request("live-1");
        req.session_id = "still-running".into();
        write_request(&req).unwrap();
        let response = FleetAskResponse {
            id: req.id.clone(),
            answers: BTreeMap::from([("q".into(), "继续".into())]),
            cancelled: false,
        };
        std::fs::write(
            response_path(&req.id).unwrap(),
            serde_json::to_vec(&response).unwrap(),
        )
        .unwrap();

        let recovered = recover_orphaned_response_with(
            &req.id,
            |_| true,
            |_, _, _, _, _, _| panic!("a live producer must not be resumed"),
        )
        .unwrap();
        assert!(!recovered);
        assert!(request_path(&req.id).unwrap().exists());
        assert!(response_path(&req.id).unwrap().exists());
        drop(home);
    }

    #[test]
    fn cancelled_orphan_is_cleaned_without_resume() {
        let home = TmpHome::new("orphan-cancel");
        let workspace = home.dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session_id = "019f6aba-6dd8-7c90-a70e-c969ea896bd3";
        home.plant_codex_session(session_id, &workspace);
        let mut req = empty_request("orphan-cancel-1");
        req.session_id = session_id.into();
        write_request(&req).unwrap();
        let response = FleetAskResponse {
            id: req.id.clone(),
            answers: BTreeMap::new(),
            cancelled: true,
        };
        std::fs::write(
            response_path(&req.id).unwrap(),
            serde_json::to_vec(&response).unwrap(),
        )
        .unwrap();

        assert!(recover_orphaned_response_with(
            &req.id,
            |_| false,
            |_, _, _, _, _, _| panic!("cancel must not resume"),
        )
        .unwrap());
        assert!(!request_path(&req.id).unwrap().exists());
        assert!(!response_path(&req.id).unwrap().exists());
    }

    #[test]
    fn form_field_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(FormFieldKind::Textarea).unwrap(),
            json!("textarea")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Checkbox).unwrap(),
            json!("checkbox")
        );
        let back: FormFieldKind = serde_json::from_value(json!("radio")).unwrap();
        assert_eq!(back, FormFieldKind::Radio);
    }

    #[test]
    fn response_round_trip_with_form_answers() {
        let mut answers = BTreeMap::new();
        answers.insert("commit_msg".into(), "fix: typo".into());
        answers.insert("strategy".into(), "rebase".into());
        let resp = FleetAskResponse {
            id: "xyz".into(),
            answers,
            cancelled: false,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: FleetAskResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.answers.get("commit_msg").unwrap(), "fix: typo");
        assert_eq!(back.answers.get("strategy").unwrap(), "rebase");
        assert!(!back.cancelled);
    }

    #[test]
    fn response_round_trip_cancelled() {
        let resp = FleetAskResponse {
            id: "k".into(),
            answers: BTreeMap::new(),
            cancelled: true,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: FleetAskResponse = serde_json::from_str(&s).unwrap();
        assert!(back.cancelled);
    }

    #[test]
    fn schema_advertises_html_and_form_fields_in_camel_case() {
        let s = fleet_ask_input_schema();
        let props = &s["properties"]["questions"]["items"]["properties"];
        assert!(props.get("html").is_some(), "html field present");
        assert!(
            props.get("formFields").is_some(),
            "formFields camelCase, not snake_case"
        );
        assert!(props.get("form_fields").is_none());
        // Form-field kinds enum surfaces in schema:
        let kinds = &props["formFields"]["items"]["properties"]["kind"]["enum"];
        let arr = kinds.as_array().expect("enum array");
        assert!(arr.iter().any(|v| v == "textarea"));
        assert!(arr.iter().any(|v| v == "checkbox"));
    }

    #[test]
    fn form_field_kind_serialises_new_variants_lowercase() {
        assert_eq!(
            serde_json::to_value(FormFieldKind::Date).unwrap(),
            json!("date")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Datetime).unwrap(),
            json!("datetime")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Time).unwrap(),
            json!("time")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Range).unwrap(),
            json!("range")
        );
        let back: FormFieldKind = serde_json::from_value(json!("date")).unwrap();
        assert_eq!(back, FormFieldKind::Date);
        let back: FormFieldKind = serde_json::from_value(json!("range")).unwrap();
        assert_eq!(back, FormFieldKind::Range);
    }

    #[test]
    fn form_field_min_max_step_round_trip() {
        let f = FleetAskFormField {
            name: "volume".into(),
            kind: FormFieldKind::Range,
            label: "Volume".into(),
            placeholder: None,
            options: vec![],
            required: false,
            default: Some(json!(50)),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(5.0),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["min"], json!(0.0));
        assert_eq!(v["max"], json!(100.0));
        assert_eq!(v["step"], json!(5.0));
        assert_eq!(v["kind"], json!("range"));
        let back: FleetAskFormField = serde_json::from_value(v).unwrap();
        assert_eq!(back.min, Some(0.0));
        assert_eq!(back.max, Some(100.0));
        assert_eq!(back.step, Some(5.0));
        assert_eq!(back.kind, FormFieldKind::Range);
    }

    #[test]
    fn form_field_min_max_step_omitted_when_none() {
        let f = FleetAskFormField {
            name: "msg".into(),
            kind: FormFieldKind::Text,
            label: "Message".into(),
            placeholder: None,
            options: vec![],
            required: false,
            default: None,
            min: None,
            max: None,
            step: None,
        };
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.get("min").is_none(), "min skipped when None");
        assert!(v.get("max").is_none(), "max skipped when None");
        assert!(v.get("step").is_none(), "step skipped when None");
    }

    #[test]
    fn schema_advertises_new_kinds_and_range_bounds() {
        let s = fleet_ask_input_schema();
        let item_props =
            &s["properties"]["questions"]["items"]["properties"]["formFields"]["items"]["properties"];
        let kinds = item_props["kind"]["enum"]
            .as_array()
            .expect("enum array");
        for k in ["date", "datetime", "time", "range"] {
            assert!(kinds.iter().any(|v| v == k), "kind enum missing {k}");
        }
        assert!(item_props.get("min").is_some(), "min advertised");
        assert!(item_props.get("max").is_some(), "max advertised");
        assert!(item_props.get("step").is_some(), "step advertised");
    }

    #[test]
    fn parses_request_with_html_and_form() {
        let raw = json!({
            "id": "r1",
            "questions": [{
                "question": "Confirm commit message and rebase strategy",
                "header": "Commit",
                "multiSelect": false,
                "html": "<p>Diff preview</p>",
                "formFields": [
                    {
                        "name": "commit_msg",
                        "kind": "textarea",
                        "label": "Commit message",
                        "required": true
                    },
                    {
                        "name": "strategy",
                        "kind": "radio",
                        "label": "Strategy",
                        "options": ["merge", "rebase", "squash"]
                    }
                ]
            }]
        });
        let req: FleetAskRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.questions[0].html.as_deref(), Some("<p>Diff preview</p>"));
        assert_eq!(req.questions[0].form_fields.len(), 2);
        assert_eq!(req.questions[0].form_fields[1].kind, FormFieldKind::Radio);
        assert_eq!(
            req.questions[0].form_fields[1].options,
            vec!["merge", "rebase", "squash"]
        );
    }

    #[test]
    fn normalize_html_covers_both_iframe_paths_and_is_idempotent() {
        // srcDoc path: a question's own html gets the script, so the card's
        // iframe can size itself instead of clipping into a fixed-height box.
        let mut req = empty_request("card-ah");
        req.questions[0].html = Some("<p>hi</p>".into());
        normalize_html(&mut req);
        let once = req.questions[0].html.clone().unwrap();
        assert!(once.starts_with("<p>hi</p>"), "author's html must survive intact: {once}");
        assert!(once.contains(AUTOHEIGHT_MARKER));

        // Re-injecting (e.g. a replayed request) must not stack a second copy.
        normalize_html(&mut req);
        assert_eq!(req.questions[0].html.as_deref(), Some(once.as_str()));
        assert_eq!(once.matches(AUTOHEIGHT_MARKER).count(), 1);

        // A question with no html keeps none — nothing to size, nothing to frame.
        let mut blank = empty_request("card-blank");
        normalize_html(&mut blank);
        assert_eq!(blank.questions[0].html, None);
    }

    #[test]
    fn content_free_html_is_dropped_instead_of_framed() {
        // An agent that stubs the field with a placeholder comment (observed in
        // the wild: `"html": "<!--HTML-->"`) has authored a document that renders
        // nothing. Keeping it would hand the card a truthy `html` and paint a
        // 200px-tall empty sandbox iframe over the question body.
        for junk in ["<!--HTML-->", "  <!-- todo: preview -->\n", "", "   "] {
            let mut req = empty_request("card-junk");
            req.questions[0].html = Some(junk.into());
            normalize_html(&mut req);
            assert_eq!(
                req.questions[0].html, None,
                "html with no renderable content must be dropped, got {:?}",
                req.questions[0].html
            );
        }

        // Content-free does NOT mean text-free: a document whose only content is
        // a replaced element still renders, and must survive with its script.
        let mut img = empty_request("card-img-only");
        img.questions[0].html = Some("<!-- chart --><img src=\"c.png\">".into());
        normalize_html(&mut img);
        let out = img.questions[0].html.clone().expect("markup must survive");
        assert!(out.starts_with("<!-- chart --><img src=\"c.png\">"));
        assert!(out.contains(AUTOHEIGHT_MARKER));
    }

    #[test]
    fn ingest_images_copies_serves_and_defends() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = fresh_tmp_dir("assets");
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized via fleet_home_lock
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        // A fake source image sitting on the "agent host".
        let src = tmp.join("orig-chart.png");
        std::fs::write(&src, b"\x89PNG\r\n\x1a\nFAKE").unwrap();

        // Case 1: agent-supplied html referencing the image relatively.
        let mut req = empty_request("card-img");
        req.questions[0].html = Some("<img src=\"chart.png\">".into());
        req.questions[0].images = vec![FleetAskImage {
            name: "chart.png".into(),
            path: src.to_string_lossy().to_string(),
            caption: Some("cap".into()),
        }];
        ingest_images(&mut req).unwrap();

        // Local path is blanked; name/caption survive for rendering.
        assert_eq!(req.questions[0].images[0].name, "chart.png");
        assert!(
            req.questions[0].images[0].path.is_empty(),
            "absolute local path must not persist into the request/history"
        );

        // index.html == the agent's html (not the synthesized gallery), plus the
        // height-reporting script the card's iframe needs to size itself.
        let index = read_decision_asset("card-img", "q0", "index.html").unwrap();
        assert_eq!(index.mime, "text/html; charset=utf-8");
        let idx = String::from_utf8_lossy(&index.bytes);
        assert!(idx.starts_with("<img src=\"chart.png\">"), "agent html served verbatim: {idx}");
        assert!(idx.contains(AUTOHEIGHT_MARKER), "served entry must report its height: {idx}");

        // The image serves with correct bytes + mime.
        let png = read_decision_asset("card-img", "q0", "chart.png").unwrap();
        assert_eq!(png.mime, "image/png");
        assert_eq!(png.bytes, b"\x89PNG\r\n\x1a\nFAKE");

        // Path traversal is rejected on both the id and the relpath.
        assert!(read_decision_asset("card-img", "q0", "../../etc/passwd").is_err());
        assert!(read_decision_asset("../x", "q0", "index.html").is_err());

        // Case 2: missing source dropped (non-fatal), unsafe name rejected,
        // and with no html a gallery is synthesized referencing survivors.
        let mut req2 = empty_request("card-img2");
        req2.questions[0].html = None;
        req2.questions[0].images = vec![
            FleetAskImage {
                name: "ok.png".into(),
                path: src.to_string_lossy().to_string(),
                caption: None,
            },
            FleetAskImage {
                name: "gone.png".into(),
                path: tmp.join("nope.png").to_string_lossy().to_string(),
                caption: None,
            },
            FleetAskImage {
                name: "../evil".into(),
                path: src.to_string_lossy().to_string(),
                caption: None,
            },
        ];
        ingest_images(&mut req2).unwrap();
        assert_eq!(req2.questions[0].images.len(), 1);
        assert_eq!(req2.questions[0].images[0].name, "ok.png");
        let gallery = read_decision_asset("card-img2", "q0", "index.html").unwrap();
        let g = String::from_utf8_lossy(&gallery.bytes);
        assert!(g.contains("src=\"ok.png\""), "gallery must reference survivor: {g}");
        assert!(g.contains(AUTOHEIGHT_MARKER), "synthesized gallery must report its height: {g}");

        // restore env + clean up
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
