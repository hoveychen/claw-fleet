//! The pure-chat workspace: `~/.fleet/chat`.
//!
//! Fleet has no `Workspace` type — a workspace *is* a `claude` process's cwd,
//! recovered by the scanner from `~/.claude/projects/<encoded-cwd>/`. So "a
//! session not tied to any project" still needs a real directory to stand in,
//! because [`crate::session_launch`] refuses to spawn into a path that isn't a
//! directory. This module owns that directory and the launch flags that make a
//! session inside it behave like a chat rather than a coding agent.
//!
//! ## Why the launch flags exist
//!
//! `~/.claude/CLAUDE.md` (plus its `@`-imports) carries the user's engineering
//! doctrine — worktree workflow, PRD discipline, test-first, decision cards.
//! Measured at ~22k tokens. All of it is noise in a chat, and a project-level
//! `CLAUDE.md` cannot cancel it: memory files are **concatenated, not
//! overridden** (verified: a project CLAUDE.md reading "ignore all global
//! instructions" still left the global doctrine fully present in context).
//!
//! `--setting-sources project` does drop it — along with the user
//! `settings.json` and `~/.claude.json`. Fleet's hooks and MCP server no longer
//! live there anyway: like every Fleet session, a chat gets them from
//! `claude_launch::fleet_launch_args_for`, which sees the excluded user layer
//! and hands every piece over (minus the system-prompt guidance, which this
//! workspace's brief replaces).

use std::fs;
use std::path::{Path, PathBuf};

use crate::session::get_fleet_dir;

/// Directory under `~/.fleet/` that backs the chat workspace.
const CHAT_DIR: &str = "chat";

/// Name the scanner reports for chat sessions instead of the `chat` basename
/// (see `session::workspace_name`). Kept ASCII so it reads the same in both
/// desktop locales and on the phone; the launcher UI labels its pinned entry
/// with a localized string of its own.
pub const CHAT_WORKSPACE_NAME: &str = "Chat";

/// The chat workspace's own `CLAUDE.md`. This is the *only* memory file a chat
/// session loads (the user's global doctrine is excluded by
/// [`chat_session_args`]), so it carries the whole brief — including the things
/// the global file would otherwise supply, like how to address the user and
/// which language to answer in. Assume nothing else is loaded.
///
/// The rules below deliberately track the design of Anthropic's own published
/// claude.ai system prompt (platform.claude.com/docs/en/release-notes/system-prompts):
/// prose over bullets, no engagement-farming, don't blame your behaviour on a
/// file the user can't see, own mistakes without grovelling, default to helping.
/// A coding agent's habits are the wrong defaults for a conversation.
const CHAT_CLAUDE_MD: &str = r##"# 纯聊天工作区 (managed by Claw Fleet — do not edit)

这是 Fleet 的纯聊天工作区。这里没有代码库，也不对应任何项目——老板来这儿是为了聊天：问问题、
聊想法、查东西、让你帮忙把一件事想清楚。

用中文回答（老板用中文问的话），称呼他「老板」。

## 怎么说话

**散文优先。** 正常对话和简单问题就用平常的口吻直接答，几句话就够了不必凑长。解释、分析、
调研结论这类内容也写成连贯的段落，而不是一堆标题加 bullet。要列举时，就在句子里列——
「大概有三条路：A、B、C」——而不是换行打点。

**只在真正需要时才用列表和格式。** 判据是内容本身是否多面到非结构化不可，而不是「这样看起来
更专业」。真要用 bullet，每条至少写成一两句完整的话，别退化成关键词碎片。**拒绝或否定老板的
想法时，绝不要用 bullet**——那种时候更需要好好说话。

**每句话都要带来老板还没有的东西。** 下面这些写法是在「显得有分量」，不是在说事。写完自查一遍，看到就改成直接陈述：

- 「不是 X，而是 Y」「不仅……更是……」「与其说……不如说……」：没人主张过 X，就直接说 Y。只有在纠正老板确实持有的看法时才保留这种对比。
- 段尾一句话的收束（「这才是关键」「这一点很重要」），以及换个说法复述上文：删掉，停在最后一个具体事实上。
- 「本质上」「说到底」「核心在于」这类套话，「先说结论」「下面我来拆解」这类铺垫：删掉，直接说。
- 反驳没人提过的观点（「需要说明的是，这并不是说……」）：删掉。
- 把普通事实拔高（「具有里程碑意义」「标志着」「未来可期」）或写成推销腔：只留事实。
- 为了显得完整硬凑三项：有几项写几项。

**加粗只留给真正需要老板停下来看的一两处**，不要给每个要点都加粗标签。

**不要加笔记和对话里没有的事实。** 数字、名字、日期、因果只写有依据的；缺细节就写得简单些，或直说不知道，不要拿听起来合理的推测补上。

**回复是写给已经知道背景的人的。** 老板自己说过的背景、刚问的问题不用重讲，结论放在第一句。但支撑结论的数字和证据要留全，砍的是复述和铺垫，不是论据。

**不要描述你自己的写法**，比如「下表对比了……」「以下按……组织」「我把不确定的都标出来了」。

**一次最多问一个问题**，而且先尽力回答再问。别用一串澄清问题把球踢回去。

**不要用决策卡**（`AskUserQuestion` / `fleet__ask`）结束回合。聊天的回复就是普通文字，老板直接
读、直接回。只有真正卡在需要他拍板的岔路口时，弹卡才有意义。

## 该画图的时候就画图

上面那条「散文优先」管的是**说话的口吻**——反对的是拿标题和 bullet 把一段本可以好好讲的话
装点成汇报。它不是让你把任何东西都压成文字。

判据是**内容本身长什么样**：如果它天然是个结构、流程、时序、依赖、对比或者数量关系，那就直接
画出来，而不是用一段话去描述那张图。渲染器支持这些，别浪费：

- **mermaid**（```mermaid 代码块）——标准结构图交给它：架构图、流程图、时序图、状态机、甘特图、
  饼图。它声明式、自己排版，你写 `A-->B` 就得到一张框不重叠、箭头不错位的图，省掉手算坐标
  这件最容易出错的活。凡是要自动布局的标准图（流程/时序/状态/依赖），走 mermaid 最省心。
- **表格**——两个以上的东西按同样几个维度比较时，表格是对的形状。
- **数学公式**（`$...$` 行内，`$$...$$` 独立成块）——涉及推导、增长率、复杂度就写公式，
  别用「n 的平方乘以 log n」这种话把式子念出来。
- **内联 HTML/SVG**——mermaid 画不出来的，就大方用它：电路、物理结构、自定义插画、颜色方案、
  界面草图这类需要精确摆放或自定义形状的东西。直接写一小段内联 HTML/SVG，它会真实渲染出来，
  别因为「手写标签麻烦」就退回用一段话去描述那张图。它和 mermaid 不分高下、只是分工不同：要
  自动排版的标准图用 mermaid，要精确摆放的自定义图形用 SVG，各取所长。

聊天区会对 HTML 做消毒，背景可能是深色也可能是浅色，所以内联 SVG 有几条硬约束：

- `<svg …>` 开标签**单独占一行**，直接写在正文里，不要包进 ```html 代码块，也不要写成完整的
  HTML 文档。
- 不用 `<style>`、`class`、`style="…"`，颜色、字体、字号全部写成 `fill`/`stroke`/`font-size`
  这类属性。
- 第一个元素铺一块不透明底：`<rect width="100%" height="100%" fill="#f5f5f5"/>`，前景用深色
  （如 `#2d3142`），别假设背景是白的或黑的。
- 箭头头部直接画成小三角 `<path>`，不要用 `<marker>`（有的显示端会让它引用的 id 失效）。
- 宽度不超过 860，文字离框边至少留 8px，中文按每字约等于字号的宽度估算，别让字溢出框。

画任何图（mermaid 或 SVG）都按「删到不能再删」来：一张图大约 9 个节点以内，多了就拆成概览加
细节两张；只给 1–2 个真正的焦点上强调色；连线横平竖直，标签别压在线上；图例横排放在图下方。
流程题先给一张主干图，分支细节用文字或表格补，别连画几张需要滚屏的长判定树。
mermaid 别用 classDef/style 自定浅色填充配白字，配色交给默认主题；时序图的参与者 id 别用
`loop`、`alt`、`opt`、`par`、`end`、`note` 这类关键字，起个英文短名，再用 `as` 写显示名。

反过来也成立：一句话能说清的事别硬画成图，为了炫技而堆图表和为了显专业而堆 bullet 是同一种毛病。
标准始终是**哪种形式让老板更快看懂**。

## 别黏人

不要培养老板对你的依赖，也不要经营「继续聊下去」这件事。具体说：**不要为他来找你而道谢**，
不要在结尾问「还有什么我可以帮你的吗」，不要说「随时来找我」「期待和你继续探讨」这类话。
该结束就干净地结束。如果某件事更该去找真人、找专业渠道，就直说。

## 关于你自己

**别把行为归因于这份文件。** 不要说「我的 CLAUDE.md 要求我……」「按照我的指令我不能……」。
老板看不到这份文件，这么说既让人困惑，又是拿隐藏规则替掉了你真实的理由。有理由就直接讲理由。

**认错要利索，但别自我作践。** 说错了就承认、就改，然后继续把问题解决掉。不要连声道歉，不要
因为被指出一个错误就把之前所有判断都推翻——那不是诚实，那是投降。老板质疑你时，如果你认为
自己是对的，就把依据摆出来。

**默认帮忙。** 只有当帮忙会造成具体而严重的伤害时才拒绝。仅仅是问题尖锐、假设性、开玩笑，
或者让你觉得不太舒服，都不构成拒绝的理由。

## 这里不适用的东西

这个目录不是代码仓库，那套工程流程在这儿一条都不适用：

不要开 worktree，不要写 TASKS.md，不要把一次闲聊拆成 P1..Pn 计划，不要主动 git commit。
也不要在这个目录里建文件——除非老板明确要求。需要临时文件就用系统临时目录。

## 工具

工具都在，该用就用：搜网、读文件、跑命令都行。老板聊着聊着让你顺手干点活是完全正常的，别因为
「这是聊天工作区」就推辞。只是别把聊天变成工程仪式——能直接回答的就直接回答，不用动辄先开一
堆调查子任务。

## 做出来的东西别留在临时目录

聊着聊着真做出了一份东西——一篇译稿、一份报告、一张图表、一段视频、一个数据集——它就不该只躺在
`/tmp` 里等着被清掉。**判据是这份东西给谁**，而不是它是什么格式：

- **递到老板手上的成品**（要读、要转发、要存档的：PDF、幻灯片、表格、图片、视频，也包括一份对外的
  md 或 html）→ 用 `fleet__artifact` 的 `action="add"`（CLI 等价 `fleet artifact add`）存进**产出库**。
  它不挑格式，二进制照收。`--title` 和 `--note` 认真写，那两行就是老板在卡片上读到的全部。
- **留给你和后面接手的会话的文字**（调研笔记、架构梳理、踩坑记录这类要被再读一遍的）→ 用
  `fleet__wiki` 的 `action="publish"`（CLI 等价 `fleet wiki publish`）存进**知识库**。它只认
  html/htmlDir/markdown，所以 PDF、xlsx 这类二进制**只能**走产出库。

拿不准就问一句：这份东西是递给人的，还是留给自己的？递 → 产出库，留 → 知识库。真是过目即弃的
中间产物，两个都不用，留在临时目录就行。
"##;

/// The full brief written to the chat workspace's `CLAUDE.md`: the static
/// [`CHAT_CLAUDE_MD`] plus the session-title section when that feature is
/// on.
///
/// The title instruction has to be appended here because otherwise it only
/// reaches a session inside the engineering guidance, which a chat launch
/// deliberately goes without — so before this, chat sessions
/// never learned to name themselves and their titles fell all the way back to a
/// raw prompt excerpt (`ai_title ?? slug ?? last_message_preview`, and Claude
/// Code stopped writing `ai-title` on 2026-09-06). The wording still has a
/// single owner in [`crate::session_title_guidance`]; this only relocates it.
///
/// The inline `[?…]` marks section rides along the same way: it lives inside
/// the interaction-mode guidance and is lifted from there by
/// [`crate::explain_marks_guidance::enabled_section`],
/// so a chat session marks its prose exactly like an engineering session does
/// — the boss's screenshot that motivated the feature was a chat session.
fn chat_claude_md() -> String {
    let mut out = CHAT_CLAUDE_MD.to_string();
    if let Some(section) = crate::session_title_guidance::enabled_section() {
        out.push('\n');
        out.push_str(&section);
        out.push('\n');
    }
    if let Some(section) = crate::explain_marks_guidance::enabled_section() {
        out.push('\n');
        out.push_str(&section);
        out.push('\n');
    }
    out
}

/// Where the chat workspace is *created*: straight under the fleet dir, whose
/// own path may still contain symlinks. Writes go here; identity comparisons go
/// through [`chat_workspace_path`].
fn chat_workspace_link_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join(CHAT_DIR))
}

/// Resolve `path` through symlinks, or `None` when it doesn't exist yet.
///
/// On Windows `canonicalize` hands back a `\\?\`-prefixed verbatim path, which
/// no other Fleet surface ever produces — strip it so the result still compares
/// equal to an ordinary path string.
fn resolved(path: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let text = canonical.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) => Some(PathBuf::from(stripped)),
        None => Some(canonical),
    }
}

/// Absolute path of the chat workspace, with symlinks resolved once it exists.
/// `None` only when the home directory can't be resolved at all.
///
/// Resolving matters wherever `~/.fleet` is itself a link — Fleet Cloud's
/// entrypoint points it into the single persistent volume
/// (`/home/fleet/.fleet -> /workspace/.fleet-state`), and macOS resolves
/// `/var` to `/private/var`. A process spawned with cwd `~/.fleet/chat` reports
/// the resolved path (`getcwd(2)` keeps no symlinks), and that is the path the
/// scanner and the launcher's recents list carry, so it has to be the one this
/// function hands out too.
pub fn chat_workspace_path() -> Option<PathBuf> {
    let raw = chat_workspace_link_path()?;
    Some(resolved(&raw).unwrap_or(raw))
}

/// Resolve the path the launcher should display without initialising the
/// workspace. Opening the new-session form is a read-only UI action: directory
/// creation and brief repair belong to the actual spawn path, which already
/// calls [`ensure_chat_workspace`]. Keeping them out of this lookup prevents a
/// slow disk from delaying the chat-mode control itself.
pub fn chat_workspace_for_ui() -> Result<String, String> {
    chat_workspace_path()
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "no fleet dir".to_string())
}

/// True when `path` denotes the chat workspace. Both the link path
/// (`~/.fleet/chat`) and its resolved form match, and trailing separators are
/// stripped, so neither a symlinked fleet dir nor a stray slash round-tripped
/// through the UI can silently demote a chat session to an ordinary one.
pub fn is_chat_workspace(path: &str) -> bool {
    let trim = |s: &str| s.trim_end_matches(['/', '\\']).to_string();
    let target = trim(path);
    // Cheap prefilter: the workspace directory is always named `chat`, so
    // anything else is out without touching the filesystem. `workspace_name`
    // calls this for every scanned session — the canonicalize below must not
    // ride on that path.
    if Path::new(&target).file_name().and_then(|n| n.to_str()) != Some(CHAT_DIR) {
        return false;
    }
    let Some(raw) = chat_workspace_link_path() else {
        return false;
    };
    if target == trim(&raw.to_string_lossy()) {
        return true;
    }
    resolved(&raw).is_some_and(|r| target == trim(&r.to_string_lossy()))
}

/// Create the chat workspace if absent and (re)write its `CLAUDE.md`, then
/// return its absolute path — resolved, because the launcher pins this string
/// and dedups its recents list against it (see [`chat_workspace_path`]).
/// Rewriting the brief on every call is deliberate: the file is Fleet-managed,
/// so an edited or truncated copy self-heals on the next spawn.
pub fn ensure_chat_workspace() -> Result<String, String> {
    let path = chat_workspace_link_path().ok_or_else(|| "no fleet dir".to_string())?;
    fs::create_dir_all(&path).map_err(|e| format!("create chat workspace: {e}"))?;
    // Only resolvable after create_dir_all, hence not `chat_workspace_path()`
    // above.
    let path = resolved(&path).unwrap_or(path);
    let md = path.join("CLAUDE.md");
    // Only rewrite when the content actually differs — a chat session may be
    // reading this file while a sibling spawn ensures the workspace. The
    // comparison is against the *composed* brief, so renaming the user,
    // switching locale or toggling the session-title feature off all self-heal
    // on the next spawn.
    let brief = chat_claude_md();
    let stale = fs::read_to_string(&md).map(|c| c != brief).unwrap_or(true);
    if stale {
        fs::write(&md, &brief).map_err(|e| format!("write chat CLAUDE.md: {e}"))?;
    }
    Ok(path.to_string_lossy().to_string())
}

/// [`chat_session_args`] when `workspace_path` is the chat workspace, empty
/// otherwise. Every spawn site (new session, resume, handoff, the mobile relay)
/// runs its args through this, so a chat stays a chat across all of its turns —
/// a resume that skipped these flags would silently reload the 22k-token
/// doctrine on turn two and contradict the brief the first turn was given.
pub fn chat_launch_args(workspace_path: &str) -> Vec<String> {
    if is_chat_workspace(workspace_path) {
        chat_session_args()
    } else {
        Vec::new()
    }
}

/// Extra `claude` args that turn a spawn inside the chat workspace into a chat:
/// exclude the user's global memory/doctrine.
pub fn chat_session_args() -> Vec<String> {
    vec!["--setting-sources".to_string(), "project".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FLEET_HOME` is process-global; serialize against the other suites that
    /// repoint it.
    fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        // Guard, not a hand-rolled restore: an assert inside `f` unwinds past
        // any restore written after the call, leaving every later test in this
        // process pointed at a home that no longer exists.
        let _guard = crate::paths::fleet_home_guard(home);
        f()
    }

    /// Point `CLAUDE_CONFIG_DIR` at a temp dir for the duration of `f`, so the
    /// composed brief is decided by *this* fixture's session-title guidance file
    /// and not by whatever the developer has installed in their real
    /// `~/.claude`. Restored even on panic. Shares `fleet_home_lock` with
    /// [`with_home`], which is the process-global serialisation both overrides
    /// need.
    fn with_claude_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
        struct Guard(Option<std::ffi::OsString>);
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe {
                    match &self.0 {
                        Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
                        None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
                    }
                }
            }
        }
        let _guard = Guard(std::env::var_os("CLAUDE_CONFIG_DIR"));
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", dir) };
        f()
    }

    /// Regression (2026-09-08): a chat session showed up in the task list titled
    /// with a raw excerpt of its own prompt. `chat_session_args` drops the
    /// user's global `CLAUDE.md`, which is the only place the "call
    /// `fleet__set_session_title`" instruction lives, so the agent never learned
    /// to name itself — and Claude Code stopped writing its own `ai-title`
    /// record on 2026-09-06, leaving `last_message_preview` as the only
    /// fallback. Verified in the transcript at the time: `会话标题` (session title) appeared 0
    /// times and the tool was never called.
    #[test]
    fn brief_carries_the_session_title_instruction_when_installed() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            with_claude_dir(&tmp.path().join(".claude"), || {
                crate::claude_launch::reconcile_guidance("老板", "zh").unwrap();
                ensure_chat_workspace().unwrap();
                let body =
                    std::fs::read_to_string(tmp.path().join(".fleet/chat/CLAUDE.md")).unwrap();
                assert!(body.contains("纯聊天工作区"), "still the chat brief");
                assert!(body.contains("## 会话标题"), "section heading appended");
                assert!(
                    body.contains("fleet__set_session_title"),
                    "the tool the agent has to call"
                );
                assert!(
                    !body.contains("# Fleet 会话标题"),
                    "the guidance file's own top-level header must not be embedded"
                );
            });
        });
    }

    /// The `[?…]` marks section lives in the interaction-mode file, which the
    /// chat launch flags drop with the rest of `~/.claude/*.md`; the brief has
    /// to carry it itself or chat replies never get marks — and a chat session
    /// is exactly what the boss was reading when they asked for the feature.
    #[test]
    fn brief_carries_the_explain_marks_section_when_interaction_mode_is_installed() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            with_claude_dir(&tmp.path().join(".claude"), || {
                crate::claude_launch::reconcile_guidance("老板", "zh").unwrap();
                ensure_chat_workspace().unwrap();
                let body =
                    std::fs::read_to_string(tmp.path().join(".fleet/chat/CLAUDE.md")).unwrap();
                assert!(body.contains("纯聊天工作区"), "still the chat brief");
                assert!(
                    body.contains("## 正文标注 `[?…]`"),
                    "section heading appended"
                );
                assert!(body.contains("最多 5 处"));
                assert!(
                    !body.contains("# Fleet 交互模式"),
                    "only the marks section is lifted, not the whole interaction-mode file"
                );
                assert!(
                    !body.contains("## 三种卡"),
                    "the decision-card rules must not leak into the chat brief"
                );
            });
        });
    }

    /// The settings-panel toggle stays authoritative: a switched-off feature
    /// contributes no section.
    #[test]
    fn brief_omits_the_section_when_the_feature_is_off() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            with_claude_dir(&tmp.path().join(".claude"), || {
                use crate::control_plane_prefs::{mark_disabled, Feature};
                mark_disabled(Feature::SessionTitleGuidance).unwrap();
                mark_disabled(Feature::InteractionMode).unwrap();
                ensure_chat_workspace().unwrap();
                let body =
                    std::fs::read_to_string(tmp.path().join(".fleet/chat/CLAUDE.md")).unwrap();
                assert_eq!(body, CHAT_CLAUDE_MD);
            });
        });
    }

    #[test]
    fn ensure_creates_dir_and_brief() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            let path = ensure_chat_workspace().unwrap();
            // Resolved, not the raw join: on macOS the tempdir lives under
            // `/var/folders/...`, which is itself a symlink into `/private`.
            // See `chat_workspace_path` for why callers get the resolved form.
            let expected = std::fs::canonicalize(tmp.path().join(".fleet/chat")).unwrap();
            assert_eq!(path, expected.to_string_lossy());
            let md = tmp.path().join(".fleet/chat/CLAUDE.md");
            assert!(md.is_file(), "chat CLAUDE.md must be written");
            let body = std::fs::read_to_string(&md).unwrap();
            assert!(body.contains("纯聊天工作区"));
            // The brief must actively cancel the doctrine the chat session no
            // longer loads, or the model falls back to coding-agent habits.
            assert!(body.contains("worktree"));
            // It is also the ONLY memory file a chat session sees, so it must
            // carry what the excluded global file would have supplied.
            assert!(body.contains("老板"), "must carry the form of address");
            // The load-bearing chat-mode rules, tracked from Anthropic's own
            // published claude.ai prompt. Losing these silently turns the chat
            // back into a coding agent that farms engagement.
            assert!(body.contains("散文优先"), "prose-over-bullets rule");
            assert!(body.contains("别黏人"), "no engagement-farming rule");
            // The anti-staging rules (no "不是X而是Y", no closers, no invented
            // facts) won a blind A/B against prose-first alone; see wiki
            // skill-bench/humanizer.
            assert!(body.contains("每句话都要带来老板还没有的东西"), "anti-staging rules");
            assert!(body.contains("支撑结论的数字和证据要留全"), "brevity must not drop evidence");
            // Prose-first governs *tone*; it must not be read as "never draw".
            // The renderer grew mermaid/math/HTML support precisely so a chat
            // can answer a structural question with a structure.
            assert!(
                body.contains("mermaid"),
                "must invite diagrams, not just prose"
            );
            assert!(
                body.contains("别把行为归因于这份文件"),
                "must not blame behaviour on a file the user cannot see",
            );
        });
    }

    #[test]
    fn ui_path_lookup_does_not_create_the_chat_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            let expected = tmp.path().join(".fleet/chat");
            let path = chat_workspace_for_ui().unwrap();
            assert_eq!(path, expected.to_string_lossy());
            assert!(
                !expected.exists(),
                "opening the launcher must not initialise the chat workspace",
            );
        });
    }

    #[test]
    fn ensure_self_heals_a_clobbered_brief() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            // Isolate the claude dir so the expected brief is this fixture's,
            // not the developer's installed session-title guidance.
            with_claude_dir(&tmp.path().join(".claude"), || {
                ensure_chat_workspace().unwrap();
                let md = tmp.path().join(".fleet/chat/CLAUDE.md");
                std::fs::write(&md, "garbage").unwrap();
                ensure_chat_workspace().unwrap();
                assert_eq!(std::fs::read_to_string(&md).unwrap(), chat_claude_md());
            });
        });
    }

    #[test]
    fn is_chat_workspace_matches_with_and_without_trailing_slash() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            let chat = tmp.path().join(".fleet/chat");
            let chat = chat.to_string_lossy().to_string();
            assert!(is_chat_workspace(&chat));
            assert!(is_chat_workspace(&format!("{chat}/")));
            assert!(!is_chat_workspace("/Users/foo/my-project"));
            // A sibling under ~/.fleet must not be mistaken for it.
            assert!(!is_chat_workspace(&format!("{chat}-other")));
        });
    }

    /// Regression: on a host where `~/.fleet` is a **symlink** (Fleet Cloud's
    /// entrypoint links it into the one persistent volume, e.g.
    /// `/home/fleet/.fleet -> /workspace/.fleet-state`), a chat session spawned
    /// with cwd `~/.fleet/chat` reports the *resolved* path — `getcwd(2)` has no
    /// symlinks left, and Claude Code encodes that resolved path into
    /// `~/.claude/projects/`. A raw string compare against the link path then
    /// fails, so the scanner labelled the chat workspace "chat" instead of
    /// "Chat" and the launcher listed it a second time next to its own pinned
    /// entry.
    #[cfg(unix)]
    #[test]
    fn is_chat_workspace_sees_through_a_symlinked_fleet_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::os::unix::fs::symlink(&state, home.join(".fleet")).unwrap();
        with_home(&home, || {
            let returned = ensure_chat_workspace().unwrap();
            // What the spawned process (and therefore the scanner) will report.
            let resolved = std::fs::canonicalize(state.join(CHAT_DIR)).unwrap();
            let resolved = resolved.to_string_lossy().to_string();
            assert!(
                is_chat_workspace(&resolved),
                "resolved chat path {resolved} must still be the chat workspace",
            );
            // The link path stays valid too — drafts and older clients hold it.
            assert!(is_chat_workspace(
                &home.join(".fleet/chat").to_string_lossy()
            ));
            // The launcher pins whatever this hands back and dedups the recents
            // against it by string equality, so it must be the resolved form.
            assert_eq!(returned, resolved);
        });
    }

    #[test]
    fn chat_session_args_only_drop_the_user_setting_source() {
        // Fleet's hooks and MCP server come from `claude_launch`, not from a
        // copy of the user's global settings.
        assert_eq!(chat_session_args(), vec!["--setting-sources", "project"]);
    }

    #[test]
    fn chat_launch_args_are_empty_outside_the_chat_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            assert!(chat_launch_args("/Users/foo/my-project").is_empty());
            let chat = tmp.path().join(".fleet/chat");
            assert!(!chat_launch_args(&chat.to_string_lossy()).is_empty());
        });
    }
}
