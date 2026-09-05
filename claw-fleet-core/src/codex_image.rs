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

/// Rules every generation/edit prompt repeats. Two of them are load-bearing for
/// Fleet rather than for the picture: **don't move the output** (Fleet locates
/// files by thread id, so a helpful `mv` into the workspace empties the
/// directory we are about to read) and **don't substitute a vector stand-in**
/// (the skill tells the agent to prefer repo-native SVG for icon-shaped asks,
/// which would silently return no bitmap at all).
const PROMPT_RULES: &str = "Do not substitute SVG, HTML/CSS, or any code-native stand-in, and do \
     not move or copy the generated file anywhere — leave it at its default \
     location and simply report what you made.";

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
        "Use the built-in `image_gen` tool to generate the following image.{refs} {PROMPT_RULES}\n\n{}",
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
        "Use the built-in `image_gen` tool to revise the image you generated earlier in this \
         conversation. Change only what the instruction below asks for and keep everything else \
         unchanged.{refs} {PROMPT_RULES}\n\n{}",
        instruction.trim()
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
        &pre_prompt_args(images),
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
        &pre_prompt_args(images),
    );
    crate::codex_launch::wrap_codex_launch(codex, args, workspace_path)
}

/// Flags shared by both launches, in front of the `--` separator.
///
/// `workspace-write` (not read-only): the skill inspects its own SKILL.md and
/// may run `sips`-style checks on what it produced. No decision-card / notify
/// bridging — this is a one-shot tool call, not a Fleet-owned session, so it
/// must not register itself as one.
fn pre_prompt_args(images: &[String]) -> Vec<String> {
    let mut args = vec!["-s".to_string(), "workspace-write".to_string()];
    args.extend(crate::codex_launch::codex_image_args(images));
    args
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
        .ok_or_else(|| format!("codex never printed thread.started; output: {}", tail(&stdout)))?;

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
        // A turn can end cleanly having generated nothing — the agent asked a
        // question, refused, or (despite the wrapper) substituted an SVG. Its
        // message is the only explanation, so surface it instead of a bare
        // "no images".
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

/// Spawn the turn and capture its `--json` stdout.
fn run_turn(
    program: &Path,
    args: &[String],
    rca_envs: &[(String, String)],
    workspace_path: &str,
) -> Result<String, String> {
    let mut cmd = crate::process_util::command(program);
    cmd.args(args)
        .current_dir(workspace_path)
        // MUST be null: `codex exec` otherwise blocks reading stdin forever.
        .stdin(std::process::Stdio::null());
    crate::codex_launch::apply_codex_launch_env(&mut cmd);
    for (k, v) in rca_envs {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("spawn {}: {e}", program.display()))?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!(
            "codex exited {:?}: {}",
            out.status.code(),
            tail(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
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
