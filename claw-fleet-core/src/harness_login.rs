//! Claude Code login orchestration for the environment wizard.
//!
//! Drives `claude setup-token` under a pty (via the existing `proc_runner`
//! Backend surface) so a non-technical user never touches a terminal:
//!
//! 1. spawn `claude setup-token` (it opens the browser itself and prints the
//!    OAuth URL as a fallback),
//! 2. parse the auth URL out of the pty stream (OSC-8 hyperlink first, plain
//!    text after ANSI stripping as fallback) so the wizard can re-open it,
//! 3. the user authorizes in the browser and gets a one-time code; the wizard
//!    collects it in a form field and writes it to the pty,
//! 4. capture the long-lived token (`sk-ant-…`) from the output, persist it to
//!    `~/.fleet/harness-auth.json` (0600, atomic), and inject it as
//!    `CLAUDE_CODE_OAUTH_TOKEN` into spawned sessions whenever the CLI's own
//!    keychain credential is absent.
//!
//! This module owns the parsing + persistence; the pty lifecycle stays on the
//! Backend proc methods so the flow inherits their local/remote parity.
//!
//! Output shapes verified against a real `claude setup-token` pty capture
//! (claude 2.1.246, 2026-09-01); the parsers' fixtures are excerpts of it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the wizard needs to render the claude login card at each poll.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLoginParse {
    /// OAuth URL, once the CLI printed it (the CLI also opens the browser
    /// itself; the wizard shows this as the "browser didn't open?" fallback).
    pub auth_url: Option<String>,
    /// The CLI is waiting for the one-time code paste.
    pub awaiting_code: bool,
    /// Long-lived token captured from the output (already persisted by the
    /// caller when set).
    pub token: Option<String>,
}

/// Parse the cumulative pty output of `claude setup-token`.
pub fn parse_claude_login_output(raw: &str) -> ClaudeLoginParse {
    // Ink lays words out with cursor-column escapes instead of spaces
    // (`Paste\x1b[8Gcode\x1b[13Ghere`), so strip_ansi leaves a space per
    // stripped sequence and we whitespace-normalize before phrase checks.
    let normalized = normalize_ws(&strip_ansi(raw));
    ClaudeLoginParse {
        auth_url: parse_auth_url(raw),
        awaiting_code: normalized.contains("Paste code here"),
        token: parse_setup_token(raw),
    }
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The OAuth authorize URL. The Ink UI wraps it in an OSC-8 hyperlink
/// (`ESC ] 8 ; params ; URI ST`) whose URI parameter is contiguous and clean —
/// the *visible* text is chopped by cursor-positioning sequences — so read the
/// hyperlink first and only fall back to ANSI-stripped plain text.
fn parse_auth_url(raw: &str) -> Option<String> {
    // OSC-8 form: …]8;id=xyz;https://…\x1b\\ (or BEL-terminated).
    if let Some(start) = raw.find("]8;") {
        let after = &raw[start + 3..];
        if let Some(semi) = after.find(';') {
            let uri = &after[semi + 1..];
            let end = uri
                .find(|c: char| c == '\u{1b}' || c == '\u{7}')
                .unwrap_or(uri.len());
            let uri = &uri[..end];
            if uri.starts_with("https://") {
                return Some(uri.to_string());
            }
        }
    }
    // Fallback: strip CSI sequences, then take the first https token.
    let stripped = strip_ansi(raw);
    stripped
        .split_whitespace()
        .find(|tok| tok.starts_with("https://") && tok.contains("oauth"))
        .map(|s| s.trim_end_matches(&[')', ']', '.', ','][..]).to_string())
}

/// The long-lived token: `sk-ant-` followed by its base62/dash body. Length
/// floor keeps ANSI-interleaved prefixes from matching.
fn parse_setup_token(raw: &str) -> Option<String> {
    let stripped = strip_ansi(raw);
    let start = stripped.find("sk-ant-")?;
    let token: String = stripped[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (token.len() >= 24).then_some(token)
}

/// Remove CSI (`ESC [ … cmd`) and OSC (`ESC ] … ST/BEL`) sequences. Each
/// stripped CSI leaves one space, because Ink positions words with
/// cursor-column sequences instead of literal spaces; callers that need exact
/// phrases whitespace-normalize afterwards.
fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameters then one final byte in @-~.
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
                out.push(' ');
            }
            Some(']') => {
                chars.next();
                // OSC: runs to BEL or ESC \.
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next(); // consume the '\'
                        break;
                    }
                }
            }
            _ => {
                chars.next(); // two-char escape (ESC 7, ESC 8, ESC \, …)
            }
        }
    }
    out
}

// ── Codex login parsing ───────────────────────────────────────────────────────
//
// `codex login` needs no code paste-back: it runs a localhost callback server
// (port 1455, fallback 1457 — a server-side allow-list, not configurable) and
// finishes entirely in the browser. The CLI opens the browser itself and
// prints to stderr: "Starting local login server on http://localhost:{port}.
// If your browser did not open, navigate to this URL to authenticate:\n\n{url}"
// then "Successfully logged in" / exits 1 on failure. (Shape read from
// codex-rs cli/src/login.rs + login/src/server.rs; re-verified on-device in
// the wizard e2e.) `codex login --device-auth` is the no-browser fallback.

/// What the wizard's codex-login card renders on each poll tick.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexLoginParse {
    /// The authorize URL (not the localhost line) once printed.
    pub auth_url: Option<String>,
    pub success: bool,
    /// Both allowed callback ports are taken by something else.
    pub port_busy: bool,
}

/// Parse the cumulative output of `codex login` (pty or pipes).
pub fn parse_codex_login_output(raw: &str) -> CodexLoginParse {
    let stripped = strip_ansi(raw);
    let auth_url = stripped
        .split_whitespace()
        // Exclude only a literal localhost *host* (the "server started" line);
        // the real authorize URL legitimately embeds "localhost" inside its
        // percent-encoded redirect_uri parameter.
        .find(|tok| tok.starts_with("https://") && !tok.starts_with("https://localhost"))
        .map(|s| s.trim_end_matches(&[')', ']', '.', ','][..]).to_string());
    let normalized = normalize_ws(&stripped);
    CodexLoginParse {
        auth_url,
        success: normalized.contains("Successfully logged in"),
        port_busy: normalized.to_lowercase().contains("address already in use"),
    }
}

// ── Token persistence ─────────────────────────────────────────────────────────

/// Fleet-held harness auth material: the claude long-lived OAuth token from
/// the wizard's `setup-token` flow. Injected into spawned sessions as
/// `CLAUDE_CODE_OAUTH_TOKEN` only when the CLI's own keychain credential is
/// absent, so a later proper `/login` always wins.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct HarnessAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_oauth_token: Option<String>,
}

fn auth_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("harness-auth.json"))
}

fn load_auth() -> HarnessAuth {
    let Some(path) = auth_path() else { return HarnessAuth::default() };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_auth(auth: &HarnessAuth) -> Result<(), String> {
    let path = auth_path().ok_or("cannot resolve home directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(auth).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Persist the captured token (0600, atomic write like every ~/.fleet JSON).
pub fn save_claude_oauth_token(token: &str) -> Result<(), String> {
    let mut auth = load_auth();
    auth.claude_oauth_token = Some(token.to_string());
    write_auth(&auth)
}

pub fn clear_claude_oauth_token() -> Result<(), String> {
    let mut auth = load_auth();
    auth.claude_oauth_token = None;
    write_auth(&auth)
}

/// The stored token, if any.
pub fn stored_claude_token() -> Option<String> {
    load_auth().claude_oauth_token.filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed excerpt of the real `claude setup-token` pty capture
    /// (claude 2.1.246): OSC-8 hyperlink whose visible text is chopped by
    /// cursor jumps, then the paste prompt.
    const REAL_CAPTURE: &str = concat!(
        "\u{1b}[2G·\u{1b}[4GOpening\u{1b}[12Gbrowser\u{1b}[20Gto\u{1b}[23Gsign\u{1b}[28Gin…\n",
        "\u{1b}]8;id=1q57lwy;https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&code_challenge=w3Gx&code_challenge_method=S256&state=O_4j\u{1b}\\https://claude.com/cai/oauth/…\u{1b}]8;;\u{1b}\\\n",
        "\u{1b}[2GPaste\u{1b}[8Gcode\u{1b}[13Ghere\u{1b}[18Gif\u{1b}[21Gprompted\u{1b}[30G>",
    );

    #[test]
    fn parses_auth_url_from_osc8_hyperlink() {
        let p = parse_claude_login_output(REAL_CAPTURE);
        let url = p.auth_url.expect("url");
        assert!(url.starts_with("https://claude.com/cai/oauth/authorize?code=true"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(!url.contains('\u{1b}'), "url must not carry escapes: {url}");
        assert!(p.awaiting_code);
        assert_eq!(p.token, None);
    }

    #[test]
    fn parses_auth_url_from_plain_text_when_no_hyperlink() {
        let raw = "\u{1b}[1mSign in:\u{1b}[0m https://claude.com/cai/oauth/authorize?code=true&state=x\n";
        let p = parse_claude_login_output(raw);
        assert_eq!(
            p.auth_url.as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&state=x")
        );
        assert!(!p.awaiting_code);
    }

    #[test]
    fn parses_token_and_ignores_short_prefixes() {
        let raw = "done!\nYour token: \u{1b}[1msk-ant-oat01-AbCd1234efGH5678ijKL9012mnOP\u{1b}[0m\nkeep it safe";
        let p = parse_claude_login_output(raw);
        assert_eq!(
            p.token.as_deref(),
            Some("sk-ant-oat01-AbCd1234efGH5678ijKL9012mnOP")
        );
        // A bare mention like "sk-ant-" alone must not count.
        assert_eq!(parse_setup_token("prefix sk-ant- only"), None);
    }

    #[test]
    fn strip_ansi_removes_csi_and_osc() {
        let s = strip_ansi(REAL_CAPTURE);
        assert!(!s.contains('\u{1b}'));
        // Ink spaces words via cursor-column escapes; the phrase reads back
        // after whitespace normalization.
        assert!(normalize_ws(&s).contains("Paste code here"));
    }

    #[test]
    fn parses_codex_login_stream() {
        // Stderr shape from codex-rs cli/src/login.rs::print_login_server_start.
        let raw = "Starting local login server on http://localhost:1455.\n\
                   If your browser did not open, navigate to this URL to authenticate:\n\n\
                   https://auth.openai.com/oauth/authorize?client_id=abc&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=xyz\n";
        let p = parse_codex_login_output(raw);
        assert_eq!(
            p.auth_url.as_deref(),
            Some("https://auth.openai.com/oauth/authorize?client_id=abc&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=xyz")
        );
        assert!(!p.success && !p.port_busy);

        let done = format!("{raw}Successfully logged in\n");
        assert!(parse_codex_login_output(&done).success);

        let busy = "error binding local login server: Address already in use (os error 48)";
        assert!(parse_codex_login_output(busy).port_busy);
        // The localhost line alone must not be mistaken for the auth URL.
        assert_eq!(
            parse_codex_login_output("Starting local login server on http://localhost:1455.").auth_url,
            None
        );
    }

    #[test]
    fn token_store_roundtrip_under_isolated_home() {
        // real_home_dir honors FLEET_HOME; env mutation is process-global, so
        // hold the repo's global fleet-home lock and restore the prior value
        // (same sandbox pattern as browse_paths' tests).
        let _guard = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp.path()) };

        assert_eq!(stored_claude_token(), None);
        save_claude_oauth_token("sk-ant-oat01-testtoken1234567890").unwrap();
        assert_eq!(
            stored_claude_token().as_deref(),
            Some("sk-ant-oat01-testtoken1234567890")
        );
        clear_claude_oauth_token().unwrap();
        assert_eq!(stored_claude_token(), None);

        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }
}
