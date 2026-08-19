//! The dsh agent preset that makes a chat-workspace session a chat.
//!
//! [`crate::chat_workspace`] keeps a Claude session out of the user's 22k-token
//! engineering doctrine with `--setting-sources project`. dsh has no such flag:
//! `@deepseek-ai/dsh-agent-instructions` loads `$DSH_HOME/AGENTS.md` — where
//! [`crate::dsh_guidance`] writes Fleet's PRD / worktree / interaction blocks —
//! unconditionally, so a "hi" in the chat workspace arrived carrying 19,397
//! characters of coding-agent discipline that the chat brief then had to cancel
//! line by line (measured on a real capture).
//!
//! ## Why a preset is the only lever (verified against the shipped wire schema)
//!
//! `session.create`'s payload is `{workspaceId?, cwd?, sessionId?, agentPreset?}`
//! (`sessionCreateRequestSchema` in `dsh-host-apiproxy`). There is no
//! per-session instruction or plugin-config field, and `dshHome` is *plugin*
//! config — deployment-wide. `agentPreset` is the one seam: a preset is a
//! directory holding one `agent.cordis.yml`, user-authored ones live under
//! `${DSH_HOME:-~/.dsh}/.agent-presets/<id>/`, and preset discovery is
//! unmemoized (the roster re-reads its roots on every call), so a directory
//! Fleet writes is visible to the next `session.create` without restarting
//! `dsh web`.
//!
//! So Fleet authors one preset, `fleet-chat`, that is the deployment's own
//! default composition with a single row patched: the `agent-instructions`
//! plugin's `dshHome` points at an empty Fleet-owned directory. Per that
//! plugin's `Config`, `dshHome` is used for exactly one thing —
//! `join(dshHome, "AGENTS.md")`, the user-global file, "always
//! `$DSH_HOME/AGENTS.md` with no local overlay". Pointing it at a directory
//! with no `AGENTS.md` drops the user-global chain and leaves every project
//! candidate (the chat workspace's own `CLAUDE.md`) loading normally. Nothing
//! else reads this row's `dshHome`, so skills and credentials keep the real
//! `$DSH_HOME`.
//!
//! ## Why it is re-derived on every spawn, and patched textually
//!
//! Copying a composition freezes it: a dsh upgrade that adds a plugin row to
//! `standard` would never reach a frozen copy, and the chat sessions would
//! quietly run last release's agent. So the source text is re-read from the
//! deployment (`agentPreset.read` on whichever preset reports `isDefault`) and
//! re-patched every time, which makes upgrades free.
//!
//! The patch is textual rather than a YAML round-trip because the shipped
//! composition uses the loader's own dialect — the `standard` composition
//! carries two `!!js` tags — which a plain YAML parser mangles or rejects.
//! A textual insert also keeps the comments a reader of the generated file
//! needs. It is anchored to the exact row and fails loudly when the shape is
//! not found: shipping an unpatched preset would silently reintroduce the very
//! doctrine this module exists to remove.

use std::fs;
use std::path::PathBuf;

/// Id (and directory name) of the Fleet-authored chat preset. Must match
/// dsh's `[a-z0-9][a-z0-9-]*` preset-id rule.
pub const CHAT_PRESET_ID: &str = "fleet-chat";

/// The plugin row whose `dshHome` is redirected.
const INSTRUCTIONS_ROW_ID: &str = "agent-instructions";

/// Directory under `~/.fleet/` that stands in for `$DSH_HOME` for chat
/// sessions. Deliberately empty — its whole job is to *not* contain an
/// `AGENTS.md`. Created rather than left missing so the plugin's stat of
/// `<dir>/AGENTS.md` is a clean "absent" rather than a resolve error, which it
/// would classify as "temporarily unavailable" instead.
const CHAT_DSH_HOME: &str = "dsh-chat-home";

/// Where dsh looks for user-authored presets, relative to `$DSH_HOME`.
const USER_PRESET_DIR: &str = ".agent-presets";

/// The one file a preset directory must hold.
const COMPOSITION_FILE: &str = "agent.cordis.yml";

/// Whether a dsh session in `workspace_path` should run the chat preset.
///
/// Keyed off the same predicate the Claude path uses, so the two harnesses
/// cannot disagree about what "the chat workspace" is.
pub fn wants_chat_preset(workspace_path: &str) -> bool {
    crate::chat_workspace::is_chat_workspace(workspace_path)
}

/// Absolute path of the empty stand-in `$DSH_HOME`, created if absent.
fn ensure_chat_dsh_home() -> Result<PathBuf, String> {
    let dir = crate::session::get_fleet_dir()
        .ok_or_else(|| "no fleet dir".to_string())?
        .join(CHAT_DSH_HOME);
    fs::create_dir_all(&dir).map_err(|e| format!("create chat dsh home: {e}"))?;
    // A stray AGENTS.md here would silently reinstate the doctrine, so say so
    // rather than shipping a preset that quietly does nothing.
    if dir.join("AGENTS.md").exists() {
        return Err(format!(
            "{} must stay empty: an AGENTS.md there would reinstate the doctrine \
             the chat preset exists to drop",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Quote a path as a YAML single-quoted scalar (the only escape inside one is a
/// doubled quote), so a home directory with a space or a `#` cannot change how
/// the composition parses.
fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The line range of the `- id: <row_id>` row in a composition, as
/// `(start, end)` line indices where `end` is exclusive.
///
/// A row runs from its `- ` marker to the next top-level `- ` marker (or EOF).
/// Comment and blank lines between rows belong to the row above; that only
/// affects where an insert lands relative to a trailing comment, never which
/// row is patched.
fn row_span(lines: &[&str], row_id: &str) -> Option<(usize, usize)> {
    let marker = format!("- id: {row_id}");
    let start = lines.iter().position(|l| l.trim_end() == marker)?;
    let end = lines
        .iter()
        .skip(start + 1)
        .position(|l| l.starts_with("- "))
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    Some((start, end))
}

/// Patch the `agent-instructions` row's `dshHome` in a preset composition.
///
/// Adds the key when the row already has a `config:` block, creates the block
/// when it does not, and replaces an existing `dshHome` rather than emitting a
/// duplicate key (which YAML would resolve last-wins, but a reader would not).
/// Errors when the row is absent — see the module header for why that must not
/// degrade silently.
pub fn patch_composition(composition: &str, dsh_home: &str) -> Result<String, String> {
    let lines: Vec<&str> = composition.lines().collect();
    let (start, end) = row_span(&lines, INSTRUCTIONS_ROW_ID).ok_or_else(|| {
        format!("dsh chat preset: composition has no `- id: {INSTRUCTIONS_ROW_ID}` row")
    })?;
    let entry = format!("    dshHome: {}", yaml_quote(dsh_home));

    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    // An existing key is replaced in place, keeping the row's shape stable
    // across repeated regenerations.
    if let Some(i) = (start..end).find(|&i| lines[i].trim_start().starts_with("dshHome:")) {
        out[i] = entry;
    } else if let Some(i) = (start..end).find(|&i| lines[i].trim_end() == "  config:") {
        out.insert(i + 1, entry);
    } else {
        // No config block on this row: open one right after the row's own keys.
        let last_key = (start..end)
            .rev()
            .find(|&i| lines[i].starts_with("  ") && lines[i].contains(':'))
            .unwrap_or(start);
        out.insert(last_key + 1, "  config:".to_string());
        out.insert(last_key + 2, entry);
    }

    let mut text = out.join("\n");
    if composition.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// Absolute path of the chat preset's composition file.
fn composition_path() -> Result<PathBuf, String> {
    let home = crate::session::get_dsh_dir().ok_or_else(|| "no dsh home".to_string())?;
    Ok(home
        .join(USER_PRESET_DIR)
        .join(CHAT_PRESET_ID)
        .join(COMPOSITION_FILE))
}

/// Write (or refresh) the chat preset from `source_composition` — the text of
/// the deployment's own default preset — and return its id.
///
/// Idempotent: an unchanged composition is not rewritten, so a chat spawn does
/// not touch a file the roster may be reading.
pub fn ensure_chat_preset(source_composition: &str) -> Result<String, String> {
    let dsh_home = ensure_chat_dsh_home()?;
    let patched = patch_composition(source_composition, &dsh_home.to_string_lossy())?;
    let path = composition_path()?;
    let dir = path
        .parent()
        .ok_or_else(|| "chat preset path has no parent".to_string())?;
    fs::create_dir_all(dir).map_err(|e| format!("create chat preset dir: {e}"))?;
    let stale = fs::read_to_string(&path).map(|c| c != patched).unwrap_or(true);
    if stale {
        // Atomic because the roster re-reads its roots on every call: a chat
        // spawn racing a concurrent `agentPreset.list` must never expose a
        // half-written composition, which discovery would report as broken.
        crate::atomic_json::write_atomic(&path, patched.as_bytes())
            .map_err(|e| format!("write chat preset: {e}"))?;
    }
    Ok(CHAT_PRESET_ID.to_string())
}

/// The id of the preset a session would otherwise be created under, read from
/// an `agentPreset.list` answer.
///
/// The deployment default is what a chat session must inherit — hardcoding
/// `standard` would silently change composition on a deployment whose default
/// is `code`. A broken default is refused rather than copied: mounting it would
/// fail anyway, and a chat session is better off on the unpatched default than
/// on a preset that cannot compose.
pub fn default_preset_id(list: &serde_json::Value) -> Option<String> {
    list.get("presets")?
        .as_array()?
        .iter()
        .find(|p| {
            p.get("isDefault").and_then(serde_json::Value::as_bool) == Some(true)
                && p.get("broken").is_none()
                // Fleet's own preset must never become its own source.
                && p.get("id").and_then(serde_json::Value::as_str) != Some(CHAT_PRESET_ID)
        })
        .and_then(|p| p.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verbatim shape of the shipped `standard` composition's row, including the
    /// `config:` block it already carries.
    const STANDARD_EXCERPT: &str = "\
- id: persona
  name: '@deepseek-ai/dsh-persona'
  config:
    text: >-
      You are a coding agent.

- id: agent-instructions
  name: '@deepseek-ai/dsh-agent-instructions'
  config:
    maxBytes: 65536

- id: tool-bash
  name: '@deepseek-ai/dsh-bash-local'
";

    #[test]
    fn the_dsh_home_key_lands_inside_the_instructions_row_config() {
        let out = patch_composition(STANDARD_EXCERPT, "/tmp/empty").unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let row = lines
            .iter()
            .position(|l| *l == "- id: agent-instructions")
            .unwrap();
        assert_eq!(lines[row + 2], "  config:");
        assert_eq!(lines[row + 3], "    dshHome: '/tmp/empty'");
        // The row's own key survives, and the neighbours are untouched.
        assert_eq!(lines[row + 4], "    maxBytes: 65536");
        assert!(out.contains("- id: persona"));
        assert!(out.contains("- id: tool-bash"));
        assert!(out.contains("You are a coding agent."));
    }

    /// The persona row above also has a `config:`; the patch must not land in it.
    #[test]
    fn an_earlier_rows_config_block_is_not_patched() {
        let out = patch_composition(STANDARD_EXCERPT, "/tmp/empty").unwrap();
        assert_eq!(out.matches("dshHome:").count(), 1);
        let dsh_home_at = out.find("dshHome:").unwrap();
        let row_at = out.find("- id: agent-instructions").unwrap();
        assert!(dsh_home_at > row_at, "dshHome must sit inside the instructions row");
    }

    #[test]
    fn a_row_without_a_config_block_gets_one() {
        let source = "\
- id: agent-instructions
  name: '@deepseek-ai/dsh-agent-instructions'

- id: tool-bash
  name: '@deepseek-ai/dsh-bash-local'
";
        let out = patch_composition(source, "/tmp/empty").unwrap();
        assert!(
            out.contains("  name: '@deepseek-ai/dsh-agent-instructions'\n  config:\n    dshHome: '/tmp/empty'"),
            "{out}"
        );
        assert!(out.contains("- id: tool-bash"), "later rows survive");
    }

    /// Regeneration must not stack duplicate keys — the file is rewritten from
    /// the live source on every chat spawn.
    #[test]
    fn patching_twice_replaces_rather_than_duplicates() {
        let once = patch_composition(STANDARD_EXCERPT, "/tmp/one").unwrap();
        let twice = patch_composition(&once, "/tmp/two").unwrap();
        assert_eq!(twice.matches("dshHome:").count(), 1);
        assert!(twice.contains("    dshHome: '/tmp/two'"));
        assert!(!twice.contains("/tmp/one"));
    }

    /// The loader's own dialect allows `!!js` expressions, which a YAML
    /// round-trip would mangle. A textual patch must leave them byte-identical.
    #[test]
    fn js_tags_and_comments_survive_untouched() {
        let source = "\
# a comment the reader needs
- id: agent-instructions
  name: '@deepseek-ai/dsh-agent-instructions'
  config:
    maxBytes: 65536

- id: planning
  when: !!js |
    (ctx) => ctx.enabled
";
        let out = patch_composition(source, "/tmp/empty").unwrap();
        assert!(out.contains("# a comment the reader needs"));
        assert!(out.contains("  when: !!js |"));
        assert!(out.contains("    (ctx) => ctx.enabled"));
    }

    /// A path with a quote or a space must not change how the file parses.
    #[test]
    fn a_path_needing_quoting_is_quoted() {
        let out = patch_composition(STANDARD_EXCERPT, "/tmp/it's here").unwrap();
        assert!(out.contains("    dshHome: '/tmp/it''s here'"), "{out}");
    }

    /// Shipping an unpatched preset would silently reinstate the doctrine, so a
    /// composition Fleet does not recognise is an error, not a pass-through.
    #[test]
    fn a_composition_without_the_row_is_refused() {
        let err = patch_composition("- id: persona\n  name: x\n", "/tmp/empty").unwrap_err();
        assert!(err.contains("agent-instructions"), "{err}");
    }

    #[test]
    fn the_default_preset_is_read_from_the_list_answer() {
        let list = json!({
            "presets": [
                { "id": "minimal", "trust": "system", "isDefault": false },
                { "id": "standard", "trust": "system", "isDefault": true },
                { "id": "code", "trust": "system", "isDefault": false },
            ],
            "authorable": true,
            "hasDocument": true,
        });
        assert_eq!(default_preset_id(&list).as_deref(), Some("standard"));
    }

    /// A broken default cannot compose a session, so it is not a usable source.
    #[test]
    fn a_broken_default_is_not_used_as_the_source() {
        let list = json!({
            "presets": [
                { "id": "standard", "trust": "system", "isDefault": true, "broken": "unparsable YAML" },
            ],
        });
        assert_eq!(default_preset_id(&list), None);
    }

    /// If Fleet's own preset ever became the deployment default, copying it into
    /// itself would compound the patch instead of tracking the real default.
    #[test]
    fn fleets_own_preset_is_never_its_own_source() {
        let list = json!({
            "presets": [{ "id": CHAT_PRESET_ID, "trust": "user", "isDefault": true }],
        });
        assert_eq!(default_preset_id(&list), None);
    }

    #[test]
    fn a_list_answer_without_a_default_yields_none() {
        let list = json!({ "presets": [{ "id": "standard", "isDefault": false }] });
        assert_eq!(default_preset_id(&list), None);
    }

    /// Only the chat workspace gets the preset; an ordinary project must keep
    /// the deployment default, doctrine included.
    #[test]
    fn only_the_chat_workspace_wants_the_preset() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::session::fleet_home_lock();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp.path()) };
        let chat = tmp.path().join(".fleet/chat");
        assert!(wants_chat_preset(&chat.to_string_lossy()));
        assert!(!wants_chat_preset("/Users/foo/my-project"));
        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }
    }
}
