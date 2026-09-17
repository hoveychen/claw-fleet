//! Image generation by borrowing Codex's bundled `imagegen` skill.
//!
//! The Claude side has no image-generation capability; Codex ships an official
//! system skill (`$CODEX_HOME/skills/.system/imagegen`) whose default path is
//! the native `image_gen` tool backed by `gpt-image-2`, billed against the
//! ChatGPT plan quota with no `OPENAI_API_KEY`. This module drives that from
//! Fleet so any session — Claude, Codex, or the desktop — can ask for a raster
//! asset.
//!
//! **Why we locate output by thread id rather than parsing the agent's prose.**
//! Codex's `--json` event stream carries *no* structured event for
//! `image_gen`: a generation turn emits only `thread.started`, `agent_message`,
//! `command_execution` and `turn.*`, and the file path appears solely inside the
//! agent's free-text message. But the built-in tool always writes to
//! `$CODEX_HOME/generated_images/<thread_id>/`, and `thread_id` *is* structured
//! — it is the first line of the stream. So we take the id from
//! [`crate::codex_launch::parse_thread_started`] and read the directory. Zero
//! prose parsing, zero ambiguity.
//!
//! **Host semantics.** This follows
//! [`crate::codex_launch::spawn_new_codex_session`] exactly, including the
//! `wrap_codex_launch` step: for a registered remote workspace the launch is
//! routed through rca, so Codex sees the *real* remote tree rather than the
//! empty local mirror. That matters as soon as the prompt references a
//! workspace file (a reference image, an asset to match) — an unwrapped local
//! Codex simply cannot read it.
//!
//! Output, however, always lands on *this* machine: rca routes syscalls under
//! the workspace path, and `$CODEX_HOME` (`~/.codex`) is not under it. So there
//! is exactly one place to look for generated files regardless of workspace
//! kind.

use std::path::{Path, PathBuf};

/// Extensions the built-in tool can emit. `gpt-image-2` writes PNG in practice;
/// the others are accepted so a format change upstream doesn't silently yield an
/// empty result.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// One image file produced by a generation turn.
///
/// Both `Serialize` and `Deserialize` because this type crosses the
/// `fleet serve` HTTP boundary (see the Backend-trait contract in CLAUDE.md).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GeneratedImage {
    /// Absolute path on the machine that ran Codex.
    pub path: String,
    pub bytes: u64,
}

/// Outcome of one generation turn.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GenerateImageResult {
    /// Codex thread id — also the name of the output directory.
    pub thread_id: String,
    /// Images found in that thread's output dir, largest first (the built-in
    /// tool writes one file per `image_gen` call; a multi-asset prompt yields
    /// several).
    pub images: Vec<GeneratedImage>,
    /// The agent's final message, kept for the cases the images alone don't
    /// explain — a refusal, a clarifying question, or a note that it moved the
    /// file into the workspace.
    pub agent_message: String,
    /// What the agent did during the turn, in order. See [`TurnEvent`] for the
    /// one thing it cannot show you.
    pub timeline: Vec<TurnEvent>,
}

/// `$CODEX_HOME/generated_images/<thread_id>` — where the built-in `image_gen`
/// tool drops output for one thread.
pub fn thread_images_dir(thread_id: &str) -> Option<PathBuf> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return None;
    }
    crate::codex_launch::codex_home().map(|h| h.join("generated_images").join(thread_id))
}

/// Image files directly inside `dir`, largest first.
///
/// Size-descending rather than mtime-descending because a turn that generates
/// several variants writes them within the same second, which makes mtime a
/// coin flip; size is at least stable across reads. Non-image files (the skill
/// leaves none today, but it is not a promise) are skipped.
fn collect_images_in(dir: &Path) -> Vec<GeneratedImage> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<GeneratedImage> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() {
                return None;
            }
            let ext = path
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.to_ascii_lowercase())?;
            if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                return None;
            }
            Some(GeneratedImage {
                path: path.to_string_lossy().to_string(),
                bytes: e.metadata().map(|m| m.len()).unwrap_or(0),
            })
        })
        .collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    out
}

/// Images produced by a given Codex thread, largest first. Empty when the
/// thread generated none (or `$CODEX_HOME` can't be resolved).
pub fn list_thread_images(thread_id: &str) -> Vec<GeneratedImage> {
    thread_images_dir(thread_id)
        .map(|d| collect_images_in(&d))
        .unwrap_or_default()
}

// ── Internal-thread marking ─────────────────────────────────────────────────
//
// A codex_image turn is a *tool invocation*, not a session Boss is having. But
// its rollout lands in the real `$CODEX_HOME/sessions/` stamped
// `originator = "fleet"` (see `apply_codex_launch_env`), which is exactly what
// the scanner uses to recognise a Fleet-owned Codex session. Left unmarked, an
// image turn is indistinguishable from a real one, and every mechanism that
// acts on "a Fleet task session whose turn just ended" fires on it.
//
// Observed live 2026-09-15 on thread 01a0a794 (mslug3-remake): the five
// codex_image turns each correctly ended in plain text — `fleet__ask` is not
// registered on the image launch, so the agent hit `not a function` and fell
// back, which is the headless behaviour we want. Then
// `turn_completion_card::maybe_raise` saw a task session whose process had
// exited without raising a card, put up a "Task Complete" card, and on the answer
// resumed the thread through `agent_source::resume_session` — the *normal*
// session path, which does register the fleet MCP server and does bypass the
// sandbox. That sixth turn dutifully produced a decision card, on a headless
// feature, at a cost of 415K input tokens under `danger-full-access`.
//
// So the fix is not to harden the image prompt — it is to stop claiming the
// thread is a session at all.

/// Marker files naming threads Fleet drove purely as an internal image turn.
/// One empty file per thread id, written after every generate/edit.
fn internal_thread_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("codex-internal-threads"))
}

/// Guards against a thread id escaping the marker directory. Ids are
/// Codex-minted uuids, so anything with a separator in it is not one.
fn internal_thread_path(thread_id: &str) -> Option<PathBuf> {
    let id = thread_id.trim();
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }
    internal_thread_dir().map(|d| d.join(id))
}

/// Record that `thread_id` belongs to the image tool, not to Boss.
///
/// Best-effort: losing the marker degrades to the thread showing up in the
/// session list, which is what happens today, so a write error is logged rather
/// than failing a turn that already produced its image.
pub fn mark_internal_thread(thread_id: &str) {
    let Some(path) = internal_thread_path(thread_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::log_debug(&format!("codex_image: create internal-thread dir: {e}"));
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, b"") {
        crate::log_debug(&format!("codex_image: mark internal thread {thread_id}: {e}"));
    }
}

/// Whether `thread_id` is an internal image thread and must be kept out of the
/// session list.
///
/// Two judges, because the marker only covers turns run after this shipped:
///
/// 1. the marker file, which is authoritative and free;
/// 2. for a thread that has an output directory but no marker — every image
///    thread generated before this change — the rollout's opening prompt. Only
///    [`build_image_prompt`] emits [`IMAGE_PROMPT_PREFIX`] as the first thing a
///    thread ever hears, so this cannot mistake a *real* session that happened
///    to use the built-in `image_gen` tool for an internal one: that session
///    opened with whatever Boss typed.
///
/// The second judge is gated on the output directory existing precisely so the
/// rollout read is bounded to the handful of threads that ever produced an
/// image, rather than every Codex session on the machine.
pub fn is_internal_thread(thread_id: &str) -> bool {
    let Some(marker) = internal_thread_path(thread_id) else {
        return false;
    };
    if marker.exists() {
        return true;
    }
    match thread_images_dir(thread_id) {
        Some(dir) if dir.is_dir() => rollout_opens_with_image_prompt(thread_id),
        _ => false,
    }
}

/// How many rollout lines to read looking for the thread's first user message.
/// Codex writes it at ordinal ~6, behind the session meta, the skills/plugins
/// developer messages, the world state and the turn context.
const ROLLOUT_HEAD_LINES: usize = 40;

/// Does this thread's rollout open with a [`build_image_prompt`] prompt?
///
/// Reads only the head of the file — these rollouts reach tens of MB once the
/// base64 of every attachment is in them, and the answer is always in the first
/// few lines. A `.jsonl.zst` rollout is reported `false` rather than
/// decompressed: compression only happens to archived threads, which are long
/// past being resumed or nagged, so the whole-file decompress buys nothing.
/// Memoises [`rollout_opens_with_image_prompt`]. A rollout's opening prompt is
/// written once and never rewritten, so the answer is immutable for the life of
/// the thread and both polarities are safe to keep — which matters because a
/// *negative* costs a `find_codex_rollout` directory walk, and the scan that
/// asks runs every few seconds. Deliberately not memoising the marker-file
/// check in [`is_internal_thread`]: that one has to stay live, or a thread
/// scanned while its image turn is still running would be cached as "not
/// internal" and stay visible for the rest of the process.
static ROLLOUT_IS_IMAGE_PROMPT: std::sync::Mutex<
    Option<std::collections::HashMap<String, bool>>,
> = std::sync::Mutex::new(None);

fn rollout_opens_with_image_prompt(thread_id: &str) -> bool {
    if let Some(hit) = ROLLOUT_IS_IMAGE_PROMPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|m| m.get(thread_id))
        .copied()
    {
        return hit;
    }
    let answer = read_rollout_opening_prompt(thread_id);
    ROLLOUT_IS_IMAGE_PROMPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(std::collections::HashMap::new)
        .insert(thread_id.to_string(), answer);
    answer
}

fn read_rollout_opening_prompt(thread_id: &str) -> bool {
    use std::io::BufRead;

    let Some(path) = crate::codex_source::find_codex_rollout(thread_id) else {
        return false;
    };
    if path.extension().and_then(|e| e.to_str()) == Some("zst") {
        return false;
    }
    let Ok(file) = std::fs::File::open(&path) else {
        return false;
    };
    for line in std::io::BufReader::new(file).lines().take(ROLLOUT_HEAD_LINES) {
        let Ok(line) = line else { return false };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload = v.get("payload");
        if payload.and_then(|p| p.get("role")).and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let texts = payload.and_then(|p| p.get("content")).and_then(|c| c.as_array());
        for part in texts.into_iter().flatten() {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                if text.starts_with(IMAGE_PROMPT_PREFIX) {
                    return true;
                }
            }
        }
    }
    false
}

/// Raw bytes of one generated image, for the `fleet-genimage://` protocol that
/// renders thumbnails in the desktop.
///
/// Mirrors [`crate::mcp_ipc::DecisionAssetBytes`] rather than reusing it so the
/// two asset stores stay independently evolvable; both cross the `fleet serve`
/// HTTP boundary, hence `Serialize` + `Deserialize`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GeneratedImageBytes {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Read one image out of a thread's generated-images dir by bare filename.
///
/// `name` must be a plain filename: the desktop hands it straight off a URL, so
/// anything with a separator or `..` would be a path-traversal read out of
/// `$CODEX_HOME`. Same guard shape as
/// [`crate::mcp_ipc::read_decision_asset`].
pub fn read_thread_image(thread_id: &str, name: &str) -> Result<GeneratedImageBytes, String> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || Path::new(name).is_absolute()
    {
        return Err(format!("invalid image name '{name}'"));
    }
    let dir = thread_images_dir(thread_id).ok_or("cannot determine codex home")?;
    let path = dir.join(name);
    // Extension gate as well as the name gate: this endpoint is reachable from
    // the webview, and there is no reason for it to serve anything but images.
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| x.to_ascii_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("not an image: '{name}'"));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(GeneratedImageBytes {
        bytes,
        mime: crate::wiki::mime_for_path(&path).to_string(),
    })
}

/// Rules every generation/edit prompt repeats. Two of them are load-bearing for
/// Fleet rather than for the picture: **don't move the output** (Fleet locates
/// files by thread id, so a helpful `mv` into the workspace empties the
/// directory we are about to read) and **don't substitute a vector stand-in**
/// (the skill tells the agent to prefer repo-native SVG for icon-shaped asks,
/// which would silently return no bitmap at all).
const PROMPT_RULES: &str = "Do not substitute SVG, HTML/CSS, or any code-native stand-in, and do \
     not move or copy the generated file anywhere — leave it at its default \
     location and simply report what you made.";

/// Opening words of every prompt codex_image sends. Shared by the two builders
/// below and by [`rollout_opens_with_image_prompt`], which recognises a
/// pre-marker image thread by it — keep them one constant so the recogniser
/// cannot drift away from what the builders actually emit.
const IMAGE_PROMPT_PREFIX: &str = "Use the built-in `image_gen` tool to ";

/// Wrap the caller's description into a prompt that pins the built-in path.
///
/// `attached` says whether `-i` images ride along; they are references for the
/// subject/style, not things to reproduce, and saying so stops the agent from
/// treating a style reference as an edit target.
pub fn build_image_prompt(description: &str, attached: usize) -> String {
    let refs = if attached > 0 {
        format!(
            " The {} attached image(s) are references for style, composition or subject — draw a \
             new image informed by them rather than returning them.",
            attached
        )
    } else {
        String::new()
    };
    format!(
        "{IMAGE_PROMPT_PREFIX}generate the following image.{refs} {PROMPT_RULES}\n\n{}",
        description.trim()
    )
}

/// Prompt for a follow-up turn on an existing thread.
///
/// The thread already has the previous image in context, which is exactly what
/// the built-in edit path needs (its skill only edits images *visible in the
/// conversation*). Invariants are restated every round because the skill's own
/// guidance says edits drift otherwise.
pub fn build_edit_prompt(instruction: &str, attached: usize) -> String {
    let refs = if attached > 0 {
        format!(
            " {} new reference image(s) are attached for this revision.",
            attached
        )
    } else {
        String::new()
    };
    format!(
        "{IMAGE_PROMPT_PREFIX}revise the image you generated earlier in this \
         conversation. Change only what the instruction below asks for and keep everything else \
         unchanged.{refs} {PROMPT_RULES}\n\n{}",
        instruction.trim()
    )
}

/// One step the agent took during a turn, in stream order.
///
/// This is the answer to "what did it actually do in there" — the thing you
/// need when a revision comes back wrong and you want to know whether the agent
/// misread the instruction or the tool misbehaved.
///
/// **Known blind spot:** the `image_gen` call itself emits no event (verified
/// against the live stream), so the timeline shows the agent reading its skill,
/// running `sips`, and narrating — but never the generation parameters it
/// passed. Those are unobservable from outside Codex.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TurnEvent {
    /// `message` (the agent talking) or `command` (a shell command it ran).
    pub kind: String,
    /// The message text, or the command line.
    pub text: String,
}

/// Truncation cap for a single command's captured output. A `sed` of the whole
/// SKILL.md is ~10 KB of noise; the first line or two is what identifies it.
const EVENT_TEXT_CAP: usize = 200;

/// Every agent message and command execution in a `codex exec --json` capture,
/// in stream order.
///
/// Lines that don't parse, or carry no `item`, are skipped rather than aborting
/// — a turn's stream interleaves several event shapes and gains new ones across
/// Codex versions.
pub fn parse_turn_timeline(stdout: &str) -> Vec<TurnEvent> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Only `item.completed`: `item.started` carries the same command with a
        // null exit code, so taking both would double every entry.
        if v.get("type").and_then(|t| t.as_str()) != Some("item.completed") {
            continue;
        }
        let Some(item) = v.get("item") else { continue };
        let (kind, text) = match item.get("type").and_then(|t| t.as_str()) {
            Some("agent_message") => ("message", item.get("text").and_then(|t| t.as_str())),
            Some("command_execution") => ("command", item.get("command").and_then(|c| c.as_str())),
            _ => continue,
        };
        let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
            continue;
        };
        out.push(TurnEvent {
            kind: kind.to_string(),
            text: truncate(text, EVENT_TEXT_CAP),
        });
    }
    out
}

/// Cap `s` at `max` characters, marking that it was cut. Character-based, not
/// byte-based: a byte slice would panic mid-UTF-8 on a CJK prompt.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max).collect::<String>())
}

/// Last `agent_message` item in a `codex exec --json` stdout capture.
///
/// The stream interleaves reasoning, command executions and messages; the final
/// message is the agent's own summary of the turn.
pub fn parse_last_agent_message(stdout: &str) -> Option<String> {
    let mut last = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // `continue`, not `?` — a stream line without an `item` (e.g.
        // `thread.started`) must be skipped, not abort the whole scan.
        let Some(item) = v.get("item") else {
            continue;
        };
        if item.get("type").and_then(|t| t.as_str()) != Some("agent_message") {
            continue;
        }
        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
            last = Some(text.to_string());
        }
    }
    last
}

/// Why a `codex exec --json` turn failed, if it did.
///
/// A failed turn still prints `thread.started` on stdout and *then* fails, so
/// neither the exit code check in [`run_turn`] (which needs stdout to be empty)
/// nor the thread-id lookup catches it. The two shapes Codex 0.153 emits,
/// measured:
///
/// ```text
/// {"type":"error","message":"Selected model is at capacity. …"}
/// {"type":"turn.failed","error":{"message":"Selected model is at capacity. …"}}
/// ```
///
/// Both are accepted (either alone is enough) and the last one wins, so a turn
/// that recovered from an early error and failed later reports the failure that
/// actually ended it.
pub fn parse_turn_failure(stdout: &str) -> Option<String> {
    let mut last = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let message = match v.get("type").and_then(|t| t.as_str()) {
            Some("error") => v.get("message").and_then(|m| m.as_str()),
            Some("turn.failed") => v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str()),
            _ => continue,
        };
        if let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) {
            last = Some(message.to_string());
        }
    }
    last
}

/// Assemble the exact `(program, argv, env)` a generation turn launches with.
///
/// Split out from [`generate_image`] so the launch shape is testable without
/// spending a turn — in particular that the rca wrap is applied. Skipping that
/// wrap would hand Codex the *empty local mirror* of a remote workspace instead
/// of the real remote tree, making any workspace file the prompt references
/// (a reference image, an asset to match) invisible to it. Local workspaces
/// pass through unchanged with no extra env.
fn build_generate_launch(
    codex: PathBuf,
    workspace_path: &str,
    description: &str,
    images: &[String],
    model: Option<&str>,
) -> Result<(PathBuf, Vec<String>, Vec<(String, String)>), String> {
    let prompt = build_image_prompt(description, images.len());
    let args = crate::codex_launch::build_codex_exec_args(
        workspace_path,
        &prompt,
        Some(model.unwrap_or(DEFAULT_ROUTING_MODEL)),
        None,
        &generate_pre_prompt_args(images),
    );
    crate::codex_launch::wrap_codex_launch(codex, args, workspace_path)
}

/// Same, for a follow-up turn on `thread_id` via `codex exec resume`.
///
/// `-i` works identically here: despite the help text rendering it `<FILE>`
/// rather than `<FILE>...`, repeated `-i` flags parse fine on `resume` (verified
/// against the CLI), so [`crate::codex_launch::codex_image_args`] serves both
/// paths unchanged.
fn build_edit_launch(
    codex: PathBuf,
    workspace_path: &str,
    thread_id: &str,
    instruction: &str,
    images: &[String],
    model: Option<&str>,
) -> Result<(PathBuf, Vec<String>, Vec<(String, String)>), String> {
    let prompt = build_edit_prompt(instruction, images.len());
    let args = crate::codex_launch::build_codex_resume_args(
        thread_id,
        &prompt,
        Some(model.unwrap_or(DEFAULT_ROUTING_MODEL)),
        None,
        &edit_pre_prompt_args(images),
    );
    crate::codex_launch::wrap_codex_launch(codex, args, workspace_path)
}

/// Flags for a **new** turn, in front of the `--` separator.
///
/// `workspace-write` (not read-only): the skill inspects its own SKILL.md and
/// may run `sips`-style checks on what it produced. No decision-card / notify
/// bridging — this is a one-shot tool call, not a Fleet-owned session, so it
/// must not register itself as one.
fn generate_pre_prompt_args(images: &[String]) -> Vec<String> {
    let mut args = vec!["-s".to_string(), "workspace-write".to_string()];
    args.extend(crate::codex_launch::codex_image_args(images));
    args
}

/// Flags for a **resume** turn.
///
/// Deliberately no `-s`: `codex exec resume` has no `--sandbox` flag at all and
/// hard-errors with `unexpected argument '-s' found` if handed one (caught by
/// the live e2e — the arg-shape unit tests happily asserted a command line Codex
/// rejects). A resumed thread keeps the sandbox policy it was created with,
/// which is the `workspace-write` set by [`generate_pre_prompt_args`], so
/// nothing is lost.
fn edit_pre_prompt_args(images: &[String]) -> Vec<String> {
    crate::codex_launch::codex_image_args(images)
}

/// Images in `after` that were not already in `before`, keeping `after`'s order.
///
/// A follow-up turn writes into the *same* `generated_images/<thread_id>/`
/// directory as the turn that created the thread, so "what did this round
/// produce" is only answerable by snapshotting before and diffing after. Keyed
/// on path: the built-in tool never overwrites, it writes a fresh `exec-<uuid>`.
fn new_images_since(before: &[GeneratedImage], after: Vec<GeneratedImage>) -> Vec<GeneratedImage> {
    let seen: std::collections::HashSet<&str> = before.iter().map(|i| i.path.as_str()).collect();
    after
        .into_iter()
        .filter(|i| !seen.contains(i.path.as_str()))
        .collect()
}

/// Reject image paths that don't exist before paying for a turn — Codex would
/// otherwise burn a full turn and fail deep inside the agent.
fn validate_images(images: &[String]) -> Result<(), String> {
    for path in images.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !Path::new(path).is_file() {
            return Err(format!("attachment not found: {path}"));
        }
    }
    Ok(())
}

/// Run one blocking generation turn on a fresh thread.
///
/// Blocking (not detached like [`crate::codex_launch::spawn_new_codex_session`])
/// because the caller is a tool invocation that must hand back paths. A turn
/// takes tens of seconds; callers on a UI thread must move this off it.
///
/// `images` are attached with `-i` and act as references for style, composition
/// or subject. `model` selects only the *routing* model (which `gpt-5.6` drives
/// the turn) — generation is always `gpt-image-2` on Codex's backend — so the
/// default is the cheap tier.
pub fn generate_image(
    workspace_path: &str,
    description: &str,
    images: &[String],
    model: Option<&str>,
) -> Result<GenerateImageResult, String> {
    let description = description.trim();
    if description.is_empty() {
        return Err("description is required".to_string());
    }
    let workspace_path = prepare_workspace(workspace_path)?;
    validate_images(images)?;
    let codex = resolve_codex()?;

    let (program, args, rca_envs) =
        build_generate_launch(codex, &workspace_path, description, images, model)?;
    let stdout = run_turn(&program, &args, &rca_envs, &workspace_path)?;

    let thread_id = stdout
        .lines()
        .find_map(crate::codex_launch::parse_thread_started)
        .ok_or_else(|| match parse_turn_failure(&stdout) {
            // Failing before the thread even starts is the auth/config shape of
            // the same problem; name it rather than dumping the raw stream.
            Some(reason) => format!("codex turn failed before it started: {reason}"),
            None => format!("codex never printed thread.started; output: {}", tail(&stdout)),
        })?;

    // `run_turn` already claimed it off the `thread.started` line; re-assert so
    // the claim does not depend on that parse having gone the way we expect.
    mark_internal_thread(&thread_id);

    // Fresh thread: everything in the directory is this turn's output.
    finish_turn(thread_id, &[], &stdout)
}

/// Run a follow-up turn on an existing thread — the "keep tweaking until it's
/// right" path.
///
/// The previous image is already in the thread's context, which is what the
/// built-in edit path requires (it can only edit images visible in the
/// conversation). Output lands in the *same* directory as the original turn, so
/// this snapshots it first and reports only what the round added.
pub fn edit_image(
    workspace_path: &str,
    thread_id: &str,
    instruction: &str,
    images: &[String],
    model: Option<&str>,
) -> Result<GenerateImageResult, String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err("thread_id is required".to_string());
    }
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return Err("instruction is required".to_string());
    }
    let workspace_path = prepare_workspace(workspace_path)?;
    validate_images(images)?;
    let codex = resolve_codex()?;

    // Snapshot BEFORE the turn — the diff is the only way to tell this round's
    // output from earlier rounds' in a shared directory.
    let before = list_thread_images(thread_id);

    // Re-assert on every round: an edit is the one path that can reach a thread
    // whose marker predates this change, or was cleaned out from under us.
    mark_internal_thread(thread_id);

    let (program, args, rca_envs) =
        build_edit_launch(codex, &workspace_path, thread_id, instruction, images, model)?;
    let stdout = run_turn(&program, &args, &rca_envs, &workspace_path)?;

    finish_turn(thread_id.to_string(), &before, &stdout)
}

/// Shared tail of both paths: diff out this round's images, or explain why there
/// were none.
fn finish_turn(
    thread_id: String,
    before: &[GeneratedImage],
    stdout: &str,
) -> Result<GenerateImageResult, String> {
    let images = new_images_since(before, list_thread_images(&thread_id));
    let agent_message = parse_last_agent_message(stdout).unwrap_or_default();

    if images.is_empty() {
        // A turn that never reached the agent at all — out of credits, rate
        // limited, auth expired, model at capacity — says so in an `error` /
        // `turn.failed` event and emits no `agent_message`. Reported as
        // "Agent said: (nothing)" that reads like the agent declined, which is
        // what sent a session hunting for a prompt problem on 2026-09-09 while
        // every model on the account was answering "Selected model is at
        // capacity". Codex's own words first, always.
        if let Some(reason) = parse_turn_failure(stdout) {
            return Err(format!("codex turn failed for thread {thread_id}: {reason}"));
        }
        // Otherwise the turn really did end cleanly having generated nothing —
        // the agent asked a question, refused, or (despite the wrapper)
        // substituted an SVG. Its message is the only explanation, so surface
        // it instead of a bare "no images".
        return Err(format!(
            "codex produced no image for thread {thread_id}. Agent said: {}",
            if agent_message.trim().is_empty() {
                "(nothing)"
            } else {
                agent_message.trim()
            }
        ));
    }

    Ok(GenerateImageResult {
        thread_id,
        images,
        agent_message,
        timeline: parse_turn_timeline(stdout),
    })
}

/// Normalize + materialize the workspace (a registered remote workspace's local
/// mirror may not exist yet).
fn prepare_workspace(workspace_path: &str) -> Result<String, String> {
    let workspace_path = workspace_path.trim();
    if workspace_path.is_empty() {
        return Err("workspace_path is required".to_string());
    }
    crate::remote_workspace::ensure_local_mirror(workspace_path)?;
    if !Path::new(workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {workspace_path}"));
    }
    Ok(workspace_path.to_string())
}

fn resolve_codex() -> Result<PathBuf, String> {
    crate::codex_source::find_codex_binary().ok_or_else(|| {
        "Codex CLI not found (no standalone install, VSCode extension, or `codex` on PATH)"
            .to_string()
    })
}

/// Spawn the turn and capture its `--json` stdout, claiming the thread as
/// internal the instant Codex names it.
///
/// Reads stdout line by line rather than taking `Command::output()`, purely so
/// the `thread.started` line — Codex's first — can mark the thread while the
/// turn is still running. Buffering to exit would leave the thread unmarked for
/// the whole turn, and a turn that then fails produces no `generated_images`
/// directory either, so nothing would identify it as internal until the call
/// returned: a window in which a scan could mint it as a `SessionInfo` and the
/// turn-completion card could fire on it. Marking off the first line closes it.
///
/// **stderr must be drained on its own thread.** It is piped (the failure path
/// below reports it) and Fleet sets `RUST_LOG` on every Codex child, so the
/// transport trace alone can fill the pipe buffer — a child blocked writing
/// stderr never closes stdout, and this function would wait forever.
fn run_turn(
    program: &Path,
    args: &[String],
    rca_envs: &[(String, String)],
    workspace_path: &str,
) -> Result<String, String> {
    use std::io::{BufRead, Read};

    let mut cmd = crate::process_util::command(program);
    cmd.args(args)
        .current_dir(workspace_path)
        // MUST be null: `codex exec` otherwise blocks reading stdin forever.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::codex_launch::apply_codex_launch_env(&mut cmd);
    for (k, v) in rca_envs {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", program.display()))?;

    let mut stderr_pipe = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let mut stdout = String::new();
    if let Some(pipe) = child.stdout.take() {
        let mut reader = std::io::BufReader::new(pipe);
        let mut marked = false;
        let mut raw = Vec::new();
        loop {
            raw.clear();
            // Bytes, not `read_line`: the previous `output()` call decoded the
            // whole stream with `from_utf8_lossy`, and a `read_line` that hit
            // invalid UTF-8 would error and silently truncate the turn's events
            // instead. Decoding per line keeps the old tolerance.
            match reader.read_until(b'\n', &mut raw) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&raw);
            if !marked {
                if let Some(id) = crate::codex_launch::parse_thread_started(&line) {
                    mark_internal_thread(&id);
                    marked = true;
                }
            }
            stdout.push_str(&line);
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("wait {}: {e}", program.display()))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() && stdout.is_empty() {
        return Err(format!(
            "codex exited {:?}: {}",
            status.code(),
            tail(&String::from_utf8_lossy(&stderr))
        ));
    }
    Ok(stdout)
}

/// Last few hundred chars — enough to identify a failure without pasting a whole
/// event stream into an error message.
fn tail(s: &str) -> String {
    let s = s.trim();
    const MAX: usize = 400;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let skip = s.chars().count() - MAX;
    format!("…{}", s.chars().skip(skip).collect::<String>())
}

/// Routing default: generation quality is set by `gpt-image-2` regardless, so
/// the cheap tier drives.
pub const DEFAULT_ROUTING_MODEL: &str = "gpt-5.6-luna";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_images_dir_lands_under_codex_home() {
        let dir = thread_images_dir("01a06ef6-3f80-7ba3-9645-8e85fdf15d4a")
            .expect("codex home resolvable in test env");
        let s = dir.to_string_lossy();
        assert!(
            s.contains("generated_images"),
            "must go through the generated_images dir, got {s}"
        );
        assert!(
            s.ends_with("01a06ef6-3f80-7ba3-9645-8e85fdf15d4a"),
            "must end with the thread id, got {s}"
        );
    }

    #[test]
    fn thread_images_dir_rejects_blank_id() {
        assert!(thread_images_dir("").is_none());
        assert!(thread_images_dir("   ").is_none());
    }

    #[test]
    fn read_thread_image_rejects_traversal_and_non_images() {
        // This endpoint is reachable from the webview off a URL segment, so the
        // name gate is a security boundary, not tidiness.
        for bad in ["", "../../.codex/auth.json", "a/b.png", "..", "notes.txt"] {
            assert!(
                read_thread_image("some-thread", bad).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn collect_images_skips_non_images_and_sorts_largest_first() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("small.png"), vec![0u8; 10]).unwrap();
        std::fs::write(td.path().join("big.png"), vec![0u8; 100]).unwrap();
        std::fs::write(td.path().join("notes.txt"), b"not an image").unwrap();
        std::fs::create_dir(td.path().join("subdir")).unwrap();

        let got = collect_images_in(td.path());
        assert_eq!(got.len(), 2, "only the two images, got {got:?}");
        assert!(got[0].path.ends_with("big.png"), "largest first: {got:?}");
        assert_eq!(got[0].bytes, 100);
        assert!(got[1].path.ends_with("small.png"));
    }

    #[test]
    fn collect_images_accepts_uppercase_extensions() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("hero.PNG"), vec![0u8; 5]).unwrap();
        assert_eq!(collect_images_in(td.path()).len(), 1);
    }

    #[test]
    fn collect_images_on_missing_dir_is_empty_not_panic() {
        assert!(collect_images_in(Path::new("/definitely/not/here/xyz")).is_empty());
    }

    #[test]
    fn prompt_pins_the_builtin_tool_and_forbids_moving() {
        let p = build_image_prompt("  a shiba in a red scarf  ", 0);
        assert!(p.contains("image_gen"), "must name the built-in tool: {p}");
        assert!(p.contains("a shiba in a red scarf"), "must carry the description: {p}");
        assert!(!p.contains("  a shiba"), "description must be trimmed: {p}");
        // Fleet locates output by thread id; a helpful `mv` would empty the dir.
        assert!(p.contains("not move"), "must forbid moving the output: {p}");
        assert!(p.contains("SVG"), "must forbid the vector substitution: {p}");
    }

    /// The pre-marker recogniser matches on [`IMAGE_PROMPT_PREFIX`], so a
    /// builder that stops emitting it would silently put every historical image
    /// thread back in the session list.
    #[test]
    fn both_prompts_open_with_the_recognisable_prefix() {
        assert!(
            build_image_prompt("a shiba", 0).starts_with(IMAGE_PROMPT_PREFIX),
            "generate prompt must open with the prefix the recogniser looks for"
        );
        assert!(
            build_edit_prompt("make it blue", 0).starts_with(IMAGE_PROMPT_PREFIX),
            "edit prompt must open with the prefix the recogniser looks for"
        );
    }

    /// `run_turn` must claim the thread off the `thread.started` line *while the
    /// turn runs*, and must not deadlock when the child floods stderr — Fleet
    /// sets `RUST_LOG` on every Codex child, so a full stderr pipe with no
    /// reader is the realistic failure, not a hypothetical one.
    #[cfg(unix)]
    #[test]
    fn run_turn_marks_the_thread_mid_turn_and_survives_a_stderr_flood() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!(
            "fleet-run-turn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // `FLEET_HOME` is process-global and the child here runs for seconds
        // (4 MiB of stderr), so a sibling test flipping the var mid-turn would
        // land the marker in *its* temp dir and this test would fail looking
        // for a file that was written elsewhere. Every other FLEET_HOME test in
        // the crate serialises on this lock; this one used to be the exception.
        let _env_guard = crate::session::fleet_home_lock();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let thread_id = "01a0a794-3dcd-7d90-9ea1-fbe5bb453386";
        // 4 MiB of stderr, far past any pipe buffer, written *before* the child
        // finishes its stdout — the exact shape that hangs an undrained pipe.
        let script = tmp.join("fake-codex.sh");
        let mut f = std::fs::File::create(&script).unwrap();
        writeln!(
            f,
            "#!/bin/sh\n\
             echo '{{\"type\":\"thread.started\",\"thread_id\":\"{thread_id}\"}}'\n\
             i=0; while [ $i -lt 4096 ]; do\n\
               head -c 1024 /dev/zero | tr '\\0' 'x' >&2; echo >&2; i=$((i+1));\n\
             done\n\
             echo '{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"ok\"}}}}'\n"
        )
        .unwrap();
        drop(f);
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let out = run_turn(&script, &[], &[], tmp.to_str().unwrap())
            .expect("a child that floods stderr must still complete");

        assert!(out.contains("thread.started"), "stdout must be captured: {out}");
        assert!(
            out.contains("agent_message"),
            "stdout written after the stderr flood must survive: {out}"
        );
        assert!(
            internal_thread_path(thread_id).unwrap().exists(),
            "the thread must be claimed off its thread.started line"
        );

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The marker path is built from an id that reaches us through an MCP tool
    /// argument, so it must not be able to name a file outside the dir.
    #[test]
    fn internal_thread_path_rejects_ids_that_could_escape() {
        for bad in ["", "  ", "../../etc/passwd", "a/b", "a\\b"] {
            assert!(
                internal_thread_path(bad).is_none(),
                "must refuse {bad:?} as a thread id"
            );
        }
        // A real Codex thread id resolves.
        assert!(internal_thread_path("01a0a794-3dcd-7d90-9ea1-fbe5bb453386").is_some());
    }

    #[test]
    fn last_agent_message_wins_over_earlier_ones() {
        let stdout = concat!(
            r#"{"type":"thread.started","thread_id":"abc"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"starting"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"ls"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"done, saved it"}}"#,
            "\n",
        );
        assert_eq!(
            parse_last_agent_message(stdout).as_deref(),
            Some("done, saved it")
        );
    }

    /// Verbatim capture of `codex exec --json -m gpt-5.6-luna` on 2026-09-09,
    /// when every model on the account answered "at capacity". This exact stream
    /// used to surface as "produced no image … Agent said: (nothing)".
    const CAPACITY_FAILURE_STDOUT: &str = concat!(
        r#"{"type":"thread.started","thread_id":"01a08968-1a48-7483-89c7-396316522686"}"#,
        "\n",
        r#"{"type":"turn.started"}"#,
        "\n",
        r#"{"type":"error","message":"Selected model is at capacity. Please try a different model."}"#,
        "\n",
        r#"{"type":"turn.failed","error":{"message":"Selected model is at capacity. Please try a different model."}}"#,
        "\n",
    );

    #[test]
    fn turn_failure_is_read_off_the_real_capacity_stream() {
        assert_eq!(
            parse_turn_failure(CAPACITY_FAILURE_STDOUT).as_deref(),
            Some("Selected model is at capacity. Please try a different model.")
        );
    }

    #[test]
    fn turn_failure_accepts_either_shape_alone() {
        assert_eq!(
            parse_turn_failure(r#"{"type":"error","message":"out of credits"}"#).as_deref(),
            Some("out of credits")
        );
        assert_eq!(
            parse_turn_failure(r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#)
                .as_deref(),
            Some("rate limited")
        );
    }

    #[test]
    fn turn_failure_is_none_on_a_healthy_turn() {
        let ok = concat!(
            r#"{"type":"thread.started","thread_id":"abc"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{}}"#,
        );
        assert_eq!(parse_turn_failure(ok), None);
    }

    #[test]
    fn empty_turn_reports_codex_failure_over_the_agent_silence() {
        // The whole point: a failed turn must never be reported as the agent
        // having produced nothing to say.
        let err = finish_turn("t1".to_string(), &[], CAPACITY_FAILURE_STDOUT)
            .expect_err("no images means Err");
        assert!(err.contains("at capacity"), "must carry codex's reason: {err}");
        assert!(
            !err.contains("(nothing)"),
            "must not fall through to the agent-silence wording: {err}"
        );
    }

    #[test]
    fn empty_turn_without_a_failure_still_reports_agent_silence() {
        let clean = concat!(
            r#"{"type":"thread.started","thread_id":"t2"}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{}}"#,
        );
        let err = finish_turn("t2".to_string(), &[], clean).expect_err("no images means Err");
        assert!(err.contains("(nothing)"), "unchanged for a clean empty turn: {err}");
    }

    #[test]
    fn timeline_keeps_messages_and_commands_in_order() {
        let stdout = concat!(
            r#"{"type":"thread.started","thread_id":"abc"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"starting"}}"#,
            "\n",
            r#"{"type":"item.started","item":{"type":"command_execution","command":"sips -z 1024 1024 a.png","exit_code":null}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"sips -z 1024 1024 a.png","exit_code":0}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"reasoning","text":"hmm"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{}}"#,
        );
        let got = parse_turn_timeline(stdout);
        assert_eq!(
            got,
            vec![
                TurnEvent { kind: "message".into(), text: "starting".into() },
                TurnEvent { kind: "command".into(), text: "sips -z 1024 1024 a.png".into() },
                TurnEvent { kind: "message".into(), text: "done".into() },
            ],
            "item.started must not double the command; non-message/command items are skipped"
        );
    }

    #[test]
    fn timeline_truncates_on_char_boundaries_not_bytes() {
        // A CJK command line would panic a byte-slice truncation.
        let long = "画".repeat(EVENT_TEXT_CAP + 50);
        let line = serde_json::json!({
            "type": "item.completed",
            "item": { "type": "command_execution", "command": long }
        })
        .to_string();
        let got = parse_turn_timeline(&line);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].text.chars().count(),
            EVENT_TEXT_CAP + 1,
            "capped text plus the ellipsis marker"
        );
        assert!(got[0].text.ends_with('…'));
    }

    #[test]
    fn timeline_survives_garbage_and_empty_input() {
        assert!(parse_turn_timeline("").is_empty());
        assert!(parse_turn_timeline("not json\n{broken\n").is_empty());
        // An item with no text/command contributes nothing rather than a blank row.
        assert!(parse_turn_timeline(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"   "}}"#
        )
        .is_empty());
    }

    #[test]
    fn agent_message_absent_or_garbage_is_none_not_panic() {
        assert!(parse_last_agent_message("").is_none());
        assert!(parse_last_agent_message("not json\n{broken").is_none());
        assert!(
            parse_last_agent_message(r#"{"type":"thread.started","thread_id":"abc"}"#).is_none(),
            "a stream with no agent_message yields None"
        );
    }

    #[test]
    fn launch_goes_through_the_rca_wrap_and_passes_local_through() {
        // The wrap is the whole remote-parity contract: without it a remote
        // workspace hands Codex an empty local mirror. Assert it is on the path
        // (a local workspace must come back untouched, with no rca env).
        let td = tempfile::tempdir().unwrap();
        let ws = td.path().to_string_lossy().to_string();
        let codex = PathBuf::from("/usr/local/bin/codex");
        let (program, args, envs) =
            build_generate_launch(codex.clone(), &ws, "draw a cat", &[], None).expect("local launch");

        assert_eq!(program, codex, "local workspace must not be re-programmed");
        assert!(envs.is_empty(), "local workspace needs no rca env, got {envs:?}");
        assert_eq!(args.first().map(String::as_str), Some("exec"));
        assert!(args.iter().any(|a| a == "--json"), "must stay machine-readable");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-s" && w[1] == "workspace-write"),
            "skill needs workspace-write to inspect its own output: {args:?}"
        );
        // The prompt is last, after `--`, and carries the wrapper.
        let last = args.last().expect("prompt");
        assert!(last.contains("image_gen") && last.contains("draw a cat"), "{last}");
    }

    #[test]
    fn launch_defaults_to_the_cheap_routing_model() {
        let td = tempfile::tempdir().unwrap();
        let ws = td.path().to_string_lossy().to_string();
        let (_, args, _) =
            build_generate_launch(PathBuf::from("/usr/local/bin/codex"), &ws, "x", &[], None).unwrap();
        assert!(
            args.iter().any(|a| a == DEFAULT_ROUTING_MODEL),
            "generation quality comes from gpt-image-2 regardless, so the cheap tier drives: {args:?}"
        );
    }

    /// Two real files to attach — `validate_images` rejects paths that don't
    /// exist, so tests that exercise the `-i` path need actual files.
    fn two_attachments(td: &tempfile::TempDir) -> (String, String, Vec<String>) {
        let a = td.path().join("ref-a.png");
        let b = td.path().join("ref-b.png");
        std::fs::write(&a, vec![0u8; 4]).unwrap();
        std::fs::write(&b, vec![0u8; 4]).unwrap();
        let (sa, sb) = (
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        );
        (sa.clone(), sb.clone(), vec![sa, sb])
    }

    #[test]
    fn every_attachment_becomes_its_own_dash_i_flag() {
        let td = tempfile::tempdir().unwrap();
        let ws = td.path().to_string_lossy().to_string();
        let (a, b, images) = two_attachments(&td);
        let (_, args, _) =
            build_generate_launch(PathBuf::from("/usr/local/bin/codex"), &ws, "x", &images, None)
                .unwrap();
        let flags: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0 && args[i - 1] == "-i")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(flags, vec![&a, &b], "both attachments must ride along: {args:?}");
        // Flags must precede `--`, or codex parses them as prompt text.
        let sep = args.iter().position(|a| a == "--").expect("-- separator");
        let last_i = args.iter().rposition(|a| a == "-i").expect("-i present");
        assert!(last_i < sep, "-i must come before `--`: {args:?}");
    }

    #[test]
    fn edit_launch_resumes_the_thread_and_keeps_attachments() {
        let td = tempfile::tempdir().unwrap();
        let ws = td.path().to_string_lossy().to_string();
        let (_, _, images) = two_attachments(&td);
        let (program, args, _) = build_edit_launch(
            PathBuf::from("/usr/local/bin/codex"),
            &ws,
            "01a06fc8-4aee-7eb0-a158-b05cbde17e92",
            "make the scarf blue",
            &images,
            None,
        )
        .unwrap();
        assert_eq!(program, PathBuf::from("/usr/local/bin/codex"));
        assert_eq!(&args[..2], &["exec".to_string(), "resume".to_string()][..]);
        assert_eq!(args[2], "01a06fc8-4aee-7eb0-a158-b05cbde17e92");
        // Repeated `-i` parses on `resume` too, despite the help rendering it
        // `<FILE>` rather than `<FILE>...` — verified against the CLI.
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 2, "{args:?}");
        let last = args.last().unwrap();
        assert!(last.contains("make the scarf blue"), "{last}");
        assert!(
            last.contains("Change only what the instruction below asks for"),
            "edit prompt must restate invariants: {last}"
        );
        // Regression guard for a bug only the live e2e caught: `codex exec
        // resume` has no --sandbox flag and hard-errors on `-s`. The resumed
        // thread keeps the policy it was created with, so there is nothing to
        // re-specify.
        assert!(
            !args.iter().any(|a| a == "-s"),
            "resume rejects -s outright: {args:?}"
        );
    }

    #[test]
    fn new_images_since_reports_only_this_rounds_output() {
        let img = |p: &str, b: u64| GeneratedImage {
            path: p.to_string(),
            bytes: b,
        };
        let before = vec![img("/d/one.png", 10)];
        let after = vec![img("/d/two.png", 30), img("/d/one.png", 10)];
        let got = new_images_since(&before, after);
        assert_eq!(got, vec![img("/d/two.png", 30)], "round 1's image must not resurface");
    }

    #[test]
    fn new_images_since_on_a_fresh_thread_returns_everything() {
        let after = vec![GeneratedImage {
            path: "/d/one.png".into(),
            bytes: 10,
        }];
        assert_eq!(new_images_since(&[], after.clone()), after);
    }

    #[test]
    fn missing_attachment_fails_before_paying_for_a_turn() {
        let err = validate_images(&["/definitely/not/here/ref.png".to_string()])
            .expect_err("nonexistent attachment must be rejected");
        assert!(err.contains("attachment not found"), "got {err}");
        // Blank entries are dropped by codex_image_args, so they must not trip
        // the validator either.
        assert!(validate_images(&["".to_string(), "   ".to_string()]).is_ok());
    }

    #[test]
    fn edit_rejects_blank_thread_or_instruction_before_spawning() {
        assert!(edit_image("/tmp", "  ", "do a thing", &[], None).is_err());
        assert!(edit_image("/tmp", "some-thread", "   ", &[], None).is_err());
    }

    #[test]
    fn attached_references_are_announced_in_both_prompts() {
        // Without this the agent can mistake a style reference for an edit
        // target and hand back a near-copy of the input.
        let gen = build_image_prompt("a shiba", 2);
        assert!(gen.contains("2 attached image(s) are references"), "{gen}");
        assert!(!build_image_prompt("a shiba", 0).contains("attached"));

        let edit = build_edit_prompt("bluer", 1);
        assert!(edit.contains("1 new reference image(s)"), "{edit}");
        assert!(!build_edit_prompt("bluer", 0).contains("reference image"));
    }

    #[test]
    fn generate_rejects_blank_inputs_before_spawning() {
        // Guard rails must fire on argument shape, not after paying for a turn.
        assert!(generate_image("/tmp", "   ", &[], None).is_err());
        assert!(generate_image("  ", "draw a cat", &[], None).is_err());
    }

    #[test]
    fn generate_rejects_missing_workspace() {
        let err = generate_image("/definitely/not/here/xyz", "draw a cat", &[], None)
            .expect_err("missing workspace must fail");
        assert!(err.contains("not found"), "got {err}");
    }
}
