//! Wiki guidance — injects a short block into `~/.claude/CLAUDE.md` telling
//! agents to publish durable HTML reports / demos / markdown docs into the
//! Fleet wiki via `fleet wiki publish`, instead of leaving them scattered in
//! workdirs.
//!
//! Install strategy mirrors `interaction_mode`:
//!   1. Render `~/.claude/fleet-wiki-guidance.md` for the given locale.
//!   2. Inject a sentinel-wrapped `@~/.claude/fleet-wiki-guidance.md` import
//!      into `~/.claude/CLAUDE.md`.
//!
//! Uninstall removes both. All operations are idempotent.

use std::fs;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "<!-- fleet:wiki-guidance:begin -->";
const END_MARKER: &str = "<!-- fleet:wiki-guidance:end -->";

fn claude_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".claude"))
}

fn guidance_file_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("fleet-wiki-guidance.md"))
}

fn claude_md_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("CLAUDE.md"))
}

/// Build the guidance markdown for the locale (`zh` gets a Chinese variant,
/// everything else English).
pub fn render_guidance(locale: &str) -> String {
    if locale == "zh" {
        return "# Fleet Wiki 知识库 (managed by Claude Fleet — do not edit)\n\
\n\
当你产出**值得留存**的 HTML 报告、可交互 demo 或 markdown 文档(调研报告、\
架构说明、性能分析、数据可视化等)时,完成后用 Fleet 的 wiki 知识库归档,\
不要只把文件留在工作目录里:\n\
\n\
```\nfleet wiki publish <path> [--slug <slug>] [--title \"<标题>\"]\n```\n\
\n\
- `<path>` 可以是单个 `.html` / `.md` 文件,或包含 `index.html` 的目录\
(目录会连同相对引用的 js/css/图片一起入库)。\n\
- 同一份文档迭代时**复用同一个 slug** 重新 publish——会生成新版本,\
旧版本自动保留可回看;不要为同一份内容起新 slug。\n\
- slug 用小写字母/数字/连字符;`--title` 缺省时取 `<title>` 标签或首个 \
`# ` 标题。\n\
- slug 里的 `/` 是**虚拟目录**分隔符(像对象存储的 key):发布成 \
`--slug arch/overview` 后,桌面端知识库会把它显示在 `arch` 目录下。同一主题的 \
文档用同一个前缀,别让根目录堆满散装文档。已发布的文档可以用 \
`fleet wiki mv <旧 slug> <新 slug>` 改键搬进目录,版本历史会一起带走。\n\
- 只归档最终成品,草稿、中间产物、一次性调试页不要 publish。\n\
- markdown 文档里可以用 `[[slug]]` 或 `[[slug|显示文字]]` 引用知识库里\
其他文档,渲染后可点击跳转;引用前先用 `fleet wiki list` 确认目标 slug \
已发布(未发布的引用会显示为灰色死链)。\n\
- 发布后文档出现在 Fleet 桌面端的「知识库」板块,用户可按 workspace \
筛选、全文搜索、切换版本、导出、直接渲染(HTML 里的 JS 可运行)。\n\
\n\
## 读取知识库里的文档\n\
\n\
当你需要某篇文档的正文时——尤其是用户在 prompt 里用 `[[slug]]` 引用了\
它——用 `cat` 直接读,**不要**去 `~/.fleet/wiki/` 底下手动拼版本目录:\n\
\n\
```\n\
fleet wiki cat <slug>                       # 当前版本的正文\n\
fleet wiki cat <slug> --version <version-id>  # 某个历史版本\n\
fleet wiki cat <slug> --file assets/app.js  # 目录型文档里的其他文件\n\
```\n\
\n\
- 默认输出当前版本的 entry 文件(markdown 文档就是正文,HTML 目录是 \
`index.html`)。\n\
- 不知道有哪些文档就 `fleet wiki list`;想看某篇的版本历史和 entry \
文件名就 `fleet wiki show <slug>`。\n"
            .to_string();
    }
    "# Fleet Wiki knowledge base (managed by Claude Fleet — do not edit)\n\
\n\
When you produce a **durable** HTML report, interactive demo, or markdown \
document (research reports, architecture notes, performance analyses, data \
visualizations…), archive it into the Fleet wiki when done instead of leaving \
it in the workdir:\n\
\n\
```\nfleet wiki publish <path> [--slug <slug>] [--title \"<title>\"]\n```\n\
\n\
- `<path>` is a single `.html` / `.md` file, or a directory with an \
`index.html` entry (the directory's relatively-referenced js/css/images are \
archived along with it).\n\
- When iterating on the same document, **re-publish with the same slug** — \
that creates a new version and keeps old ones browsable; don't mint a new \
slug for the same content.\n\
- Slugs are lowercase letters/digits/hyphens; `--title` defaults to the \
`<title>` tag or first `# ` heading.\n\
- A `/` in a slug is a **virtual directory** separator, like an object-store \
key: `--slug arch/overview` files the doc under an `arch` folder in the \
desktop Wiki board. Share a prefix across docs on one topic instead of piling \
everything at the root. Already-published docs move with \
`fleet wiki mv <old-slug> <new-slug>`, version history included.\n\
- Publish finished artifacts only — no drafts, intermediates, or one-off \
debug pages.\n\
- Markdown docs can reference other wiki docs with `[[slug]]` or \
`[[slug|display text]]` — rendered as clickable cross-links. Check the \
target slug exists first with `fleet wiki list` (unpublished refs render \
as grayed dead links).\n\
- Published docs appear in the Fleet desktop app's Wiki board, filterable by \
workspace, full-text searchable, with version switching, export, and live \
HTML rendering (scripts run).\n\
\n\
## Reading a wiki doc\n\
\n\
When you need a doc's content — especially when the user referenced it as \
`[[slug]]` in their prompt — read it with `cat`. Do **not** hand-assemble \
version paths under `~/.fleet/wiki/`:\n\
\n\
```\n\
fleet wiki cat <slug>                         # current version's content\n\
fleet wiki cat <slug> --version <version-id>  # a historical version\n\
fleet wiki cat <slug> --file assets/app.js    # another file in a dir doc\n\
```\n\
\n\
- Defaults to the current version's entry file (the body for markdown docs, \
`index.html` for HTML directories).\n\
- `fleet wiki list` shows the available slugs; `fleet wiki show <slug>` \
shows its version history and entry filename.\n"
        .to_string()
}

/// Apply wiki guidance: write the guidance file and inject the `@import`
/// sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_wiki_guidance(locale: &str) -> Result<(), String> {
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

/// Remove wiki guidance: strip the sentinel block and delete the guidance
/// file. Idempotent (no-op if already clean).
pub fn remove_wiki_guidance() -> Result<(), String> {
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
pub fn is_wiki_guidance_installed() -> bool {
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
        // interaction_mode / prd_discipline sentinels must not collide with
        // ours — stripping one mode must never eat another mode's block.
        assert!(BEGIN_MARKER.contains("wiki-guidance"));
        assert_ne!(BEGIN_MARKER, "<!-- fleet:interaction-mode:begin -->");
        assert_ne!(BEGIN_MARKER, "<!-- fleet:prd-discipline:begin -->");
    }

    #[test]
    fn strip_removes_block_and_preserves_rest() {
        let content = format!(
            "user rules\n\n{BEGIN_MARKER}\n@/home/x/.claude/fleet-wiki-guidance.md\n{END_MARKER}\nmore rules\n"
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
        let content = "<!-- fleet:interaction-mode:begin -->\n@x.md\n<!-- fleet:interaction-mode:end -->\n";
        assert_eq!(strip_sentinel_block(content), content);
    }

    #[test]
    fn render_both_locales_mention_publish() {
        for locale in ["en", "zh"] {
            let g = render_guidance(locale);
            assert!(g.contains("fleet wiki publish"), "{locale} guidance must mention the command");
            assert!(g.contains("slug"), "{locale} guidance must explain slug reuse");
            assert!(g.contains("[[slug]]"), "{locale} guidance must document cross-links");
        }
    }
}
