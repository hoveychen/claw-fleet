//! Native OpenAI Images API client — the replacement for driving Codex's
//! bundled `imagegen` skill through a throwaway agent turn.
//!
//! **Why this exists.** [`crate::codex_image`] generates pictures by spawning
//! `codex exec` and asking the agent to call its built-in `image_gen` tool. That
//! tool is a closed door: upstream pins the model to `gpt-image-2` in a client
//! constant and always sends `quality: auto`, `size: auto`,
//! `background: auto`, and its argument struct is `deny_unknown_fields`, so
//! there is no way — not through the prompt, not through a flag — to ask for a
//! different model or a higher quality tier. Verified against codex-cli
//! 0.156.0-alpha.7: `struct ImagegenArgs with 3 elements`, and the whole source
//! tree contains no `gpt-image-2.5` at all.
//!
//! Meanwhile `gpt-image-2.5` shipped 2026-09-08 with two same-priced variants —
//! `flare` (speed) and `sunburst` (quality) — plus two new quality tiers
//! (`xhigh`, `max`), 4K output and transparent backgrounds. Reaching those means
//! talking to the Images API ourselves.
//!
//! **Two backends, one request shape.** The only differences between them are
//! the base URL and the auth headers:
//!
//! - [`ImageAuth::ApiKey`] → `https://api.openai.com/v1`, billed per token.
//! - [`ImageAuth::ChatGpt`] → `https://chatgpt.com/backend-api/codex`, billed
//!   against the ChatGPT plan. This is the same endpoint Codex itself posts to
//!   (`codex-rs/codex-api/src/endpoint/images.rs` joins `images/generations`
//!   onto `CHATGPT_CODEX_BASE_URL`), with the same headers, and the body Codex
//!   sends already carries `model`/`quality`/`size`/`background` — it just fills
//!   them with constants. We fill them with what the caller asked for.
//!
//! Whether that backend accepts the `gpt-image-2.5-*` model names is **not yet
//! verified** — the local ChatGPT login was revoked before the probe could run,
//! so treat [`ImageAuth::ChatGpt`] + a 2.5 model as unproven until a live call
//! says otherwise.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// `gpt-image-2.5`, quality-optimised. Same price as flare, slower, sharper.
pub const MODEL_SUNBURST: &str = "gpt-image-2.5-sunburst";
/// `gpt-image-2.5`, speed-optimised. Roughly `gpt-image-2` quality at up to
/// half the latency.
pub const MODEL_FLARE: &str = "gpt-image-2.5-flare";
/// What Codex's built-in tool is pinned to; kept reachable for fallback and
/// for comparing against the old behaviour.
pub const MODEL_GPT_IMAGE_2: &str = "gpt-image-2";

/// Default model. Flare rather than sunburst because the two cost the same and
/// flare is never slower than the `gpt-image-2` this module replaces — callers
/// who want the quality tier ask for it explicitly.
pub const DEFAULT_MODEL: &str = MODEL_FLARE;

/// Generous by design: a `max`-quality 4K render is minutes of work, and the
/// old Codex-driven path had no timeout at all (a hung turn hung Fleet).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

const API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_BASE: &str = "https://chatgpt.com/backend-api/codex";

/// Quality tiers. `xhigh` and `max` exist only on `gpt-image-2.5`.
const QUALITIES: &[&str] = &["low", "medium", "high", "xhigh", "max", "auto"];
const BACKGROUNDS: &[&str] = &["transparent", "opaque", "auto"];
const OUTPUT_FORMATS: &[&str] = &["png", "jpeg", "webp"];
/// Formats that can carry an alpha channel; `background=transparent` needs one.
const ALPHA_FORMATS: &[&str] = &["png", "webp"];

// `gpt-image-2`/`2.5` size constraints, per the Images API docs.
const MAX_EDGE: u32 = 3840;
const EDGE_MULTIPLE: u32 = 16;
const MAX_RATIO: f64 = 3.0;
const MIN_PIXELS: u64 = 655_360;
const MAX_PIXELS: u64 = 8_294_400;

/// How to authenticate, and by extension which backend to talk to.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageAuth {
    /// Platform API key. Public contract, billed per token.
    ApiKey(String),
    /// Codex's ChatGPT session, read out of `auth.json`. Billed against the
    /// plan quota.
    ChatGpt {
        access_token: String,
        account_id: Option<String>,
    },
}

impl ImageAuth {
    /// Base URL this credential talks to. Mirrors codex-rs's
    /// `model-provider-info`, which picks the ChatGPT backend for every
    /// OAuth-ish auth mode and `api.openai.com/v1` for a raw key.
    pub fn base_url(&self) -> &'static str {
        match self {
            ImageAuth::ApiKey(_) => API_BASE,
            ImageAuth::ChatGpt { .. } => CHATGPT_BASE,
        }
    }

    /// Short label for logs and error messages. Never includes the secret.
    pub fn label(&self) -> &'static str {
        match self {
            ImageAuth::ApiKey(_) => "api-key",
            ImageAuth::ChatGpt { .. } => "chatgpt",
        }
    }
}

/// Resolve a credential: an explicit `OPENAI_API_KEY` wins, otherwise fall back
/// to Codex's ChatGPT session.
///
/// The key comes first because it is the one the caller deliberately set, and
/// because it is the backend whose contract is public — if both are present,
/// silently spending the plan quota would be the surprising choice.
pub fn load_auth(codex_home: Option<&Path>) -> Result<ImageAuth, String> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(ImageAuth::ApiKey(key.trim().to_string()));
        }
    }
    let home = codex_home
        .map(Path::to_path_buf)
        .or_else(crate::codex_launch::codex_home)
        .ok_or_else(|| "no OPENAI_API_KEY and no CODEX_HOME to fall back on".to_string())?;
    auth_from_codex_home(&home)
}

/// Read `<codex_home>/auth.json` into an [`ImageAuth`].
///
/// Split out from [`load_auth`] so tests can point at a fixture directory
/// without touching the process environment.
pub fn auth_from_codex_home(codex_home: &Path) -> Result<ImageAuth, String> {
    let path = codex_home.join("auth.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    auth_from_json(&v)
}

/// Pure projection of a parsed `auth.json`, split out for unit testing.
///
/// Shape matches [`crate::harness_status`]'s reader: either a top-level
/// `OPENAI_API_KEY`, or `tokens.{access_token,account_id}`.
pub fn auth_from_json(v: &serde_json::Value) -> Result<ImageAuth, String> {
    if let Some(key) = v.get("OPENAI_API_KEY").and_then(serde_json::Value::as_str) {
        if !key.trim().is_empty() {
            return Ok(ImageAuth::ApiKey(key.trim().to_string()));
        }
    }
    let tokens = v
        .get("tokens")
        .ok_or_else(|| "auth.json has neither OPENAI_API_KEY nor tokens".to_string())?;
    let access_token = tokens
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "auth.json tokens.access_token is missing or empty".to_string())?;
    let account_id = tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .filter(|a| !a.trim().is_empty())
        .map(str::to_string);
    Ok(ImageAuth::ChatGpt {
        access_token: access_token.to_string(),
        account_id,
    })
}

/// One generation request. Every field the Images API exposes for GPT Image
/// models, which is the entire point of this module.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImageRequest {
    pub prompt: String,
    /// Defaults to [`DEFAULT_MODEL`] when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `low` | `medium` | `high` | `xhigh` | `max` | `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// `auto` or `WIDTHxHEIGHT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// `transparent` | `opaque` | `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// `png` | `jpeg` | `webp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// 0-100, jpeg/webp only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_compression: Option<u8>,
    /// Variants of *this* prompt, 1-10. Distinct assets want distinct requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u8>,
    /// `auto` (default) | `low`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation: Option<String>,
    /// Reference or edit-target images. Non-empty routes the call to
    /// `images/edits` instead of `images/generations`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PathBuf>,
    /// Optional alpha mask, edits only. Applies to the first image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<PathBuf>,
}

impl ImageRequest {
    /// Minimal request: just a prompt, everything else left to the API default.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            quality: None,
            size: None,
            background: None,
            output_format: None,
            output_compression: None,
            n: None,
            moderation: None,
            images: Vec::new(),
            mask: None,
        }
    }

    /// Model actually sent on the wire.
    pub fn effective_model(&self) -> &str {
        self.model.as_deref().unwrap_or(DEFAULT_MODEL)
    }

    /// An edit is a request that carries input images; there is no separate
    /// mode flag, matching how Codex's own tool decides.
    pub fn is_edit(&self) -> bool {
        !self.images.is_empty()
    }
}

/// One returned image, already decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    /// Server-assigned id, when it sends one. Useful as a handle for follow-up
    /// edits.
    pub generation_id: Option<String>,
}

/// Validate everything we can before spending a network round trip — and, on
/// the plan-quota backend, before spending quota.
///
/// Deliberately strict about the combinations the API rejects anyway
/// (`background=transparent` on a format with no alpha channel), and about the
/// size grid, because a rejected 4K request still costs the caller a minute of
/// waiting.
pub fn validate(req: &ImageRequest) -> Result<(), String> {
    if req.prompt.trim().is_empty() {
        return Err("prompt is empty".to_string());
    }
    if let Some(q) = &req.quality {
        if !QUALITIES.contains(&q.as_str()) {
            return Err(format!("quality must be one of {}", QUALITIES.join(", ")));
        }
    }
    if let Some(b) = &req.background {
        if !BACKGROUNDS.contains(&b.as_str()) {
            return Err(format!(
                "background must be one of {}",
                BACKGROUNDS.join(", ")
            ));
        }
    }
    if let Some(f) = &req.output_format {
        if !OUTPUT_FORMATS.contains(&f.as_str()) {
            return Err(format!(
                "output_format must be one of {}",
                OUTPUT_FORMATS.join(", ")
            ));
        }
    }
    if req.background.as_deref() == Some("transparent") {
        // png is the API default, so an unset format is fine here.
        let fmt = req.output_format.as_deref().unwrap_or("png");
        if !ALPHA_FORMATS.contains(&fmt) {
            return Err(format!(
                "background=transparent needs output_format png or webp, not {fmt}"
            ));
        }
    }
    if let Some(c) = req.output_compression {
        if c > 100 {
            return Err("output_compression must be between 0 and 100".to_string());
        }
    }
    if let Some(n) = req.n {
        if !(1..=10).contains(&n) {
            return Err("n must be between 1 and 10".to_string());
        }
    }
    if let Some(m) = &req.moderation {
        if m != "auto" && m != "low" {
            return Err("moderation must be auto or low".to_string());
        }
    }
    if let Some(size) = &req.size {
        validate_size(size)?;
    }
    if req.mask.is_some() && !req.is_edit() {
        return Err("mask is only meaningful with input images".to_string());
    }
    for path in req.images.iter().chain(req.mask.as_ref()) {
        if !path.is_file() {
            return Err(format!("input image not found: {}", path.display()));
        }
    }
    Ok(())
}

/// `auto`, or a `WIDTHxHEIGHT` on the GPT Image size grid.
fn validate_size(size: &str) -> Result<(), String> {
    if size == "auto" {
        return Ok(());
    }
    let (w, h) = size
        .split_once('x')
        .ok_or_else(|| format!("size must be auto or WIDTHxHEIGHT, got {size}"))?;
    let w: u32 = w
        .parse()
        .map_err(|_| format!("size must be auto or WIDTHxHEIGHT, got {size}"))?;
    let h: u32 = h
        .parse()
        .map_err(|_| format!("size must be auto or WIDTHxHEIGHT, got {size}"))?;
    if w == 0 || h == 0 {
        return Err(format!("size must have non-zero edges, got {size}"));
    }
    if w % EDGE_MULTIPLE != 0 || h % EDGE_MULTIPLE != 0 {
        return Err(format!("size edges must be multiples of {EDGE_MULTIPLE}px"));
    }
    if w.max(h) > MAX_EDGE {
        return Err(format!("size edges must not exceed {MAX_EDGE}px"));
    }
    if f64::from(w.max(h)) / f64::from(w.min(h)) > MAX_RATIO {
        return Err("size aspect ratio must not exceed 3:1".to_string());
    }
    let pixels = u64::from(w) * u64::from(h);
    if !(MIN_PIXELS..=MAX_PIXELS).contains(&pixels) {
        return Err(format!(
            "size must be between {MIN_PIXELS} and {MAX_PIXELS} total pixels, got {pixels}"
        ));
    }
    Ok(())
}

/// The JSON body for `images/generations`.
///
/// Built as a `Value` rather than a struct because the field set is exactly the
/// caller's optional fields — skipping the unset ones keeps us on the API's
/// defaults instead of pinning them, which is the mistake the Codex client
/// makes.
pub fn generation_body(req: &ImageRequest) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), req.effective_model().into());
    body.insert("prompt".into(), req.prompt.clone().into());
    for (key, value) in [
        ("quality", req.quality.clone()),
        ("size", req.size.clone()),
        ("background", req.background.clone()),
        ("output_format", req.output_format.clone()),
        ("moderation", req.moderation.clone()),
    ] {
        if let Some(v) = value {
            body.insert(key.into(), v.into());
        }
    }
    if let Some(c) = req.output_compression {
        body.insert("output_compression".into(), c.into());
    }
    if let Some(n) = req.n {
        body.insert("n".into(), n.into());
    }
    serde_json::Value::Object(body)
}

/// Endpoint path, relative to the backend's base URL. Both backends use the
/// same two paths.
pub fn endpoint_path(req: &ImageRequest) -> &'static str {
    if req.is_edit() {
        "images/edits"
    } else {
        "images/generations"
    }
}

/// Auth headers for a backend, as `(name, value)` pairs.
///
/// `ChatGPT-Account-ID` and `x-codex-image-turn-id` mirror what Codex sends on
/// the plan-quota backend; the turn id is an opaque correlation handle, so we
/// pass the caller's.
pub fn auth_headers(auth: &ImageAuth, turn_id: &str) -> Vec<(String, String)> {
    match auth {
        ImageAuth::ApiKey(key) => vec![("Authorization".into(), format!("Bearer {key}"))],
        ImageAuth::ChatGpt {
            access_token,
            account_id,
        } => {
            let mut headers = vec![("Authorization".into(), format!("Bearer {access_token}"))];
            if let Some(id) = account_id {
                headers.push(("ChatGPT-Account-ID".into(), id.clone()));
            }
            headers.push(("x-codex-image-turn-id".into(), turn_id.to_string()));
            headers
        }
    }
}

/// Send one request and decode the images it returns.
///
/// Blocking on purpose: every caller (the MCP handler, the Tauri command) is
/// already on a worker thread, and the streaming `partial_images` mode buys
/// nothing when the result is written to disk anyway.
pub fn execute(
    req: &ImageRequest,
    auth: &ImageAuth,
    turn_id: &str,
    timeout: Duration,
) -> Result<Vec<ImageBytes>, String> {
    validate(req)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let url = format!("{}/{}", auth.base_url(), endpoint_path(req));
    let mut builder = client.post(&url);
    for (name, value) in auth_headers(auth, turn_id) {
        builder = builder.header(name, value);
    }
    builder = if req.is_edit() {
        builder.multipart(edit_form(req)?)
    } else {
        builder.json(&generation_body(req))
    };

    let resp = builder
        .send()
        .map_err(|e| format!("{} {url}: {e}", auth.label()))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status} from {url}: {}", tail(&body, 400)));
    }
    decode_response(&body)
}

/// Multipart form for `images/edits`. The API takes repeated `image[]` parts,
/// and scalar fields as plain text parts.
fn edit_form(req: &ImageRequest) -> Result<reqwest::blocking::multipart::Form, String> {
    let mut form = reqwest::blocking::multipart::Form::new()
        .text("model", req.effective_model().to_string())
        .text("prompt", req.prompt.clone());
    for (key, value) in [
        ("quality", req.quality.clone()),
        ("size", req.size.clone()),
        ("background", req.background.clone()),
        ("output_format", req.output_format.clone()),
        ("moderation", req.moderation.clone()),
    ] {
        if let Some(v) = value {
            form = form.text(key, v);
        }
    }
    if let Some(c) = req.output_compression {
        form = form.text("output_compression", c.to_string());
    }
    if let Some(n) = req.n {
        form = form.text("n", n.to_string());
    }
    for path in &req.images {
        form = form
            .file("image[]", path)
            .map_err(|e| format!("attach {}: {e}", path.display()))?;
    }
    if let Some(mask) = &req.mask {
        form = form
            .file("mask", mask)
            .map_err(|e| format!("attach mask {}: {e}", mask.display()))?;
    }
    Ok(form)
}

/// Decode `{ data: [{ b64_json, generation_id }] }`.
///
/// Shape is shared by both backends — it is the same `ImageResponse` struct
/// codex-rs deserialises (`created`, `data`, `background`, `quality`, `size`).
pub fn decode_response(body: &str) -> Result<Vec<ImageBytes>, String> {
    use base64::Engine as _;
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("parse response: {e}"))?;
    let data = v
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("response has no data array: {}", tail(body, 200)))?;
    let mut out = Vec::with_capacity(data.len());
    for item in data {
        let b64 = item
            .get("b64_json")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "response item has no b64_json".to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("decode b64_json: {e}"))?;
        out.push(ImageBytes {
            bytes,
            generation_id: item
                .get("generation_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        });
    }
    if out.is_empty() {
        return Err("response returned no images".to_string());
    }
    Ok(out)
}

/// Last `n` characters, for quoting an error body without flooding a log line.
fn tail(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    chars[chars.len() - n..].iter().collect()
}

// ── Output store ────────────────────────────────────────────────────────────
//
// The Codex-driven path never had to think about where pictures live: the
// built-in tool wrote them to `$CODEX_HOME/generated_images/<thread_id>/`, and
// the thread id doubled as the directory name, the `fleet-genimage://` key, the
// `/session_images` query parameter and the follow-up-edit handle. Calling the
// API ourselves means there is no thread and no id, so we mint one.
//
// The handle is prefixed, and [`crate::codex_image::thread_images_dir`] routes
// on that prefix. That is what keeps every existing reader — the custom
// protocol, the two `fleet serve` routes, the desktop thumbnail strip — working
// unchanged, and what keeps images from older Codex threads readable instead of
// orphaning them.

/// Marks a handle as ours rather than a Codex thread id. Codex ids are bare
/// UUIDs, so no real thread can collide with this.
pub const HANDLE_PREFIX: &str = "img-";

/// Is this a handle minted by [`new_handle`] rather than a Codex thread id?
pub fn is_native_handle(handle: &str) -> bool {
    handle.starts_with(HANDLE_PREFIX) && handle_is_safe(handle)
}

/// A handle is a directory name, so it has to be one path segment and nothing
/// clever. Checked on both the write and the read side.
fn handle_is_safe(handle: &str) -> bool {
    !handle.is_empty()
        && !handle.contains('/')
        && !handle.contains('\\')
        && !handle.contains("..")
        && !Path::new(handle).is_absolute()
}

/// Mint a fresh handle for one generation.
pub fn new_handle() -> String {
    format!("{HANDLE_PREFIX}{}", uuid::Uuid::new_v4())
}

/// Where this handle's images live: `<fleet dir>/generated_images/<handle>`.
///
/// Deliberately under Fleet's own directory, not `$CODEX_HOME`: these are not
/// Codex's output any more, and a user who logs out of Codex or clears its home
/// should not lose them.
pub fn handle_dir(handle: &str) -> Option<PathBuf> {
    if !handle_is_safe(handle) {
        return None;
    }
    crate::session::get_fleet_dir().map(|d| d.join("generated_images").join(handle))
}

/// Write one turn's images into the handle's directory.
///
/// Names are `1.png`, `2.png`, … in the order the API returned them, which
/// keeps `n > 1` variants in the caller's order — unlike the Codex path, which
/// had to sort by file size because it could not tell them apart.
pub fn save_images(
    handle: &str,
    images: &[ImageBytes],
    output_format: Option<&str>,
) -> Result<Vec<crate::codex_image::GeneratedImage>, String> {
    let dir = handle_dir(handle).ok_or_else(|| format!("invalid image handle '{handle}'"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let ext = match output_format.unwrap_or("png") {
        "jpeg" => "jpg",
        other => other,
    };
    let mut out = Vec::with_capacity(images.len());
    for (i, image) in images.iter().enumerate() {
        let path = dir.join(format!("{}.{ext}", i + 1));
        std::fs::write(&path, &image.bytes)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        out.push(crate::codex_image::GeneratedImage {
            path: path.to_string_lossy().to_string(),
            bytes: image.bytes.len() as u64,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_fixture(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        path
    }

    #[test]
    fn api_key_and_chatgpt_talk_to_different_backends() {
        assert_eq!(ImageAuth::ApiKey("k".into()).base_url(), API_BASE);
        assert_eq!(
            ImageAuth::ChatGpt {
                access_token: "t".into(),
                account_id: None
            }
            .base_url(),
            CHATGPT_BASE
        );
    }

    #[test]
    fn chatgpt_headers_carry_the_account_and_turn_id() {
        let auth = ImageAuth::ChatGpt {
            access_token: "tok".into(),
            account_id: Some("acct".into()),
        };
        let headers = auth_headers(&auth, "turn-7");
        assert!(headers.contains(&("Authorization".into(), "Bearer tok".into())));
        assert!(headers.contains(&("ChatGPT-Account-ID".into(), "acct".into())));
        assert!(headers.contains(&("x-codex-image-turn-id".into(), "turn-7".into())));
    }

    #[test]
    fn api_key_headers_do_not_leak_codex_specific_ones() {
        let headers = auth_headers(&ImageAuth::ApiKey("sk-x".into()), "turn-7");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "Authorization");
    }

    #[test]
    fn an_account_less_chatgpt_session_still_authenticates() {
        let headers = auth_headers(
            &ImageAuth::ChatGpt {
                access_token: "tok".into(),
                account_id: None,
            },
            "t",
        );
        assert!(!headers.iter().any(|(n, _)| n == "ChatGPT-Account-ID"));
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn auth_json_prefers_an_api_key_over_the_chatgpt_session() {
        let v = serde_json::json!({
            "OPENAI_API_KEY": "sk-live",
            "tokens": { "access_token": "tok", "account_id": "acct" }
        });
        assert_eq!(auth_from_json(&v), Ok(ImageAuth::ApiKey("sk-live".into())));
    }

    #[test]
    fn a_null_api_key_falls_through_to_the_chatgpt_session() {
        // This is the real shape of a ChatGPT-logged-in auth.json: the key
        // field is present but null.
        let v = serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": serde_json::Value::Null,
            "tokens": { "access_token": "tok", "account_id": "acct" }
        });
        assert_eq!(
            auth_from_json(&v),
            Ok(ImageAuth::ChatGpt {
                access_token: "tok".into(),
                account_id: Some("acct".into())
            })
        );
    }

    #[test]
    fn auth_json_without_credentials_is_an_error_not_a_silent_anonymous_call() {
        let v = serde_json::json!({ "auth_mode": "chatgpt" });
        assert!(auth_from_json(&v).is_err());
        let v = serde_json::json!({ "tokens": { "access_token": "" } });
        assert!(auth_from_json(&v).is_err());
    }

    #[test]
    fn the_default_model_is_a_2_5_variant() {
        // The whole point of the module: never fall back to the pinned
        // gpt-image-2 that the Codex built-in tool is stuck on.
        assert_eq!(ImageRequest::new("a cat").effective_model(), DEFAULT_MODEL);
        assert!(DEFAULT_MODEL.starts_with("gpt-image-2.5"));
    }

    #[test]
    fn input_images_route_the_call_to_the_edits_endpoint() {
        let mut req = ImageRequest::new("make it night");
        assert_eq!(endpoint_path(&req), "images/generations");
        req.images.push(PathBuf::from("/tmp/x.png"));
        assert_eq!(endpoint_path(&req), "images/edits");
    }

    #[test]
    fn generation_body_omits_every_unset_field() {
        // Pinning `quality: auto` / `size: auto` the way Codex does is exactly
        // the bug this module exists to avoid — unset must stay absent.
        let body = generation_body(&ImageRequest::new("a cat"));
        let obj = body.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj["model"], DEFAULT_MODEL);
        assert_eq!(obj["prompt"], "a cat");
    }

    #[test]
    fn generation_body_carries_every_control_the_caller_set() {
        let mut req = ImageRequest::new("a cat");
        req.model = Some(MODEL_SUNBURST.into());
        req.quality = Some("xhigh".into());
        req.size = Some("3840x2160".into());
        req.background = Some("transparent".into());
        req.output_format = Some("webp".into());
        req.output_compression = Some(80);
        req.n = Some(3);
        req.moderation = Some("low".into());
        let body = generation_body(&req);
        assert_eq!(body["model"], MODEL_SUNBURST);
        assert_eq!(body["quality"], "xhigh");
        assert_eq!(body["size"], "3840x2160");
        assert_eq!(body["background"], "transparent");
        assert_eq!(body["output_format"], "webp");
        assert_eq!(body["output_compression"], 80);
        assert_eq!(body["n"], 3);
        assert_eq!(body["moderation"], "low");
    }

    #[test]
    fn quality_accepts_the_two_tiers_that_only_exist_on_2_5() {
        for q in ["low", "medium", "high", "xhigh", "max", "auto"] {
            let mut req = ImageRequest::new("a cat");
            req.quality = Some(q.into());
            assert!(validate(&req).is_ok(), "{q} should be accepted");
        }
        let mut req = ImageRequest::new("a cat");
        req.quality = Some("ultra".into());
        assert!(validate(&req).is_err());
    }

    #[test]
    fn transparent_background_requires_a_format_with_an_alpha_channel() {
        let mut req = ImageRequest::new("a cutout");
        req.background = Some("transparent".into());
        // png is the API default, so leaving the format unset is fine.
        assert!(validate(&req).is_ok());
        req.output_format = Some("webp".into());
        assert!(validate(&req).is_ok());
        req.output_format = Some("jpeg".into());
        assert!(validate(&req).is_err());
    }

    #[test]
    fn the_size_grid_rejects_what_the_api_would_reject() {
        for good in ["auto", "1024x1024", "1536x1024", "2048x1152", "3840x2160"] {
            assert!(validate_size(good).is_ok(), "{good} should be accepted");
        }
        // Not a multiple of 16.
        assert!(validate_size("1000x1000").is_err());
        // Edge over 3840.
        assert!(validate_size("4096x1024").is_err());
        // Ratio over 3:1.
        assert!(validate_size("3840x1024").is_err());
        // Under the pixel floor.
        assert!(validate_size("512x512").is_err());
        assert!(validate_size("1024").is_err());
        assert!(validate_size("axb").is_err());
    }

    #[test]
    fn a_blank_prompt_never_reaches_the_network() {
        assert!(validate(&ImageRequest::new("   ")).is_err());
    }

    #[test]
    fn n_and_compression_are_bounded() {
        let mut req = ImageRequest::new("a cat");
        req.n = Some(0);
        assert!(validate(&req).is_err());
        req.n = Some(11);
        assert!(validate(&req).is_err());
        req.n = Some(10);
        assert!(validate(&req).is_ok());
        req.output_compression = Some(101);
        assert!(validate(&req).is_err());
    }

    #[test]
    fn missing_input_images_fail_before_the_request_is_spent() {
        let dir = tempfile::tempdir().unwrap();
        let real = png_fixture(dir.path(), "real.png");
        let mut req = ImageRequest::new("edit it");
        req.images.push(real);
        assert!(validate(&req).is_ok());
        req.images.push(dir.path().join("ghost.png"));
        assert!(validate(&req).is_err());
    }

    #[test]
    fn a_mask_without_input_images_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = ImageRequest::new("edit it");
        req.mask = Some(png_fixture(dir.path(), "mask.png"));
        assert!(validate(&req).is_err());
        req.images.push(png_fixture(dir.path(), "base.png"));
        assert!(validate(&req).is_ok());
    }

    #[test]
    fn a_response_decodes_to_bytes_and_keeps_the_generation_id() {
        let body = serde_json::json!({
            "created": 1,
            "background": "auto",
            "quality": "high",
            "size": "1024x1024",
            "data": [
                { "b64_json": "aGk=", "generation_id": "gen_1" },
                { "b64_json": "eWE=" }
            ]
        })
        .to_string();
        let images = decode_response(&body).unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].bytes, b"hi");
        assert_eq!(images[0].generation_id.as_deref(), Some("gen_1"));
        assert_eq!(images[1].bytes, b"ya");
        assert_eq!(images[1].generation_id, None);
    }

    #[test]
    fn an_empty_or_shapeless_response_is_an_error() {
        assert!(decode_response("{\"data\":[]}").is_err());
        assert!(decode_response("{\"error\":{\"message\":\"nope\"}}").is_err());
        assert!(decode_response("not json").is_err());
    }

    #[test]
    fn a_native_handle_is_distinguishable_from_a_codex_thread_id() {
        let handle = new_handle();
        assert!(is_native_handle(&handle));
        // Codex thread ids are bare UUIDs.
        assert!(!is_native_handle("01a0a794-1234-5678-9abc-def012345678"));
        assert!(!is_native_handle(""));
    }

    #[test]
    fn a_handle_that_could_escape_the_store_is_refused() {
        for evil in ["img-../..", "img-a/b", "img-a\\b", "/img-abs"] {
            assert!(!is_native_handle(evil), "{evil} must not be a handle");
            assert!(handle_dir(evil).is_none(), "{evil} must not resolve");
        }
    }

    #[test]
    fn saved_images_are_numbered_in_the_order_the_api_returned_them() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::paths::fleet_home_guard(tmp.path());
        let handle = new_handle();
        let images = vec![
            ImageBytes {
                bytes: b"first-and-much-longer".to_vec(),
                generation_id: None,
            },
            ImageBytes {
                bytes: b"second".to_vec(),
                generation_id: None,
            },
        ];
        let saved = save_images(&handle, &images, None).unwrap();
        assert_eq!(saved.len(), 2);
        // Order is API order, not size order — the Codex path had to sort by
        // size because it could not tell variants apart; we can.
        assert!(saved[0].path.ends_with("1.png"));
        assert!(saved[1].path.ends_with("2.png"));
        assert_eq!(saved[1].bytes, 6);
        assert_eq!(
            std::fs::read(&saved[0].path).unwrap(),
            b"first-and-much-longer"
        );
    }

    #[test]
    fn jpeg_output_is_saved_with_the_conventional_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::paths::fleet_home_guard(tmp.path());
        let handle = new_handle();
        let images = vec![ImageBytes {
            bytes: b"x".to_vec(),
            generation_id: None,
        }];
        let saved = save_images(&handle, &images, Some("jpeg")).unwrap();
        assert!(saved[0].path.ends_with("1.jpg"), "{}", saved[0].path);
        let saved = save_images(&handle, &images, Some("webp")).unwrap();
        assert!(saved[0].path.ends_with("1.webp"), "{}", saved[0].path);
    }

    #[test]
    fn a_native_handle_reads_back_through_the_existing_image_lookups() {
        // The desktop protocol and both `fleet serve` routes go through
        // `codex_image::{list_thread_images, read_thread_image}`. Neither knows
        // about the new store, and neither should have to.
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::paths::fleet_home_guard(tmp.path());
        let handle = new_handle();
        save_images(
            &handle,
            &[ImageBytes {
                bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
                generation_id: None,
            }],
            None,
        )
        .unwrap();

        let listed = crate::codex_image::list_thread_images(&handle);
        assert_eq!(listed.len(), 1, "native handle must list through codex_image");
        let read = crate::codex_image::read_thread_image(&handle, "1.png").unwrap();
        assert_eq!(read.bytes, b"\x89PNG\r\n\x1a\n");
        assert_eq!(read.mime, "image/png");
    }

    #[test]
    fn tail_quotes_the_end_of_a_long_body_without_splitting_a_char() {
        assert_eq!(tail("short", 400), "short");
        assert_eq!(tail("abcdef", 3), "def");
        // Multi-byte input must not panic on a byte-index slice.
        assert_eq!(tail("错误信息", 2), "信息");
    }
}
