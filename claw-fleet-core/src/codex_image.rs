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
//! **Host semantics.** Like [`crate::codex_launch::spawn_new_codex_session`],
//! the Codex binary resolved by [`crate::codex_source::find_codex_binary`] is
//! always the *local* one, and a registered remote workspace has a local mirror
//! at the identical path (`remote_workspace::ensure_local_mirror`). Generation
//! therefore always happens on this machine and output always lands in this
//! machine's `$CODEX_HOME` — the same contract every other Fleet-driven Codex
//! turn already follows. There is no second host to choose.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Wrap the caller's description into a prompt that pins the built-in path.
///
/// Left to its own devices the agent may substitute an SVG or an HTML/CSS
/// mockup — its skill explicitly tells it to prefer repo-native vectors for
/// icon-shaped requests — so the wrapper names the tool. It also tells the
/// agent *not* to move the output: Fleet locates files by thread id, and a
/// helpful `mv` into the workspace would empty the directory we are about to
/// read.
pub fn build_image_prompt(description: &str) -> String {
    format!(
        "Use the built-in `image_gen` tool to generate the following image. \
         Do not substitute SVG, HTML/CSS, or any code-native stand-in, and do \
         not move or copy the generated file anywhere — leave it at its default \
         location and simply report what you made.\n\n{}",
        description.trim()
    )
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

/// Run one blocking generation turn and return whatever it produced.
///
/// Blocking (not detached like [`crate::codex_launch::spawn_new_codex_session`])
/// because the caller is a tool invocation that must hand back paths. A turn
/// takes tens of seconds; callers on a UI thread must move this off it.
///
/// `model` selects only the *routing* model (which `gpt-5.6` drives the turn) —
/// generation is always `gpt-image-2` on Codex's backend — so the default is the
/// cheap tier.
pub fn generate_image(
    workspace_path: &str,
    description: &str,
    model: Option<&str>,
) -> Result<GenerateImageResult, String> {
    let description = description.trim();
    if description.is_empty() {
        return Err("description is required".to_string());
    }
    let workspace_path = workspace_path.trim();
    if workspace_path.is_empty() {
        return Err("workspace_path is required".to_string());
    }
    // A registered remote workspace's local mirror may not exist yet.
    crate::remote_workspace::ensure_local_mirror(workspace_path)?;
    if !Path::new(workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {workspace_path}"));
    }

    let codex = crate::codex_source::find_codex_binary().ok_or_else(|| {
        "Codex CLI not found (no standalone install, VSCode extension, or `codex` on PATH)"
            .to_string()
    })?;

    let prompt = build_image_prompt(description);
    // `workspace-write` (not read-only): the skill inspects its own SKILL.md and
    // may run `sips`-style checks on what it produced.
    // No decision-card / notify bridging here: this is a one-shot tool call, not
    // a Fleet-owned session, so it must not register itself as one.
    let args = crate::codex_launch::build_codex_exec_args(
        workspace_path,
        &prompt,
        Some(model.unwrap_or(DEFAULT_ROUTING_MODEL)),
        None,
        &["-s".to_string(), "workspace-write".to_string()],
    );

    let out = Command::new(&codex)
        .args(&args)
        .current_dir(workspace_path)
        .output()
        .map_err(|e| format!("spawn {}: {e}", codex.display()))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let thread_id = stdout
        .lines()
        .find_map(crate::codex_launch::parse_thread_started)
        .ok_or_else(|| {
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!(
                "codex never printed thread.started (exit {:?}); stderr: {}",
                out.status.code(),
                stderr.trim()
            )
        })?;

    let images = list_thread_images(&thread_id);
    let agent_message = parse_last_agent_message(&stdout).unwrap_or_default();

    if images.is_empty() {
        // A turn can end cleanly having generated nothing — the agent asked a
        // question, refused, or (despite the wrapper) substituted an SVG. Its
        // message is the only explanation, so surface it instead of a bare
        // "no images".
        return Err(format!(
            "codex produced no image for thread {thread_id}. Agent said: {}",
            if agent_message.is_empty() {
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
    })
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
        let p = build_image_prompt("  a shiba in a red scarf  ");
        assert!(p.contains("image_gen"), "must name the built-in tool: {p}");
        assert!(p.contains("a shiba in a red scarf"), "must carry the description: {p}");
        assert!(!p.contains("  a shiba"), "description must be trimmed: {p}");
        // Fleet locates output by thread id; a helpful `mv` would empty the dir.
        assert!(p.contains("not move"), "must forbid moving the output: {p}");
        assert!(p.contains("SVG"), "must forbid the vector substitution: {p}");
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
    fn generate_rejects_blank_inputs_before_spawning() {
        // Guard rails must fire on argument shape, not after paying for a turn.
        assert!(generate_image("/tmp", "   ", None).is_err());
        assert!(generate_image("  ", "draw a cat", None).is_err());
    }

    #[test]
    fn generate_rejects_missing_workspace() {
        let err = generate_image("/definitely/not/here/xyz", "draw a cat", None)
            .expect_err("missing workspace must fail");
        assert!(err.contains("not found"), "got {err}");
    }
}
