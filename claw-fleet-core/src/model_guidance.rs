//! Model-selection guidance — injects a short reference block into
//! `~/.claude/CLAUDE.md` giving agents a capability/cost cheat-sheet for the
//! Claude and Codex model families, so they can pick a model deliberately when
//! spawning subagents, workflow agents, or new sessions (`Agent` tool `model`,
//! `Workflow` `agent()` `opts.model`/`opts.effort`, `fleet` spawn `--model`,
//! `cws dispatch --model`/`--effort`).
//!
//! Install strategy mirrors `wiki_guidance` / `interaction_mode`:
//!   1. Render `~/.claude/fleet-model-guidance.md` for the given locale.
//!   2. Inject a sentinel-wrapped `@~/.claude/fleet-model-guidance.md` import
//!      into `~/.claude/CLAUDE.md`.
//!
//! Uninstall removes both. All operations are idempotent.

use std::fs;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "<!-- fleet:model-guidance:begin -->";
const END_MARKER: &str = "<!-- fleet:model-guidance:end -->";

fn claude_dir() -> Option<PathBuf> {
    crate::session::get_claude_dir()
}

fn guidance_file_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("fleet-model-guidance.md"))
}

fn claude_md_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("CLAUDE.md"))
}

/// Build the guidance markdown for the locale (`zh` gets a Chinese variant,
/// everything else English).
///
/// Pricing/positioning for the Claude family is sourced from the `claude-api`
/// skill catalog; the Codex family (`gpt-5.6-sol`/`-terra`/`-luna`, `gpt-5.5`)
/// from `~/.codex/models_cache.json` — Codex bills against a ChatGPT-plan
/// quota, so it has no per-token price to quote.
/// Build the model cheat-sheet for the locale.
///
/// One document, shared verbatim by all three harnesses — see
/// [`crate::model_catalog::render_sheet`], which renders it from
/// `models.toml`.
pub fn render_guidance(locale: &str) -> String {
    crate::model_catalog::render_sheet(locale)
}

/// Apply model guidance: write the guidance file and inject the `@import`
/// sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_model_guidance(locale: &str) -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_model_guidance_inner(locale),
        crate::control_plane_prefs::Feature::ModelGuidance,
        false,
    )
}

fn apply_model_guidance_inner(locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    // Always (re)write the guidance file — locale may have changed.
    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    fs::write(&guidance_path, render_guidance(locale))
        .map_err(|e| format!("write guidance file: {e}"))?;

    // Locked read-modify-write — see `claude_md_lock`.
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    crate::claude_md_lock::with_lock(&claude_md, || {
        let existing = fs::read_to_string(&claude_md).unwrap_or_default();
        let stripped = strip_sentinel_block(&existing);
        let new_content = if stripped.is_empty() {
            block
        } else if stripped.ends_with('\n') {
            format!("{stripped}\n{block}")
        } else {
            format!("{stripped}\n\n{block}")
        };
        crate::atomic_json::write_atomic(&claude_md, new_content.as_bytes()).map_err(|e| format!("write CLAUDE.md: {e}"))
    })
}

/// Remove model guidance: strip the sentinel block and delete the guidance
/// file. Idempotent (no-op if already clean).
pub fn remove_model_guidance() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        remove_model_guidance_inner(),
        crate::control_plane_prefs::Feature::ModelGuidance,
        true,
    )
}

fn remove_model_guidance_inner() -> Result<(), String> {
    if let Some(claude_md) = claude_md_path() {
        crate::claude_md_lock::with_lock(&claude_md, || {
            if let Ok(existing) = fs::read_to_string(&claude_md) {
                let stripped = strip_sentinel_block(&existing);
                if stripped != existing {
                    crate::atomic_json::write_atomic(&claude_md, stripped.as_bytes()).map_err(|e| format!("write CLAUDE.md: {e}"))?;
                }
            }
            Ok::<(), String>(())
        })?;
    }
    if let Some(path) = guidance_file_path() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("remove guidance file: {e}"))?;
        }
    }
    Ok(())
}

/// Whether the sentinel block is present in `~/.claude/CLAUDE.md`.
pub fn is_model_guidance_installed() -> bool {
    let Some(claude_md) = claude_md_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&claude_md) else {
        return false;
    };
    content.contains(BEGIN_MARKER) && content.contains(END_MARKER)
}

fn strip_sentinel_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == BEGIN_MARKER {
            in_block = true;
            continue;
        }
        if trimmed == END_MARKER {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_unique_vs_other_fleet_blocks() {
        // interaction_mode / prd_discipline / wiki_guidance sentinels must not
        // collide with ours — stripping one mode must never eat another's block.
        assert!(BEGIN_MARKER.contains("model-guidance"));
        assert_ne!(BEGIN_MARKER, "<!-- fleet:interaction-mode:begin -->");
        assert_ne!(BEGIN_MARKER, "<!-- fleet:prd-discipline:begin -->");
        assert_ne!(BEGIN_MARKER, "<!-- fleet:wiki-guidance:begin -->");
    }

    #[test]
    fn strip_removes_block_and_preserves_rest() {
        let content = format!(
            "user rules\n\n{BEGIN_MARKER}\n@/home/x/.claude/fleet-model-guidance.md\n{END_MARKER}\nmore rules\n"
        );
        assert_eq!(strip_sentinel_block(&content), "user rules\n\nmore rules\n");
    }

    #[test]
    fn strip_noop_without_block() {
        let content = "just some rules\n";
        assert_eq!(strip_sentinel_block(content), content);
    }

    #[test]
    fn strip_leaves_other_modes_blocks_alone() {
        let content = "<!-- fleet:wiki-guidance:begin -->\n@x.md\n<!-- fleet:wiki-guidance:end -->\n";
        assert_eq!(strip_sentinel_block(content), content);
    }

    #[test]
    fn render_both_locales_cover_both_families() {
        for locale in ["en", "zh"] {
            let g = render_guidance(locale);
            // Claude family model IDs
            assert!(g.contains("claude-opus-5"), "{locale} must list Opus 5");
            assert!(g.contains("claude-fable-5-1"), "{locale} must list Fable 5.1");
            assert!(g.contains("claude-fable-5"), "{locale} must list Fable 5");
            assert!(g.contains("claude-sonnet-5"), "{locale} must list Sonnet 5");
            assert!(g.contains("claude-haiku-4-5"), "{locale} must list Haiku 4.5");
            // Codex family model IDs
            assert!(g.contains("gpt-6-astra"), "{locale} must list GPT-6 Astra");
            assert!(g.contains("gpt-5.6-sol"), "{locale} must list Codex Sol");
            assert!(g.contains("gpt-5.6-terra"), "{locale} must list Codex Terra");
            assert!(g.contains("gpt-5.6-luna"), "{locale} must list Codex Luna");
        }
    }

    #[test]
    fn render_both_locales_flag_the_selection_surfaces() {
        // The guidance is useless if it doesn't tell the agent WHERE a model is
        // chosen — the four override surfaces must be named in both locales.
        for locale in ["en", "zh"] {
            let g = render_guidance(locale);
            assert!(g.contains("opts.model"), "{locale} must name the Workflow agent() override");
            assert!(g.contains("--model"), "{locale} must name the spawn/dispatch override");
            assert!(g.contains("--effort"), "{locale} must name the effort override");
        }
    }
    /// The sheet quotes no prices at all.
    ///
    /// This used to assert the narrower rule "never quote a per-token price for
    /// Codex, which bills against a plan quota". The sheet now carries no price
    /// column in any row: what a model costs is not how an agent should pick one
    /// — that is what the tier vocabulary is for — so the Codex-specific hazard
    /// is subsumed by the general one.
    #[test]
    fn render_quotes_no_prices() {
        for locale in ["zh", "en"] {
            let g = render_guidance(locale);
            assert!(!g.contains('$'), "{locale} must not quote a price");
            assert!(!g.contains("/1M"), "{locale} must not carry a per-Mtok column");
        }
    }
}

#[cfg(test)]
mod render_dump {
    /// Not an assertion — a way to eyeball the generated cheat-sheet.
    /// Run with `cargo test -p claw-fleet-core --lib dump_zh -- --nocapture`.
    #[test]
    #[ignore]
    fn dump_zh() {
        println!("{}", super::render_guidance("zh"));
    }
    #[test]
    #[ignore]
    fn dump_en() {
        println!("{}", super::render_guidance("en"));
    }
}
