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
/// [`chat_session_args`]), so it carries the whole brief.
const CHAT_CLAUDE_MD: &str = r#"# 纯聊天工作区 (managed by Claude Fleet — do not edit)

这是 Fleet 的纯聊天工作区。这里**没有代码库**，也不对应任何项目——老板来这儿是为了聊天：
问问题、聊想法、查东西、让你帮忙想清楚一件事。

## 怎么回话

像正常对话一样回话。直接给答案，然后再展开理由。不要写工作汇报，不要为一句话的问题套标题和分节，
不要把结论压缩成箭头链或缩写。该长就长，该一句话就一句话——由问题决定，不由格式决定。

老板称呼你时你就叫他「老板」。中文提问就中文回答。

**不要用决策卡**（`AskUserQuestion` / `fleet__ask`）来结束回合。聊天的回复就是普通文字，老板在会话里
直接读、直接回。只有在你真的卡住、需要老板在几个选项之间拍板时才值得弹卡。

## 这里不适用的东西

- **不要开 worktree、不要写 TASKS.md、不要拆 P1..Pn 计划。** 那套流程是给改代码用的，这里没有代码要改。
- **不要在这个目录里建文件**，除非老板明确要求。这是聊天工作区，不是草稿箱。真要写临时文件，
  用系统临时目录。
- **不要主动 git commit。** 这个目录不是 git 仓库。

## 工具

工具都在，该用就用——搜网、读文件、跑命令都行。老板聊着聊着让你顺手干点活是正常的，别因为"这是聊天
工作区"就拒绝。只是别把聊天变成一场工程仪式：能直接回答的就直接回答，不用先开一堆调查子任务。
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
