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
    crate::session::get_claude_dir()
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
        return "# Fleet Wiki 知识库 (managed by Claw Fleet — do not edit)\n\
\n\
当你产出**值得留存**的 HTML 报告、可交互 demo 或 markdown 文档(调研报告、\
架构说明、性能分析、数据可视化等)时,完成后用 Fleet 的 wiki 知识库归档,\
不要只把文件留在工作目录里:\n\
\n\
```\nfleet wiki publish <path> [--slug <slug>] [--title \"<标题>\"]\n```\n\
\n\
> **若工具列表里有 `fleet__wiki` MCP 工具(Fleet 启动的会话都有),优先用它\
(`action=\"publish\"/\"cat\"/\"list\"/\"search\"`)而不是 `fleet wiki` CLI——远端\
(rca)会话里 Bash 跑 `fleet wiki` 会被路由到没有 fleet 的远端而失败。\
注意 `publish` 读的是**本地**文件路径,远端会话里 agent 写的文件可能在远端 fs \
上;这种情况仍需用 CLI 或先把文件取到本地。工具不可用时才退回 CLI。**\n\
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
其他文档,渲染后可点击跳转;slug 是全局的,引用前先用 \
`fleet wiki list --all` 确认目标 slug 已发布(未发布的引用会显示为灰色\
死链)。\n\
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
- 不知道有哪些文档就 `fleet wiki list`——默认只列当前 workspace 发布过的\
文档,正好用来开工前回顾这个项目已有的调研成果;加 `--all` 看所有 \
workspace。\n\
- 按内容找就 `fleet wiki search <关键词>`(同样默认限当前 workspace,\
`--all` 放宽),它会搜标题、slug 和正文并给出片段。\n\
- 想看某篇的版本历史和 entry 文件名就 `fleet wiki show <slug>`。\n\
\n\
## 交付物走产出库,不是 wiki\n\
\n\
当你产出的是**要交给人的文件**——PDF、幻灯片(pptx)、表格(xlsx)、Word \
文档、渲染好的视频或图片、导出的数据集——用产出库,不要试图 publish 进 \
wiki:\n\
\n\
```\nfleet artifact add <path> [--title \"<标题>\"] [--note \"<一句话说明>\"]\n```\n\
\n\
> **若工具列表里有 `fleet__artifact` MCP 工具(Fleet 启动的会话都有),优先\
用它(`action=\"add\"/\"list\"/\"get\"/\"delete\"`)而不是 `fleet artifact` CLI\
——理由与 `fleet__wiki` 相同:远端(rca)会话里 Bash 跑 `fleet` 会被路由到\
没有 fleet 的远端而失败。**\n\
\n\
- **wiki 装不下这些格式,不是风格问题而是硬限制。**知识库的 kind 只有 \
`html`/`htmlDir`/`markdown`,entry 必须是可渲染的文本文件;一份 `.xlsx` \
publish 进去,列表能列出来、点开是白板。\n\
- **产出一旦生成就立刻入库,别等到最后。**交付物通常写在 \
`.worktrees/<task-id>` 里,而那个目录会在计划合并时被删掉——到那时文件就\
没了。这跟「合并前抢救 gitignored 产物」是同一条命,只是这里有现成的地方放。\n\
- **`--title` 和 `--note` 值得认真写。**它们就是用户在卡片上读到的全部\
内容;缺省标题是文件名,而「out.xlsx」对人没有任何意义。\n\
- 一次一个文件,不接受目录——要存一整个文件夹先打包成 zip。\n\
- 入库后出现在 Fleet 桌面端的「产出」板块:可按 workspace 筛选、收藏、\
预览(图片/视频/音频/PDF/文本直接看,Office 给类型占位符并提供导出与\
系统应用打开)、导出到任意位置。\n\
\n\
## 什么时候不该用 wiki\n\
\n\
渲染一份东西给人看,有三个近亲手段,别混用:\n\
\n\
- **一次性看一眼**(过目一张图、一段 diff,看完即弃)→ 用 `fleet__ask` 的 \
`html` 或 `fleet__render_a2ui` 渲染成决策卡,不要 publish。\n\
- **要把链接发给别人**→ 用 Claude Code 的 Artifact 工具(如果本 session \
有)。它托管在 claude.ai 上、可分享,代价是内容会上传到外部服务,且必须单\
文件自包含(CSS/JS 内联、图片转 data URI)。私有项目的产物别走这条。\n\
- **要交给人的二进制文件**(PDF / pptx / xlsx / 视频 / 图片)→ 产出库,\
见上一节。wiki 根本渲染不了它们。\n\
- **要沉淀、之后还要读回来**→ wiki。只有 wiki 的文档能被后续 session 用 \
`fleet wiki cat` 读到正文、用 `[[slug]]` 交叉引用、按 workspace 筛选和全文\
搜索,也只有 wiki 收得下带 `assets/` 的多文件目录。\n\
\n\
判据不是「哪个能渲染」——三个都能;而是「这份产物之后还会不会被读第二次」。\
会 → wiki。\n"
            .to_string();
    }
    "# Fleet Wiki knowledge base (managed by Claw Fleet — do not edit)\n\
\n\
When you produce a **durable** HTML report, interactive demo, or markdown \
document (research reports, architecture notes, performance analyses, data \
visualizations…), archive it into the Fleet wiki when done instead of leaving \
it in the workdir:\n\
\n\
```\nfleet wiki publish <path> [--slug <slug>] [--title \"<title>\"]\n```\n\
\n\
> **If your tool list includes the `fleet__wiki` MCP tool (every Fleet-launched \
session has it), prefer it (`action=\"publish\"/\"cat\"/\"list\"/\"search\"`) over \
the `fleet wiki` CLI — in an rca remote session a Bash `fleet wiki` is routed to \
a remote executor with no `fleet` and fails. Note `publish` reads a **local** \
file path; in a remote session the file the agent wrote may live on the remote \
fs, in which case use the CLI or fetch the file locally first. Fall back to the \
CLI only when the tool is absent.**\n\
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
`[[slug|display text]]` — rendered as clickable cross-links. Slugs are \
global, so check the target exists first with `fleet wiki list --all` \
(unpublished refs render as grayed dead links).\n\
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
- `fleet wiki list` shows the docs published from the current workspace — \
the way to review what this project already investigated before starting \
work; `--all` spans every workspace.\n\
- `fleet wiki search <term>` finds docs by content (also current-workspace \
by default, `--all` to widen), matching titles, slugs and body text.\n\
- `fleet wiki show <slug>` shows a doc's version history and entry \
filename.\n\
\n\
## Deliverables go to the artifact store, not the wiki\n\
\n\
When what you produced is a **file meant to be handed to a person** — a PDF, a \
slide deck (pptx), a spreadsheet (xlsx), a Word document, a rendered video or \
image, an exported dataset — store it as an artifact instead of trying to \
publish it to the wiki:\n\
\n\
```\nfleet artifact add <path> [--title \"<title>\"] [--note \"<one line>\"]\n```\n\
\n\
> **If your tool list includes the `fleet__artifact` MCP tool (every \
Fleet-launched session has it), prefer it (`action=\"add\"/\"list\"/\"get\"/\
\"delete\"`) over the `fleet artifact` CLI — same reason as `fleet__wiki`: in \
an rca remote session a Bash `fleet` is routed to a remote executor with no \
`fleet` and fails.**\n\
\n\
- **The wiki cannot hold these formats — a hard limit, not a preference.** Its \
kinds are `html`/`htmlDir`/`markdown` and the entry must be a renderable text \
file; an `.xlsx` published there lists fine and opens blank.\n\
- **Store it the moment you produce it, not at the end.** Deliverables are \
usually written inside `.worktrees/<task-id>`, and that directory is deleted \
when the plan merges — after which the file is gone. Same hazard as rescuing \
gitignored output before removing a worktree, except here there is a place \
built to put it.\n\
- **`--title` and `--note` are worth writing properly.** They are the entire \
content the user reads on the card; the default title is the filename, and \
\"out.xlsx\" tells a person nothing.\n\
- One file per call, not a directory — zip a folder first.\n\
- Stored artifacts appear on the desktop's 产出 board: filter by workspace, \
star them, preview them (images / video / audio / PDF / text render inline; \
Office formats get a typed placeholder plus export and open-with-system-app), \
and export anywhere.\n\
\n\
## When NOT to use the wiki\n\
\n\
Three close cousins render something for a human to look at. Don't confuse \
them:\n\
\n\
- **A one-time glance** (show a chart or a diff, then throw it away) → render \
it into a decision card with `fleet__ask`'s `html` or `fleet__render_a2ui`. \
Don't publish it.\n\
- **A link to send someone else** → use Claude Code's Artifact tool, if this \
session has one. It's hosted on claude.ai and shareable; the cost is that the \
content is uploaded to an external service, and it must be a single \
self-contained file (inlined CSS/JS, images as data URIs). Keep private \
project output off it.\n\
- **A binary file meant for a person** (PDF / pptx / xlsx / video / image) \
→ the artifact store, see the section above. The wiki cannot render any of \
them.\n\
- **Something that will be read again later** → the wiki. Only wiki docs can \
be read back by a later session with `fleet wiki cat`, cross-linked with \
`[[slug]]`, filtered by workspace, and full-text searched — and only the wiki \
accepts a multi-file directory with an `assets/` folder.\n\
\n\
The test is not \"which one can render this\" — all three can. It's \"will this \
output be read a second time?\" If yes → wiki.\n"
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

    /// The artifact store is the ONLY ingest path for deliverables — the
    /// desktop has no "add" button — so if this guidance stops naming it, an
    /// agent that produces a deck has nowhere to put it and the 产出 page
    /// silently stays empty. Both locales must steer the binary formats there
    /// and say why the wiki cannot take them.
    #[test]
    fn render_both_locales_route_deliverables_to_the_artifact_store() {
        for locale in ["zh", "en"] {
            let g = render_guidance(locale);
            assert!(
                g.contains("fleet__artifact"),
                "{locale}: guidance must name the MCP tool, not just the CLI"
            );
            assert!(
                g.contains("fleet artifact add"),
                "{locale}: guidance must show the CLI fallback"
            );
            for fmt in ["pptx", "xlsx", "PDF"] {
                assert!(g.contains(fmt), "{locale}: guidance must name {fmt}");
            }
            assert!(
                g.contains("worktree") || g.contains(".worktrees"),
                "{locale}: guidance must say why storing it late loses the file"
            );
        }
    }

    #[test]
    fn render_both_locales_delimit_wiki_from_its_cousins() {
        // Without this section an agent picks between the wiki, Artifact, and
        // the decision card at random, and durable output ends up scattered.
        for locale in ["en", "zh"] {
            let g = render_guidance(locale);
            assert!(g.contains("Artifact"), "{locale} guidance must name the Artifact tool");
            assert!(g.contains("fleet__ask"), "{locale} guidance must point one-off renders at the decision card");
            assert!(g.contains("fleet wiki cat"), "{locale} guidance must justify the wiki by read-back");
        }
    }
}
