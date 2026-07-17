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
//! `--setting-sources project` does drop it — but it also drops the user
//! `settings.json` (Fleet's hooks) and `~/.claude.json` (the `fleet` MCP
//! server), which would blind Fleet to the session and, worse, make the CLI
//! abort at startup once `--permission-prompt-tool` names an MCP tool that no
//! longer resolves. [`chat_session_args`] therefore drops the user source and
//! hands both back explicitly via `--settings` / `--mcp-config`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::session::{get_claude_dir, get_fleet_dir};

/// Directory under `~/.fleet/` that backs the chat workspace.
const CHAT_DIR: &str = "chat";

/// Name the scanner reports for chat sessions instead of the `chat` basename
/// (see `session::workspace_name`). Kept ASCII so it reads the same in both
/// desktop locales and on the phone; the launcher UI labels its pinned entry
/// with a localized string of its own.
pub const CHAT_WORKSPACE_NAME: &str = "Chat";

/// Generated `--mcp-config` payload, refreshed on every chat spawn so it can't
/// go stale when the Fleet binary moves.
const CHAT_MCP_FILE: &str = "chat-mcp.json";

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
const CHAT_CLAUDE_MD: &str = r#"# 纯聊天工作区 (managed by Claw Fleet — do not edit)

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

**一次最多问一个问题**，而且先尽力回答再问。别用一串澄清问题把球踢回去。

**不要用决策卡**（`AskUserQuestion` / `fleet__ask`）结束回合。聊天的回复就是普通文字，老板直接
读、直接回。只有真正卡在需要他拍板的岔路口时，弹卡才有意义。

## 该画图的时候就画图

上面那条「散文优先」管的是**说话的口吻**——反对的是拿标题和 bullet 把一段本可以好好讲的话
装点成汇报。它不是让你把任何东西都压成文字。

判据是**内容本身长什么样**：如果它天然是个结构、流程、时序、依赖、对比或者数量关系，那就直接
画出来，而不是用一段话去描述那张图。渲染器支持这些，别浪费：

- **mermaid**（```mermaid 代码块）——**画标准结构图的首选**：架构图、流程图、时序图、状态机、
  甘特图、饼图。「A 调 B，B 再回调 A」这种关系，画一张 `sequenceDiagram` 胜过三句话。凡是
  mermaid 能表达的结构（流程/时序/状态/依赖），都优先用它。
- **表格**——两个以上的东西按同样几个维度比较时，表格是对的形状。
- **数学公式**（`$...$` 行内，`$$...$$` 独立成块）——涉及推导、增长率、复杂度就写公式，
  别用「n 的平方乘以 log n」这种话把式子念出来。
- **内联 HTML/SVG**——只在 mermaid 画不出来时才用：需要精确几何、自定义形状、物理结构示意
  （比如电路图里的电容极板、颜色方案、界面草图）。直接写一小段内联 HTML/SVG，它会真实渲染
  出来。它是 mermaid 的补充而非替代——别拿 SVG 去手画一张 mermaid 一行就能表达的流程图。

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
堆调查子任务。真有值得长期留存的产出，再用 `fleet wiki publish` 归档。
"#;

/// Absolute path of the chat workspace. `None` only when the home directory
/// can't be resolved at all.
pub fn chat_workspace_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join(CHAT_DIR))
}

/// True when `path` denotes the chat workspace. Compared with trailing
/// separators stripped, so both `~/.fleet/chat` and `~/.fleet/chat/` match —
/// the launcher round-trips this string through the UI and a stray slash must
/// not silently demote a chat session to an ordinary one.
pub fn is_chat_workspace(path: &str) -> bool {
    let Some(chat) = chat_workspace_path() else {
        return false;
    };
    let trim = |s: &str| s.trim_end_matches(['/', '\\']).to_string();
    trim(path) == trim(&chat.to_string_lossy())
}

/// Create the chat workspace if absent and (re)write its `CLAUDE.md`, then
/// return its absolute path. Rewriting on every call is deliberate: the file is
/// Fleet-managed, so an edited or truncated copy self-heals on the next spawn.
pub fn ensure_chat_workspace() -> Result<String, String> {
    let path = chat_workspace_path().ok_or_else(|| "no fleet dir".to_string())?;
    fs::create_dir_all(&path).map_err(|e| format!("create chat workspace: {e}"))?;
    let md = path.join("CLAUDE.md");
    // Only rewrite when the content actually differs — a chat session may be
    // reading this file while a sibling spawn ensures the workspace.
    let stale = fs::read_to_string(&md).map(|c| c != CHAT_CLAUDE_MD).unwrap_or(true);
    if stale {
        fs::write(&md, CHAT_CLAUDE_MD).map_err(|e| format!("write chat CLAUDE.md: {e}"))?;
    }
    Ok(path.to_string_lossy().to_string())
}

/// Write the `--mcp-config` payload mirroring the `fleet` MCP server currently
/// registered in `~/.claude.json`, returning its path. `None` when Fleet's MCP
/// injection isn't live — the caller then omits the flag, which lines up with
/// `session_launch::permission_prompt_tool_args` omitting
/// `--permission-prompt-tool` under the same condition.
fn write_chat_mcp_config() -> Option<String> {
    let entry = crate::mcp_injector::registered_fleet_entry()?;
    let dir = get_fleet_dir()?;
    let path = dir.join(CHAT_MCP_FILE);
    let payload = serde_json::json!({
        "mcpServers": { crate::mcp_injector::FLEET_SERVER_KEY: entry },
    });
    fs::write(&path, serde_json::to_string_pretty(&payload).ok()?).ok()?;
    Some(path.to_string_lossy().to_string())
}

/// Path of the user's `settings.json`, or `None` when it doesn't exist.
fn user_settings_path() -> Option<PathBuf> {
    let path = get_claude_dir()?.join("settings.json");
    Path::new(&path).is_file().then_some(path)
}

/// Extra `claude` args that turn a spawn inside the chat workspace into a chat:
/// exclude the user's global memory/doctrine, then hand back the two things
/// Fleet actually needs from that source — its hooks and its MCP server.
///
/// Each `--settings` / `--mcp-config` flag is added only when its file resolves,
/// so a host without them degrades to "no global CLAUDE.md" rather than failing
/// to launch.
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

pub fn chat_session_args() -> Vec<String> {
    let mut args = vec!["--setting-sources".to_string(), "project".to_string()];
    if let Some(settings) = user_settings_path() {
        args.push("--settings".to_string());
        args.push(settings.to_string_lossy().to_string());
    }
    if let Some(mcp) = write_chat_mcp_config() {
        args.push("--mcp-config".to_string());
        args.push(mcp);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FLEET_HOME` is process-global; serialize against the other suites that
    /// repoint it.
    fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", home) };
        let out = f();
        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }
        out
    }

    #[test]
    fn ensure_creates_dir_and_brief() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            let path = ensure_chat_workspace().unwrap();
            assert_eq!(path, tmp.path().join(".fleet/chat").to_string_lossy());
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
            // Prose-first governs *tone*; it must not be read as "never draw".
            // The renderer grew mermaid/math/HTML support precisely so a chat
            // can answer a structural question with a structure.
            assert!(body.contains("mermaid"), "must invite diagrams, not just prose");
            assert!(
                body.contains("别把行为归因于这份文件"),
                "must not blame behaviour on a file the user cannot see",
            );
        });
    }

    #[test]
    fn ensure_self_heals_a_clobbered_brief() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            ensure_chat_workspace().unwrap();
            let md = tmp.path().join(".fleet/chat/CLAUDE.md");
            std::fs::write(&md, "garbage").unwrap();
            ensure_chat_workspace().unwrap();
            assert_eq!(std::fs::read_to_string(&md).unwrap(), CHAT_CLAUDE_MD);
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

    #[test]
    fn chat_session_args_always_drop_the_user_setting_source() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            // No ~/.claude/settings.json and no registered MCP server on this
            // synthetic home: the flag pair must degrade, not panic.
            let args = chat_session_args();
            assert_eq!(args, vec!["--setting-sources", "project"]);
        });
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

    #[test]
    fn chat_session_args_hand_back_settings_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        with_home(tmp.path(), || {
            let claude = tmp.path().join(".claude");
            std::fs::create_dir_all(&claude).unwrap();
            let settings = claude.join("settings.json");
            std::fs::write(&settings, "{}").unwrap();
            let args = chat_session_args();
            let i = args.iter().position(|a| a == "--settings").expect("--settings");
            assert_eq!(args[i + 1], settings.to_string_lossy());
        });
    }
}
