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
    /// Human-readable summary of what the agent just did or needs.
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
         TAGS: tag1[,tag2] | SUMMARY: <one sentence under 80 chars>\n\
         \n\
         The SUMMARY is ALWAYS required. Write it as if YOU are a loyal little fan reporting to your beloved boss (the user). \
         Address the user directly — use \"你\" (Chinese) or \"you\" (English), NEVER refer to the user in third person. \
         Tone: enthusiastic, slightly sycophantic, like an eager junior dev who adores their boss. \
         Be brief, direct, and focused on what was done or what is needed from the boss. \
         Describe what the assistant actually DID or what STATE it is in — do NOT let the tag choice influence the summary. \
         Read the text carefully: if the assistant says it already implemented something, say so. \
         Do NOT say \"asking\" or \"proposing\" when the work is already done.\n\
         Examples: \"Login bug squashed, tests all green!\", \"Boss, need you to pick a database\", \
         \"老板，登录bug搞定了，测试全过！\", \"老板，等你定一下用哪个数据库\"\n\
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
            .args(["-p", &prompt_clone, "--model", "haiku", "--no-session-persistence"])
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

const QUIPS_PER_GROUP: usize = 10;

fn build_quip_prompt(busy_titles: &[String], done_titles: &[String], locale: &str) -> String {
    let is_zh = locale.starts_with("zh");
    let lang = if is_zh {
        "用中文回复。每句不超过16个中文字。"
    } else {
        "Reply in English. Max 10 words per line."
    };

    let style_ref = if is_zh {
        "\
语感参考（仅供理解调性，禁止复用其中任何词句或意象）：\n\
- \"编译通过了我比谁都激动！\"\n\
- \"我在认真看代码虽然我看不懂代码\"\n\
- \"又报错了我看着都累\"\n\
\n\
❌ 不要写这种平淡的台词：\n\
- \"加油！\" / \"你真棒！\" / \"继续努力！\"\n\
\n\
✅ 要写有反转的——自我拆台、突然跑偏、先扬后抑。"
    } else {
        "\
Style reference (for tone only — do NOT reuse any words, phrases, or imagery from these):\n\
- \"Three agents compiling simultaneously? Peak civilization.\"\n\
- \"I'm helping by being extremely quiet right now.\"\n\
- \"That's the third failed build. I'm just watching.\"\n\
\n\
❌ DON'T write flat lines like:\n\
- \"Good job!\" / \"You can do it!\" / \"Keep going!\"\n\
\n\
✅ DO write lines with a twist — self-aware humor, absurd tangents, or a setup that undercuts itself."
    };

    let format_titles = |titles: &[String]| -> String {
        titles
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}. {}", i + 1, t))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let busy_text = if busy_titles.is_empty() {
        "(none)".to_string()
    } else {
        format_titles(busy_titles)
    };
    let done_text = if done_titles.is_empty() {
        "(none)".to_string()
    } else {
        format_titles(done_titles)
    };

    format!(
        "You are a tiny mascot living inside a developer's multi-agent dashboard (\"Claude Fleet\"). \
         You are rendered as a simple SVG — just two round eyes and a small mouth, no body, no limbs. \
         You sit in the corner watching AI coding agents work. You can't code, you can't move, you can only watch and comment.\n\
         \n\
         Your core contradiction: you desperately want to help but all you can do is watch and say short quips. \
         You overcompensate with enthusiasm, fill silence with weird thoughts, and deflect awkwardness with self-deprecation.\n\
         \n\
         {style_ref}\n\
         \n\
         Rules:\n\
         - No two lines may share the same joke structure or punchline pattern.\n\
         - Reference specific task titles when possible.\n\
         \n\
         Currently working on (busy):\n\
         {busy_text}\n\
         \n\
         Recently completed (done):\n\
         {done_text}\n\
         \n\
         Generate quips in TWO sections:\n\
         \n\
         BUSY (for when agents are working):\n\
         - Generate {n} quips about ongoing tasks\n\
         - Mix: snarky roasting, dramatic reactions, self-deprecating encouragement\n\
         \n\
         IDLE (for when things are calm):\n\
         - Generate {n} quips about downtime or completed work\n\
         - Mix: over-the-top celebration, playful boredom, weird mascot thoughts\n\
         \n\
         {lang}\n\
         \n\
         Output format — EXACTLY like this, no numbers, bullets, or quotes:\n\
         BUSY\n\
         quip1\n\
         quip2\n\
         ...\n\
         \n\
         IDLE\n\
         quip1\n\
         quip2\n\
         ...",
        style_ref = style_ref,
        busy_text = busy_text,
        done_text = done_text,
        n = QUIPS_PER_GROUP,
        lang = lang,
    )
}

/// Result of mascot quip generation — two groups of quips.
#[derive(serde::Serialize, Default)]
pub struct MascotQuips {
    pub busy: Vec<String>,
    pub idle: Vec<String>,
}

/// Generate mascot quips based on recent session titles.
///
/// Returns up to [`QUIPS_PER_GROUP`] quips for each of two groups (busy/idle).
/// Blocks for up to [`ANALYSIS_TIMEOUT`] — call from a background thread.
pub fn generate_mascot_quips(busy_titles: &[String], done_titles: &[String], locale: &str) -> MascotQuips {
    let claude_bin = match resolve_claude_path() {
        Some(p) => p,
        None => {
            log_debug("[mascot_quips] claude CLI not found");
            return MascotQuips::default();
        }
    };

    if busy_titles.is_empty() && done_titles.is_empty() {
        return MascotQuips::default();
    }

    let prompt = build_quip_prompt(busy_titles, done_titles, locale);

    let (tx, rx) = mpsc::channel();
    let prompt_clone = prompt.clone();
    std::thread::spawn(move || {
        let result = Command::new(&claude_bin)
            .args([
                "-p",
                &prompt_clone,
                "--model",
                "sonnet",
                "--no-session-persistence",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .output();
        let _ = tx.send(result);
    });

    // Sonnet is slower than Haiku — allow more time for quip generation
    let quip_timeout = Duration::from_secs(60);
    let output = match rx.recv_timeout(quip_timeout) {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            log_debug(&format!(
                "[mascot_quips] claude -p exited with status {}",
                output.status
            ));
            return MascotQuips::default();
        }
        Ok(Err(e)) => {
            log_debug(&format!("[mascot_quips] failed to spawn claude: {e}"));
            return MascotQuips::default();
        }
        Err(_) => {
            log_debug("[mascot_quips] timed out");
            return MascotQuips::default();
        }
    };

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    log_debug(&format!(
        "[mascot_quips] raw response (len={}): {:?}",
        raw.len(),
        truncate_str(&raw, 500)
    ));

    parse_quip_groups(&raw)
}

/// Parse the two-group output from the LLM into busy/idle quip vectors.
fn parse_quip_groups(raw: &str) -> MascotQuips {
    let mut busy = Vec::new();
    let mut idle = Vec::new();
    let mut current: Option<&str> = None; // "busy" or "idle"

    for line in raw.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();
        if upper == "BUSY" || upper.starts_with("BUSY:") {
            current = Some("busy");
            continue;
        }
        if upper == "IDLE" || upper.starts_with("IDLE:") {
            current = Some("idle");
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        // Skip lines that are too long
        if trimmed.len() > 80 {
            continue;
        }
        match current {
            Some("busy") if busy.len() < QUIPS_PER_GROUP => busy.push(trimmed.to_string()),
            Some("idle") if idle.len() < QUIPS_PER_GROUP => idle.push(trimmed.to_string()),
            _ => {}
        }
    }

    MascotQuips { busy, idle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_tags() {
        let r = parse_response("TAGS: bug_fixed, show_off");
        assert_eq!(r.tags, vec!["bug_fixed", "show_off"]);
        // No summary in this response — caller should use fallback
        assert!(r.summary.is_none());
    }

    #[test]
    fn parse_tags_with_summary() {
        let r = parse_response("TAGS: bug_fixed | SUMMARY: Fixed the login timeout issue");
        assert_eq!(r.tags, vec!["bug_fixed"]);
        assert_eq!(r.summary.as_deref(), Some("Fixed the login timeout issue"));
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
        let busy = vec!["Fix auth bug".into(), "Add dark mode".into()];
        let done = vec!["Refactor tests".into()];
        let p = build_quip_prompt(&busy, &done, "en");
        assert!(p.contains("1. Fix auth bug"));
        assert!(p.contains("2. Add dark mode"));
        assert!(p.contains("1. Refactor tests"));
        assert!(p.contains("BUSY"));
        assert!(p.contains("IDLE"));
    }

    #[test]
    fn build_quip_prompt_chinese_personality() {
        let busy = vec!["修复登录".into()];
        let done: Vec<String> = vec![];
        let p = build_quip_prompt(&busy, &done, "zh");
        assert!(p.contains("用中文回复"));
        assert!(p.contains("1. 修复登录"));
    }

    // ── parse_quip_groups tests ─────────────────────────────────────────────

    #[test]
    fn parse_quip_groups_basic() {
        let raw = "BUSY\nroast line 1\nroast line 2\n\nIDLE\npraise line 1\npraise line 2";
        let result = parse_quip_groups(raw);
        assert_eq!(result.busy, vec!["roast line 1", "roast line 2"]);
        assert_eq!(result.idle, vec!["praise line 1", "praise line 2"]);
    }

    #[test]
    fn parse_quip_groups_with_colon() {
        let raw = "BUSY:\nline1\n\nIDLE:\nline2";
        let result = parse_quip_groups(raw);
        assert_eq!(result.busy, vec!["line1"]);
        assert_eq!(result.idle, vec!["line2"]);
    }

    #[test]
    fn parse_quip_groups_caps_insensitive() {
        let raw = "busy\nA\n\nidle\nB";
        let result = parse_quip_groups(raw);
        assert_eq!(result.busy, vec!["A"]);
        assert_eq!(result.idle, vec!["B"]);
    }
}
