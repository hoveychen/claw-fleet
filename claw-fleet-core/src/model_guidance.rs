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
pub fn render_guidance(locale: &str) -> String {
    // The model tables come from `models.toml` via `model_catalog`. They used to
    // be hand-written here and in the two AGENTS.md cheat-sheets, and the three
    // copies drifted: all of them claimed Codex tops out at `high` and offers a
    // `minimal` level, and all put Astra at 1.05M context. The catalog's numbers
    // are read from `~/.codex/models_cache.json`.
    let claude_rows = crate::model_catalog::render_rows("claude-code", locale, true);
    let codex_rows = crate::model_catalog::render_rows_with("codex", locale, false, false);
    let claude_efforts = crate::model_catalog::render_effort_line("claude-code", locale);
    let codex_efforts = crate::model_catalog::render_effort_line("codex", locale);
    if locale == "zh" {
        return format!("# Fleet 模型选择速查 (managed by Claw Fleet — do not edit)\n\
\n\
给 subagent、workflow agent 或新会话选模型时用。**默认继承父/会话模型**——它\
几乎总是对的;只有当你明确判断某一档更合适时才 override。选模型的入口:\
`Agent` 工具的 `model` 参数、`Workflow` 里 `agent()` 的 `opts.model`/\
`opts.effort`、`fleet` spawn 的 `--model`、`cws dispatch` 的 `--model`/\
`--effort`。\n\
\n\
## Claude 家族(claude 工具链)\n\
\n\
| 模型 | ID | 上下文 | 输入 $/1M | 输出 $/1M | 何时选 |\n\
|---|---|---|---|---|---|\n\
{claude_rows}\
\n\
effort(`output_config.effort` / `--effort`):{claude_efforts}。`xhigh` 是编码和 \
agentic 的最佳档;`high` 是多数智力敏感任务的下限;`low` 给 subagent 和简单\
任务(更少、更集中的工具调用)。\n\
\n\
## Codex 家族(codex 工具链)\n\
\n\
Fleet 经 codex CLI 调用,按 **ChatGPT 套餐配额**计费,**没有按 token 的\
定价**。\n\
\n\
| 模型 | ID | 上下文 | 定位 |\n\
|---|---|---|---|\n\
{codex_rows}\
\n\
Sol / Terra / Luna = 强 / 中 / 快 三档,同属 gpt-5.6。effort 梯子:{codex_efforts}。\n\
\n\
## 生图(只有 codex 有)\n\
\n\
Claude 侧**没有**生图能力。要位图资产(插画、贴图、mockup、hero 图)时借 \
codex 自带的 imagegen skill:`codex exec -m gpt-5.6-luna \"用内置图像生成\
工具画 …\"`。模型是 `gpt-image-2`,走 ChatGPT 配额,**不需要** \
`OPENAI_API_KEY`。产物落 `$CODEX_HOME/generated_images/<thread_id>/`,其中 \
`<thread_id>` 就是 `--json` 流里 `thread.started` 的那个 id。细节(尺寸约束、\
透明背景限制、token 成本)见 wiki `codex/image-generation`。\n\
\n\
## 怎么挑\n\
\n\
- 机械、可并行、量大的 subagent → 便宜快档(Haiku / Sonnet;Luna / Terra)\
+ 低 effort。\n\
- 最难的端到端工作 → Astra + high/xhigh;普通硬推理、最终把关 → \
Opus / Fable 或 Sol。\n\
- 编码 / agentic 主循环 → Opus 5 或 Sonnet 5 配 xhigh;Codex 侧 Sol 从 \
medium 起步。\n\
- 拿不准就别 override,继承父/会话模型。\n");
    }
    format!("# Fleet model-selection cheat-sheet (managed by Claw Fleet — do not edit)\n\
\n\
Use this when picking a model for a subagent, a workflow agent, or a new \
session. **Default to inheriting the parent/session model** — it is almost \
always right; only override when you have a clear reason a different tier \
fits. The places a model gets chosen: the `Agent` tool's `model` param, \
`Workflow` `agent()`'s `opts.model`/`opts.effort`, `fleet` spawn's `--model`, \
and `cws dispatch`'s `--model`/`--effort`.\n\
\n\
## Claude family (claude toolchain)\n\
\n\
| Model | ID | Context | In $/1M | Out $/1M | When to pick |\n\
|---|---|---|---|---|---|\n\
{claude_rows}\
\n\
Effort (`output_config.effort` / `--effort`): {claude_efforts}. `xhigh` is best \
for coding and agentic work; `high` is the floor for most \
intelligence-sensitive work; `low` for subagents and simple tasks (fewer, \
more-consolidated tool calls).\n\
\n\
## Codex family (codex toolchain)\n\
\n\
Fleet drives these through the codex CLI. They bill against a **ChatGPT-plan \
quota** and have **no per-token price**.\n\
\n\
| Model | ID | Context | Positioning |\n\
|---|---|---|---|\n\
{codex_rows}\
\n\
Sol / Terra / Luna = strong / balanced / fast, all in the gpt-5.6 family. \
Effort ladders: {codex_efforts}.\n\
\n\
## Image generation (codex only)\n\
\n\
The Claude side has **no** image-generation capability. When you need a raster \
asset (illustration, sprite, mockup, hero image), borrow codex's bundled \
imagegen skill: `codex exec -m gpt-5.6-luna \"use the built-in image \
generation tool to draw …\"`. The model is `gpt-image-2`, it bills against the \
ChatGPT quota, and it does **not** need `OPENAI_API_KEY`. Output lands in \
`$CODEX_HOME/generated_images/<thread_id>/`, where `<thread_id>` is the id \
from `thread.started` in the `--json` stream. Details (size constraints, \
transparency limits, token cost) live in the wiki at \
`codex/image-generation`.\n\
\n\
## How to pick\n\
\n\
- Mechanical, parallel, high-volume subagents → the cheap/fast tier \
(Haiku / Sonnet; Luna / Terra) at low effort.\n\
- Hardest end-to-end work → Astra at high/xhigh; regular hard reasoning and \
final verification → Opus / Fable or Sol.\n\
- Coding / agentic main loop → Opus 5 or Sonnet 5 at xhigh; on the Codex \
side, Sol starting at medium.\n\
- When in doubt, don't override — inherit the parent/session model.\n")
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

    #[test]
    fn render_both_locales_point_image_gen_at_codex() {
        // Claude cannot generate images; a session that doesn't know that will
        // hand back ASCII or SVG instead of borrowing codex's gpt-image-2. Both
        // locales must name the model and where the output lands.
        for locale in ["en", "zh"] {
            let g = render_guidance(locale);
            assert!(g.contains("gpt-image-2"), "{locale} must name the image model");
            assert!(
                g.contains("generated_images"),
                "{locale} must say where generated images land"
            );
            assert!(
                g.contains("codex/image-generation"),
                "{locale} must point at the wiki doc"
            );
        }
    }

    #[test]
    fn render_marks_codex_as_quota_billed() {
        // Never quote a per-token price for Codex — it bills against a plan
        // quota. The guidance must say so explicitly in both locales.
        assert!(render_guidance("zh").contains("配额"), "zh must flag Codex quota billing");
        assert!(render_guidance("en").contains("quota"), "en must flag Codex quota billing");
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
