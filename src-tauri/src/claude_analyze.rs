//! Lightweight wrapper around `claude -p` for semantic analysis of agent output.
//!
//! Classifies the agent's last output into outcome tags (e.g. bug_fixed,
//! needs_input, celebrating) and optionally produces a short summary when the
//! agent is blocked waiting for user input.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Truncate a string to at most `max` bytes on a valid UTF-8 char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

use crate::log_debug;

const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INPUT_CHARS: usize = 1000;

/// All recognised outcome tags.  Keep in sync with the prompt and the
/// frontend `SessionOutcome` type.
pub const VALID_TAGS: &[&str] = &[
    "needs_input",
    "bug_fixed",
    "feature_added",
    "stuck",
    "apologizing",
    "show_off",
    "concerned",
    "confused",
    "celebrating",
    "quick_fix",
    "overwhelmed",
    "scheming",
    "reporting",
];

/// Result of analysing the last assistant output.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// 1–2 outcome tags (see [`VALID_TAGS`]).
    pub tags: Vec<String>,
    /// Human-readable summary — only set when `needs_input` is among the tags.
    pub summary: Option<String>,
}

// ── Prompt ──────────────────────────────────────────────────────────────────

fn build_prompt(last_text: &str, locale: &str) -> String {
    let lang_instruction = match locale {
        "zh" => "Summary 部分用中文回复。",
        "ja" => "Summary 部分は日本語で回答してください。",
        "ko" => "Summary 부분은 한국어로 답변해 주세요.",
        _ => "Write the summary in English.",
    };

    format!(
        "Below is the last output from a coding assistant. Classify its outcome \
         by picking **1–2 tags** from this list:\n\
         \n\
         - needs_input  : the assistant is BLOCKED — it explicitly asked a question or presented options it needs the user to answer before it can proceed\n\
         - bug_fixed    : successfully fixed a bug or resolved an issue\n\
         - feature_added: successfully added new functionality\n\
         - stuck        : tried but could not solve the problem\n\
         - apologizing  : acknowledging a mistake or apologising\n\
         - show_off     : proudly explaining an elegant or clever solution\n\
         - concerned    : warning about potential issues, risks, or code smells\n\
         - confused     : uncertain about unexpected behaviour\n\
         - celebrating  : completed a major task, all tests passing, mission accomplished\n\
         - quick_fix    : trivially solved a simple problem in very few steps\n\
         - overwhelmed  : had to make extensive changes across many files\n\
         - scheming     : presenting a plan or strategy for upcoming work\n\
         - reporting    : neutral status update or informational summary\n\
         \n\
         Rules:\n\
         - Pick the 1–2 MOST relevant tags.  If two tags overlap heavily, pick just one.\n\
         - `needs_input` means the assistant **cannot continue** without the user's reply.  Do NOT pick it if the assistant merely suggests the user test/verify something.\n\
         - If nothing fits well, use `reporting`.\n\
         \n\
         Response format (exactly one line):\n\
         - If `needs_input` is one of the tags:\n\
           TAGS: needs_input[,other_tag] | SUMMARY: <one sentence under 80 chars describing what is needed>\n\
         - Otherwise:\n\
           TAGS: tag1[,tag2]\n\
         \n\
         {lang_instruction}\n\
         \n\
         ---\n\
         {text}",
        lang_instruction = lang_instruction,
        text = last_text,
    )
}

// ── CLI resolution ──────────────────────────────────────────────────────────

fn resolve_claude_path() -> Option<String> {
    let (installed, path) = crate::check_cli_installed();
    if installed { path } else { None }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Analyse the last assistant text and return structured outcome tags.
///
/// This function blocks for up to [`ANALYSIS_TIMEOUT`] and should be called
/// from a background thread.
pub fn analyze_session_outcome(last_text: &str, locale: &str) -> Option<AnalysisResult> {
    let claude_bin = match resolve_claude_path() {
        Some(p) => p,
        None => {
            log_debug("[claude_analyze] claude CLI not found on PATH or common locations");
            return None;
        }
    };

    let truncated: String = last_text.chars().take(MAX_INPUT_CHARS).collect();
    let prompt = build_prompt(&truncated, locale);

    let (tx, rx) = mpsc::channel();

    let prompt_clone = prompt.clone();
    std::thread::spawn(move || {
        let result = Command::new(&claude_bin)
            .args(["-p", &prompt_clone, "--model", "claude-haiku-4-5-20251001", "--no-session-persistence"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .output();
        let _ = tx.send(result);
    });

    let output = match rx.recv_timeout(ANALYSIS_TIMEOUT) {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            log_debug(&format!(
                "[claude_analyze] claude -p exited with status {}",
                output.status
            ));
            return None;
        }
        Ok(Err(e)) => {
            log_debug(&format!("[claude_analyze] failed to spawn claude: {e}"));
            return None;
        }
        Err(_) => {
            log_debug("[claude_analyze] timed out waiting for claude -p");
            return None;
        }
    };

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    log_debug(&format!(
        "[claude_analyze] raw response (len={}): {:?}",
        raw.len(),
        truncate_str(&raw, 200)
    ));

    if raw.is_empty() {
        log_debug("[claude_analyze] empty response, returning None");
        return None;
    }

    Some(parse_response(&raw))
}

/// Keep the old function signature as a thin wrapper for backward-compat
/// callers (if any).
pub fn analyze_waiting_input(last_text: &str, locale: &str) -> Option<String> {
    let result = analyze_session_outcome(last_text, locale)?;
    if result.tags.contains(&"needs_input".to_string()) {
        Some(result.summary.unwrap_or_else(|| "Waiting for input".to_string()))
    } else {
        None
    }
}

// ── Response parser ─────────────────────────────────────────────────────────

fn parse_response(raw: &str) -> AnalysisResult {
    // Expected formats:
    //   TAGS: needs_input,stuck | SUMMARY: Can't connect to DB, which driver?
    //   TAGS: bug_fixed,show_off
    //   bug_fixed,show_off          (fallback: no "TAGS:" prefix)

    let line = raw.lines().next().unwrap_or(raw).trim();

    // Strip optional "TAGS:" prefix
    let after_tags = line
        .strip_prefix("TAGS:")
        .or_else(|| line.strip_prefix("TAGS："))
        .map(|s| s.trim())
        .unwrap_or(line);

    // Split on " | SUMMARY:" to separate tags from summary
    let (tags_part, summary) = if let Some(idx) = after_tags.find("| SUMMARY:") {
        let t = after_tags[..idx].trim();
        let s = after_tags[idx + "| SUMMARY:".len()..].trim();
        (t, if s.is_empty() { None } else { Some(s.to_string()) })
    } else if let Some(idx) = after_tags.find("| SUMMARY：") {
        let t = after_tags[..idx].trim();
        let s = after_tags[idx + "| SUMMARY：".len()..].trim();
        (t, if s.is_empty() { None } else { Some(s.to_string()) })
    } else {
        (after_tags, None)
    };

    // Parse comma-separated tags, filtering to valid ones
    let tags: Vec<String> = tags_part
        .split(',')
        .map(|s| s.trim().to_lowercase().replace(' ', "_"))
        .filter(|t| VALID_TAGS.contains(&t.as_str()))
        .take(2)
        .collect();

    let tags = if tags.is_empty() {
        vec!["reporting".to_string()]
    } else {
        tags
    };

    log_debug(&format!(
        "[claude_analyze] parsed: tags={:?}, summary={:?}",
        tags, summary
    ));

    AnalysisResult { tags, summary }
}

// ── Mascot quip generation ─────────────────────────────────────────────────

const MAX_QUIPS: usize = 8;

fn build_quip_prompt(titles: &[String], mood: &str, locale: &str) -> String {
    let lang = match locale {
        "zh" => "用中文回复，口语化，用网络用语和梗，像程序员朋友在吐槽。",
        _ => "Reply in casual English, like a programmer friend roasting alongside you.",
    };

    let titles_text = titles
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. {}", i + 1, t))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are a mascot — the user's loyal little sidekick watching AI coding agents work. \
         You stand WITH the user, watching these agents together. \
         You're cheerful but snarky. You roast the code, the bugs, the agents, and the absurdity of the tasks — \
         like a coworker watching over someone's shoulder and making comments.\n\
         \n\
         Personality:\n\
         - Adore the user (老板/boss), never roast THEM\n\
         - Freely roast: the code quality, the bugs, the agents' mistakes, ridiculous requirements\n\
         - Mix: sarcastic commentary, encouragement, dramatic reactions, programmer humor\n\
         - Examples: \"又修bug，谁写的屎山\", \"改了又改，需求人呢\", \"秒了，这也太简单\", \
           \"测试全红，笑死\", \"这需求离谱到我想报警\"\n\
         \n\
         Current mood: {mood}\n\
         The AI agents are working on:\n\
         {titles_text}\n\
         \n\
         Generate {n} short quips (max 15 Chinese chars / 25 English chars). \
         Reference specific tasks. Be funny, snarky, dramatic. Vary the tone — \
         some supportive, some roasting, some shocked, some sarcastic.\n\
         \n\
         {lang}\n\
         \n\
         Output ONLY the quips, one per line. No numbers, bullets, or quotes.",
        mood = mood,
        titles_text = titles_text,
        n = MAX_QUIPS,
        lang = lang,
    )
}

/// Generate mascot quips based on recent session titles.
///
/// Returns up to [`MAX_QUIPS`] short personality lines.  Blocks for up to
/// [`ANALYSIS_TIMEOUT`] — call from a background thread.
pub fn generate_mascot_quips(titles: &[String], mood: &str, locale: &str) -> Vec<String> {
    let claude_bin = match resolve_claude_path() {
        Some(p) => p,
        None => {
            log_debug("[mascot_quips] claude CLI not found");
            return vec![];
        }
    };

    if titles.is_empty() {
        return vec![];
    }

    let prompt = build_quip_prompt(titles, mood, locale);

    let (tx, rx) = mpsc::channel();
    let prompt_clone = prompt.clone();
    std::thread::spawn(move || {
        let result = Command::new(&claude_bin)
            .args([
                "-p",
                &prompt_clone,
                "--model",
                "claude-haiku-4-5-20251001",
                "--no-session-persistence",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .output();
        let _ = tx.send(result);
    });

    let output = match rx.recv_timeout(ANALYSIS_TIMEOUT) {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            log_debug(&format!(
                "[mascot_quips] claude -p exited with status {}",
                output.status
            ));
            return vec![];
        }
        Ok(Err(e)) => {
            log_debug(&format!("[mascot_quips] failed to spawn claude: {e}"));
            return vec![];
        }
        Err(_) => {
            log_debug("[mascot_quips] timed out");
            return vec![];
        }
    };

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    log_debug(&format!(
        "[mascot_quips] raw response (len={}): {:?}",
        raw.len(),
        truncate_str(&raw, 300)
    ));

    raw.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l.len() <= 80)
        .take(MAX_QUIPS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_tags() {
        let r = parse_response("TAGS: bug_fixed, show_off");
        assert_eq!(r.tags, vec!["bug_fixed", "show_off"]);
        assert!(r.summary.is_none());
    }

    #[test]
    fn parse_needs_input_with_summary() {
        let r = parse_response("TAGS: needs_input, scheming | SUMMARY: Asking which database to use");
        assert_eq!(r.tags, vec!["needs_input", "scheming"]);
        assert_eq!(r.summary.as_deref(), Some("Asking which database to use"));
    }

    #[test]
    fn parse_no_prefix() {
        let r = parse_response("celebrating");
        assert_eq!(r.tags, vec!["celebrating"]);
    }

    #[test]
    fn parse_invalid_tags_fallback() {
        let r = parse_response("TAGS: foobar, baz");
        assert_eq!(r.tags, vec!["reporting"]);
    }

    #[test]
    fn parse_fullwidth_colon_tags() {
        let r = parse_response("TAGS：bug_fixed");
        assert_eq!(r.tags, vec!["bug_fixed"]);
    }

    #[test]
    fn parse_fullwidth_summary() {
        let r = parse_response("TAGS: needs_input | SUMMARY：需要选择数据库类型");
        assert_eq!(r.tags, vec!["needs_input"]);
        assert_eq!(r.summary.as_deref(), Some("需要选择数据库类型"));
    }

    #[test]
    fn parse_tags_capped_at_two() {
        let r = parse_response("TAGS: bug_fixed, show_off, celebrating");
        assert_eq!(r.tags.len(), 2);
        assert_eq!(r.tags, vec!["bug_fixed", "show_off"]);
    }

    #[test]
    fn parse_tags_normalizes_spaces_and_case() {
        let r = parse_response("TAGS: Bug Fixed, SHOW OFF");
        assert_eq!(r.tags, vec!["bug_fixed", "show_off"]);
    }

    #[test]
    fn parse_mixed_valid_invalid_tags() {
        let r = parse_response("TAGS: nonsense, stuck, invalid");
        assert_eq!(r.tags, vec!["stuck"]);
    }

    #[test]
    fn parse_empty_summary_treated_as_none() {
        let r = parse_response("TAGS: needs_input | SUMMARY: ");
        assert_eq!(r.tags, vec!["needs_input"]);
        assert!(r.summary.is_none());
    }

    #[test]
    fn parse_multiline_takes_first() {
        let r = parse_response("TAGS: celebrating\nsome extra text\nmore");
        assert_eq!(r.tags, vec!["celebrating"]);
    }

    // ── truncate_str tests ──────────────────────────────────────────────────

    #[test]
    fn truncate_within_limit() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_at_limit() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        // '你' is 3 bytes in UTF-8
        let s = "你好世界"; // 12 bytes
        let result = truncate_str(s, 7);
        // 7 bytes: can fit '你好' (6 bytes) but not '世' (would be 9)
        assert_eq!(result, "你好");
        assert_eq!(result.len(), 6);
    }

    #[test]
    fn truncate_zero() {
        assert_eq!(truncate_str("hello", 0), "");
    }

    // ── build_prompt tests ──────────────────────────────────────────────────

    #[test]
    fn build_prompt_english() {
        let p = build_prompt("some code output", "en");
        assert!(p.contains("Write the summary in English"));
        assert!(p.contains("some code output"));
        assert!(p.contains("needs_input"));
    }

    #[test]
    fn build_prompt_chinese() {
        let p = build_prompt("代码输出", "zh");
        assert!(p.contains("用中文回复"));
        assert!(p.contains("代码输出"));
    }

    #[test]
    fn build_prompt_japanese() {
        let p = build_prompt("output", "ja");
        assert!(p.contains("日本語で"));
    }

    // ── build_quip_prompt tests ─────────────────────────────────────────────

    #[test]
    fn build_quip_prompt_formats_titles() {
        let titles = vec!["Fix auth bug".into(), "Add dark mode".into()];
        let p = build_quip_prompt(&titles, "happy", "en");
        assert!(p.contains("1. Fix auth bug"));
        assert!(p.contains("2. Add dark mode"));
        assert!(p.contains("happy"));
    }

    #[test]
    fn build_quip_prompt_chinese_personality() {
        let titles = vec!["修复登录".into()];
        let p = build_quip_prompt(&titles, "neutral", "zh");
        assert!(p.contains("用中文回复"));
        assert!(p.contains("1. 修复登录"));
    }
}
