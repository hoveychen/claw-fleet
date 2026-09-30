//! Commit-trailer interception: `git commit` calls that hand-write a Claude
//! byline after the user turned bylines off.
//!
//! `includeCoAuthoredBy: false` (every Fleet launch sets it, see [`crate::claude_launch`])
//! only removes the line Claude Code *injects* into its own system prompt. It
//! cannot stop the model from copying the trailer out of the repo's history.
//! Observed 2026-09-29 in `anatole-mono` commit `5fd0a96`: the session ran
//! `git show a36cf49` to reuse a teammate's `deploy: 1.0.141 …` message, and the
//! teammate's commit carried `Co-Authored-By: Claude …`, so the new
//! `deploy: 1.0.142 …` commit carried it too — the only one of the user's ~30
//! recent commits in that repo to do so. Settings cannot reach that; only a
//! check at the moment the command runs can.
//!
//! The check is textual, on the Bash command itself. A message passed through
//! `-F <file>` or an editor is not seen; the observed failure (and Claude Code's
//! own commit recipe) puts the message inline, which is the case covered here.

/// Reason returned to the model with the denial.
pub const DENY_REASON: &str = "Fleet: this `git commit` message contains a Claude attribution line (`Co-Authored-By: Claude …` / `noreply@anthropic.com` / `Generated with Claude Code`). The user has disabled commit attribution — do not copy that trailer from earlier commits in the history. Re-run the same commit with the attribution line removed and everything else unchanged.";

/// Some shell command in `command` is a `git` invocation whose subcommand is
/// `commit`, allowing global options in between (`git -C dir commit`,
/// `git -c k=v commit`). Commands are split on newlines and shell separators,
/// so `git status && make commit` does not count.
fn runs_git_commit(command: &str) -> bool {
    command
        .split(|c| matches!(c, '\n' | ';' | '&' | '|' | '(' | ')'))
        .any(|segment| {
            let mut tokens = segment.split_whitespace();
            if !tokens
                .next()
                .is_some_and(|t| t == "git" || t.ends_with("/git"))
            {
                return false;
            }
            while let Some(tok) = tokens.next() {
                match tok {
                    "-C" | "-c" => {
                        tokens.next();
                    }
                    t if t.starts_with('-') => {}
                    t => return t == "commit",
                }
            }
            false
        })
}

/// Some line of `command` attributes a commit to Claude.
fn has_attribution_line(command: &str) -> bool {
    command.lines().any(|line| {
        let l = line.to_lowercase();
        let head =
            l.trim_start_matches(|c: char| c.is_whitespace() || c == '>' || c == '"' || c == '\'');
        (head.starts_with("co-authored-by:") && (l.contains("claude") || l.contains("anthropic")))
            || l.contains("generated with [claude code")
            || l.contains("generated with claude code")
    })
}

/// True when `command` is a `git commit` whose inline message carries a Claude
/// attribution line. Pure.
pub fn has_claude_trailer(command: &str) -> bool {
    runs_git_commit(command) && has_attribution_line(command)
}

/// Deny reason for this call, or `None` to let it through. `trailers_disabled`
/// reads the user's settings — a user who keeps bylines on is never blocked. It
/// is only called once the command matches, so ordinary Bash calls never touch
/// settings.json.
pub fn decide(
    tool_name: Option<&str>,
    command: Option<&str>,
    trailers_disabled: impl FnOnce() -> bool,
) -> Option<String> {
    if tool_name != Some("Bash") || !has_claude_trailer(command?) || !trailers_disabled() {
        return None;
    }
    Some(DENY_REASON.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact command from the 2026-09-29 incident.
    const INCIDENT: &str = "git fetch -q && sed -i '' 's#a#b#' deploy/docker-compose.muvee.yml && git diff --stat && git commit -qam \"deploy: 1.0.142（ChatGPT MCP 连接器），拨 recreate-rev\n\nCo-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>\" && git push origin main 2>&1 | tail -1";

    #[test]
    fn denies_the_incident_command() {
        assert!(has_claude_trailer(INCIDENT));
        assert_eq!(
            decide(Some("Bash"), Some(INCIDENT), || true).as_deref(),
            Some(DENY_REASON)
        );
    }

    #[test]
    fn heredoc_message_and_global_options_are_caught() {
        let cmd = "git -C /repo commit -m \"$(cat <<'EOF'\nfix: x\n\nCo-authored-by: claude <noreply@anthropic.com>\nEOF\n)\"";
        assert!(has_claude_trailer(cmd));
        let generated = "git commit -m 'feat: y\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)'";
        assert!(has_claude_trailer(generated));
    }

    #[test]
    fn clean_commits_and_human_coauthors_pass() {
        assert!(!has_claude_trailer("git commit -qam \"deploy: 1.0.143\""));
        assert!(!has_claude_trailer(
            "git commit -m \"fix\n\nCo-Authored-By: Kaiya <kaiya@flab.ai>\""
        ));
    }

    #[test]
    fn non_commit_commands_mentioning_the_trailer_pass() {
        // Inspecting history or grepping for the trailer is not writing it.
        assert!(!has_claude_trailer(
            "git log --grep='Co-Authored-By: Claude'"
        ));
        assert!(!has_claude_trailer(
            "git show a36cf49 | grep 'Co-Authored-By: Claude'"
        ));
        assert!(!has_claude_trailer(
            "git status && make commit # Co-Authored-By: Claude"
        ));
    }

    #[test]
    fn respects_settings_and_tool_name() {
        assert!(decide(Some("Bash"), Some(INCIDENT), || false).is_none());
        assert!(decide(Some("Read"), Some(INCIDENT), || true).is_none());
        assert!(decide(Some("Bash"), None, || true).is_none());
    }
}
