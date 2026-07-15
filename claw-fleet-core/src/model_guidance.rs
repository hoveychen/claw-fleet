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
    crate::session::real_home_dir().map(|h| h.join(".claude"))
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
    if locale == "zh" {
        return "# Fleet 模型选择速查 (managed by Claw Fleet — do not edit)\n\
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
| Fable 5 | `claude-fable-5` | 1M | $10 | $50 | 最强推理 + 超长程 agentic;\
thinking 常开、原始思维链不返回;需 30 天数据留存;比 Opus 贵一倍——只用在\
最难的任务 |\n\
| Opus 4.8 | `claude-opus-4-8` | 1M | $5 | $25 | 默认主力。最强 Opus 档,\
自主 agentic / 知识工作 / 记忆;支持 fast mode(约 2.5x 出字速度,溢价) |\n\
| Sonnet 5 | `claude-sonnet-5` | 1M | $3(至 2026-08-31 优惠 $2)| \
$15(优惠 $10)| 接近 Opus 的编码 / agentic,成本明显更低;高吞吐生产、\
并行 subagent 的性价比之选 |\n\
| Haiku 4.5 | `claude-haiku-4-5` | 200K | $1 | $5 | 最快最便宜;分类、抽取、\
简单机械活、延迟敏感任务 |\n\
\n\
effort(`output_config.effort` / `--effort`):`low` / `medium` / `high` / \
`xhigh` / `max`。`xhigh` 是编码和 agentic 的最佳档;`high` 是多数智力敏感\
任务的下限;`low` 给 subagent 和简单任务(更少、更集中的工具调用)。\n\
\n\
## Codex 家族(codex 工具链,gpt-5.6 系)\n\
\n\
Fleet 经 codex CLI 调用,按 **ChatGPT 套餐配额**计费,**没有按 token 的\
定价**。\n\
\n\
| 模型 | ID | 定位 |\n\
|---|---|---|\n\
| Sol | `gpt-5.6-sol` | 前沿最强 agentic 编码(Fleet 默认);低 effort 也很\
能打——先低后按需调高 |\n\
| Terra | `gpt-5.6-terra` | 均衡型日常 agentic 编码 |\n\
| Luna | `gpt-5.6-luna` | 快且省的 agentic 编码 |\n\
| GPT-5.5 | `gpt-5.5` | 复杂编码 / 研究前沿,默认 effort 更高(xhigh) |\n\
\n\
Sol / Terra / Luna = 强 / 中 / 快 三档,同属 gpt-5.6。Codex 的 effort 档是 \
`minimal` / `low` / `medium` / `high`(**没有** Claude 的 xhigh/max)。\n\
\n\
## 怎么挑\n\
\n\
- 机械、可并行、量大的 subagent → 便宜快档(Haiku / Sonnet;Luna / Terra)\
+ 低 effort。\n\
- 硬推理、最终综合、把关校验 → 最强档(Opus / Fable;Sol)+ high/xhigh。\n\
- 编码 / agentic 主循环 → Opus 4.8 或 Sonnet 5 配 xhigh;Codex 侧 Sol 从 \
medium 起步。\n\
- 拿不准就别 override,继承父/会话模型。\n"
            .to_string();
    }
    "# Fleet model-selection cheat-sheet (managed by Claw Fleet — do not edit)\n\
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
| Fable 5 | `claude-fable-5` | 1M | $10 | $50 | Strongest reasoning + \
longest-horizon agentic; thinking always on, raw chain-of-thought never \
returned; requires 30-day data retention; ~2x the price of Opus — reserve it \
for the hardest tasks |\n\
| Opus 4.8 | `claude-opus-4-8` | 1M | $5 | $25 | The default workhorse. Most \
capable Opus tier: autonomous agentic / knowledge work / memory; supports \
fast mode (~2.5x output speed, premium price) |\n\
| Sonnet 5 | `claude-sonnet-5` | 1M | $3 ($2 intro through 2026-08-31) | \
$15 ($10 intro) | Near-Opus coding / agentic at noticeably lower cost; the \
value pick for high-throughput production and parallel subagents |\n\
| Haiku 4.5 | `claude-haiku-4-5` | 200K | $1 | $5 | Fastest and cheapest; \
classification, extraction, simple mechanical work, latency-sensitive tasks |\n\
\n\
Effort (`output_config.effort` / `--effort`): `low` / `medium` / `high` / \
`xhigh` / `max`. `xhigh` is best for coding and agentic work; `high` is the \
floor for most intelligence-sensitive work; `low` for subagents and simple \
tasks (fewer, more-consolidated tool calls).\n\
\n\
## Codex family (codex toolchain, gpt-5.6 series)\n\
\n\
Fleet drives these through the codex CLI. They bill against a **ChatGPT-plan \
quota** and have **no per-token price**.\n\
\n\
| Model | ID | Positioning |\n\
|---|---|---|\n\
| Sol | `gpt-5.6-sol` | Frontier, most capable agentic coding (Fleet default); \
highly capable even at low effort — start low, turn it up as needed |\n\
| Terra | `gpt-5.6-terra` | Balanced everyday agentic coding |\n\
| Luna | `gpt-5.6-luna` | Fast and affordable agentic coding |\n\
| GPT-5.5 | `gpt-5.5` | Frontier for complex coding / research; higher default \
effort (xhigh) |\n\
\n\
Sol / Terra / Luna = strong / balanced / fast, all in the gpt-5.6 family. \
Codex effort levels are `minimal` / `low` / `medium` / `high` (**no** xhigh or \
max, unlike Claude).\n\
\n\
## How to pick\n\
\n\
- Mechanical, parallel, high-volume subagents → the cheap/fast tier \
(Haiku / Sonnet; Luna / Terra) at low effort.\n\
- Hard reasoning, final synthesis, adversarial verification → the strongest \
tier (Opus / Fable; Sol) at high/xhigh.\n\
- Coding / agentic main loop → Opus 4.8 or Sonnet 5 at xhigh; on the Codex \
side, Sol starting at medium.\n\
- When in doubt, don't override — inherit the parent/session model.\n"
        .to_string()
}

/// Apply model guidance: write the guidance file and inject the `@import`
/// sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_model_guidance(locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    // Always (re)write the guidance file — locale may have changed.
    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    fs::write(&guidance_path, render_guidance(locale))
        .map_err(|e| format!("write guidance file: {e}"))?;

    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let existing = fs::read_to_string(&claude_md).unwrap_or_default();
    let stripped = strip_sentinel_block(&existing);
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    let new_content = if stripped.is_empty() {
        block
    } else if stripped.ends_with('\n') {
        format!("{stripped}\n{block}")
    } else {
        format!("{stripped}\n\n{block}")
    };
    fs::write(&claude_md, new_content).map_err(|e| format!("write CLAUDE.md: {e}"))
}

/// Remove model guidance: strip the sentinel block and delete the guidance
/// file. Idempotent (no-op if already clean).
pub fn remove_model_guidance() -> Result<(), String> {
    if let Some(claude_md) = claude_md_path() {
        if let Ok(existing) = fs::read_to_string(&claude_md) {
            let stripped = strip_sentinel_block(&existing);
            if stripped != existing {
                fs::write(&claude_md, stripped).map_err(|e| format!("write CLAUDE.md: {e}"))?;
            }
        }
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
            assert!(g.contains("claude-opus-4-8"), "{locale} must list Opus 4.8");
            assert!(g.contains("claude-fable-5"), "{locale} must list Fable 5");
            assert!(g.contains("claude-sonnet-5"), "{locale} must list Sonnet 5");
            assert!(g.contains("claude-haiku-4-5"), "{locale} must list Haiku 4.5");
            // Codex family model IDs
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
    fn render_marks_codex_as_quota_billed() {
        // Never quote a per-token price for Codex — it bills against a plan
        // quota. The guidance must say so explicitly in both locales.
        assert!(render_guidance("zh").contains("配额"), "zh must flag Codex quota billing");
        assert!(render_guidance("en").contains("quota"), "en must flag Codex quota billing");
    }
}
