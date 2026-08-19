//! Install and uninstall Fleet's cordis plugin for dsh.
//!
//! The plugin source lives in this repo at `dsh-plugin/` and is compiled into
//! the binary with [`include_str!`], so a Fleet build carries the exact plugin
//! it expects — no npm registry, no network, no version skew between the two
//! halves of the contract (`dsh-plugin/index.js` ⇄ `fleet dsh-context`).
//!
//! Installing means two things:
//!
//! 1. **Materialize** the plugin into `~/.fleet/dsh-plugin/`. Verified live: a
//!    loader entry's `name` may be an absolute file path — `pluginInventory/list`
//!    reported `{"moduleName":"<abs path>.js","fiberPhase":"active"}` — so the
//!    plugin does **not** need `pnpm add`-ing into
//!    `$DSH_HOME/profiles/*/package.json`. `package.json` ships alongside
//!    `index.js` only to pin `"type": "module"`, since Node decides a bare `.js`
//!    file's module kind from the nearest `package.json`.
//! 2. **Register** it in `$DSH_HOME/cordis.patch.yml` inside a Fleet sentinel
//!    block. That file is the *home-level* user patch layer, which dsh applies
//!    after every bundle layer and which outranks the per-profile file — so one
//!    file covers both the `web` and `headless` profiles.
//!
//! # Why sentinels instead of parsing the YAML
//!
//! Same reason [`crate::dsh_guidance`] uses them for `AGENTS.md`: the file is
//! the user's, Fleet owns one region of it, and a round-trip through a YAML
//! parser would reformat and reorder everything outside that region. dsh keeps
//! this file live through its HMR watcher, so a rewritten block takes effect
//! without restarting `dsh web`.
//!
//! # The `[]` placeholder is load-bearing
//!
//! dsh's own docs: "An empty or comments-only file throws (it parses to nothing,
//! not to a list); disable the layer with `[]`." So uninstalling cannot leave an
//! empty file behind — with no user entries left, this writes `[]`. For the same
//! reason installing must *remove* a lone `[]` before appending list items: `[]`
//! followed by `- insert:` is not valid YAML.

use std::fs;
use std::path::PathBuf;

/// The plugin module, compiled in from `dsh-plugin/index.js`.
const PLUGIN_JS: &str = include_str!("../../dsh-plugin/index.js");

/// Its manifest, compiled in from `dsh-plugin/package.json`. Present on disk
/// only so Node reads `"type": "module"` for `index.js`.
const PLUGIN_PACKAGE_JSON: &str = include_str!("../../dsh-plugin/package.json");

/// Loader entry id Fleet owns inside `cordis.patch.yml`.
const ENTRY_ID: &str = "fleet-context";

const SENTINEL_BEGIN: &str = "# fleet:dsh-plugin:begin";
const SENTINEL_END: &str = "# fleet:dsh-plugin:end";

/// Where the materialized plugin lives.
pub fn plugin_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("dsh-plugin"))
}

/// The plugin entrypoint a loader entry's `name` points at.
pub fn plugin_entry_path() -> Option<PathBuf> {
    plugin_dir().map(|d| d.join("index.js"))
}

/// dsh's home-level user patch layer.
fn patch_path() -> Option<PathBuf> {
    crate::session::get_dsh_dir().map(|d| d.join("cordis.patch.yml"))
}

/// Write the compiled-in plugin to `~/.fleet/dsh-plugin/`, skipping files whose
/// content already matches so a reinstall does not churn mtimes (dsh's HMR
/// watcher is watching the patch file, but there is no reason to touch these).
pub fn materialize() -> Result<PathBuf, String> {
    let dir = plugin_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    for (name, body) in [
        ("index.js", PLUGIN_JS),
        ("package.json", PLUGIN_PACKAGE_JSON),
    ] {
        let path = dir.join(name);
        if fs::read_to_string(&path).ok().as_deref() == Some(body) {
            continue;
        }
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(dir.join("index.js"))
}

/// Render the Fleet-owned block: one patch entry inserting the plugin.
///
/// `fleet_bin` is passed through as the plugin's `fleetBin` config so the plugin
/// does not depend on `fleet` being on the PATH of whatever shell launched
/// `dsh web` (a GUI-launched app inherits a minimal PATH).
///
/// `user_title` and `locale` are frozen into the config for the same reason: the
/// plugin runs inside dsh with no access to Fleet's settings, and `fleet
/// dsh-context` defaults to `Boss` / `en` when nobody tells it otherwise — which
/// would silently render the guidance in English and address the user as "Boss".
/// The desktop already holds the real values when it reconciles, so they travel
/// with the entry and refresh on every reconcile.
fn render_block(
    entry_path: &std::path::Path,
    fleet_bin: Option<&std::path::Path>,
    user_title: &str,
    locale: &str,
) -> String {
    let mut out = String::new();
    out.push_str(SENTINEL_BEGIN);
    out.push_str("\n# Managed by Claw Fleet — do not edit. Injects Fleet's per-turn context.\n");
    out.push_str("- insert:\n");
    out.push_str(&format!("    - id: {ENTRY_ID}\n"));
    out.push_str(&format!("      name: {}\n", yaml_scalar(&entry_path.to_string_lossy())));
    out.push_str("      config:\n");
    if let Some(bin) = fleet_bin {
        out.push_str(&format!(
            "        fleetBin: {}\n",
            yaml_scalar(&bin.to_string_lossy())
        ));
    }
    out.push_str(&format!("        userTitle: {}\n", yaml_scalar(user_title)));
    out.push_str(&format!("        locale: {}\n", yaml_scalar(locale)));
    out.push_str(SENTINEL_END);
    out.push('\n');
    out
}

/// Quote a scalar so a path containing YAML-significant characters (`:`, `#`, a
/// leading `%`, …) survives. Double quotes with the two escapes YAML requires.
fn yaml_scalar(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Strip the Fleet block, returning the user's own content.
///
/// Tolerates a missing end marker (a half-written file from a crash): everything
/// from `begin` on is dropped in that case, since Fleet wrote all of it.
fn strip_block(text: &str) -> String {
    let Some(start) = text.find(SENTINEL_BEGIN) else {
        return text.to_string();
    };
    let rest = match text[start..].find(SENTINEL_END) {
        Some(offset) => {
            let end = start + offset + SENTINEL_END.len();
            // Consume the newline that terminated the end marker's line.
            text[end..].strip_prefix('\n').unwrap_or(&text[end..])
        }
        None => "",
    };
    format!("{}{}", &text[..start], rest)
}

/// Whether the remaining text carries any actual list entry, as opposed to
/// comments, whitespace, or dsh's `[]` placeholder.
fn has_user_entries(text: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        !t.is_empty() && !t.starts_with('#') && t != "[]"
    })
}

/// The single writer for `$DSH_HOME/cordis.patch.yml`.
///
/// `enabled` installs (materializing the plugin first) or removes the Fleet
/// block. Idempotent in both directions, and preserves user-authored entries
/// outside the sentinels.
pub fn reconcile_dsh_patch(enabled: bool, user_title: &str, locale: &str) -> Result<(), String> {
    let path = patch_path().ok_or("cannot determine dsh home")?;
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let user = strip_block(&existing);

    let next = if enabled {
        let entry = materialize()?;
        let fleet_bin = crate::fleet_cli::resolve_fleet_binary();
        let block = render_block(&entry, fleet_bin.as_deref(), user_title, locale);
        if has_user_entries(&user) {
            format!("{}\n{block}", user.trim_end())
        } else {
            // Drop a lone `[]` (or a comments-only file): `[]` cannot precede
            // list items, and an empty document throws on load.
            let comments: String = user
                .lines()
                .filter(|l| l.trim().starts_with('#'))
                .map(|l| format!("{l}\n"))
                .collect();
            format!("{comments}{block}")
        }
    } else if has_user_entries(&user) {
        format!("{}\n", user.trim_end())
    } else {
        // No entries left. An empty file throws on load; `[]` is dsh's own way
        // to spell "this layer is disabled".
        let comments: String = user
            .lines()
            .filter(|l| l.trim().starts_with('#'))
            .map(|l| format!("{l}\n"))
            .collect();
        format!("{comments}[]\n")
    };

    if fs::read_to_string(&path).ok().as_deref() == Some(next.as_str()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(&path, next).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Whether Fleet's plugin block is registered in `$DSH_HOME/cordis.patch.yml`.
pub fn is_dsh_plugin_installed() -> bool {
    patch_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .is_some_and(|t| t.contains(SENTINEL_BEGIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_names_the_entry_the_plugin_path_and_the_fleet_binary() {
        let block = render_block(
            std::path::Path::new("/home/u/.fleet/dsh-plugin/index.js"),
            Some(std::path::Path::new("/usr/local/bin/fleet")),
            "老板",
            "zh",
        );
        assert!(block.starts_with(SENTINEL_BEGIN));
        assert!(block.trim_end().ends_with(SENTINEL_END));
        assert!(block.contains("- insert:"));
        assert!(block.contains(&format!("- id: {ENTRY_ID}")));
        assert!(block.contains("name: \"/home/u/.fleet/dsh-plugin/index.js\""));
        assert!(block.contains("fleetBin: \"/usr/local/bin/fleet\""));
    }

    /// No `fleet` on disk drops only that key — the plugin then falls back to
    /// its own `'fleet'` default and looks the binary up on PATH. `userTitle` and
    /// `locale` must survive regardless, since their CLI defaults (`Boss` / `en`)
    /// are wrong for anyone who configured otherwise.
    #[test]
    fn block_omits_only_the_fleet_binary_when_none_was_found() {
        let block = render_block(std::path::Path::new("/p/index.js"), None, "老板", "zh");
        assert!(!block.contains("fleetBin"));
        assert!(block.contains("config:"));
        assert!(block.contains("userTitle: \"老板\""));
        assert!(block.contains("locale: \"zh\""));
    }

    #[test]
    fn yaml_scalar_quotes_and_escapes() {
        assert_eq!(yaml_scalar("/a/b"), "\"/a/b\"");
        assert_eq!(yaml_scalar(r#"/a"b"#), r#""/a\"b""#);
        assert_eq!(yaml_scalar(r"C:\x"), "\"C:\\\\x\"");
    }

    #[test]
    fn strip_block_removes_only_the_fleet_region() {
        let text = format!(
            "- id: mine\n  name: user-plugin\n{}\n{}",
            render_block(std::path::Path::new("/p/index.js"), None, "Boss", "en").trim_end(),
            "- id: after\n  name: other\n"
        );
        let stripped = strip_block(&text);
        assert!(stripped.contains("name: user-plugin"));
        assert!(stripped.contains("name: other"));
        assert!(!stripped.contains(ENTRY_ID));
        assert!(!stripped.contains(SENTINEL_BEGIN));
    }

    #[test]
    fn strip_block_is_a_noop_without_the_sentinel() {
        assert_eq!(strip_block("- id: mine\n"), "- id: mine\n");
    }

    #[test]
    fn strip_block_drops_a_half_written_block_missing_its_end_marker() {
        let text = format!("- id: mine\n{SENTINEL_BEGIN}\n- insert:\n");
        assert_eq!(strip_block(&text), "- id: mine\n");
    }

    #[test]
    fn has_user_entries_ignores_comments_blanks_and_the_placeholder() {
        assert!(!has_user_entries(""));
        assert!(!has_user_entries("[]\n"));
        assert!(!has_user_entries("# just a comment\n\n[]\n"));
        assert!(has_user_entries("- id: mine\n"));
    }

    /// Composition contract, exercised without touching a real `$DSH_HOME`:
    /// install → uninstall must round-trip a user's own entries, and uninstall
    /// must never leave a file that dsh would throw on.
    #[test]
    fn install_then_uninstall_round_trips_user_entries() {
        let block = render_block(std::path::Path::new("/p/index.js"), None, "Boss", "en");

        // Fresh file (dsh's template is a lone `[]`).
        let installed = {
            let user = strip_block("[]\n");
            assert!(!has_user_entries(&user));
            block.clone()
        };
        assert!(!installed.contains("[]"), "the placeholder must be dropped");

        // Reinstall over an already-installed file is idempotent.
        assert_eq!(strip_block(&installed), "");

        // User adds their own entry above ours, then Fleet reinstalls.
        let with_user = format!("- id: mine\n  name: other\n{installed}");
        let user = strip_block(&with_user);
        assert!(has_user_entries(&user));
        let reinstalled = format!("{}\n{block}", user.trim_end());
        assert!(reinstalled.contains("name: other"));
        assert_eq!(reinstalled.matches(SENTINEL_BEGIN).count(), 1);

        // Uninstall keeps the user's entry and drops ours.
        let uninstalled = strip_block(&reinstalled);
        assert!(uninstalled.contains("name: other"));
        assert!(!uninstalled.contains(ENTRY_ID));
    }

    #[test]
    fn compiled_in_plugin_is_the_repo_plugin() {
        // A stale include_str! target would silently ship an empty plugin.
        assert!(PLUGIN_JS.contains("agent/pre-step"));
        assert!(PLUGIN_JS.contains("dsh-context"));
        assert!(PLUGIN_JS.contains("kind: 'plugin'"));
        assert!(PLUGIN_PACKAGE_JSON.contains("\"type\": \"module\""));
    }
}
