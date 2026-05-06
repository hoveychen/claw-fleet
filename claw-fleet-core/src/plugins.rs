//! Plugin scanning — reads installed Claude Code plugins from
//! `~/.claude/plugins/marketplaces/<mk>/{plugins,external_plugins}/<name>/`.
//!
//! Each plugin is identified by its `<name>@<marketplace>` id (matching the
//! key format used by `~/.claude.json`'s `enabledPlugins` and the install-
//! counts cache). Contributions (commands / agents / skills / hooks / MCP)
//! are surfaced as simple counts so the UI can render badges without
//! re-walking each plugin's directory tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::claude_cli::{self, CliPlugin};
use crate::session::get_claude_dir;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginItem {
    pub name: String,
    pub description: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub homepage: Option<String>,
    /// Marketplace directory name (e.g. "claude-plugins-official").
    pub marketplace: String,
    /// "internal" for `marketplaces/<mk>/plugins/<name>`,
    /// "external" for `marketplaces/<mk>/external_plugins/<name>`.
    pub source_kind: String,
    /// Canonical `<name>@<marketplace>` id used by enabledPlugins / counts.
    pub plugin_id: String,
    pub enabled: bool,
    pub install_count: Option<u64>,
    pub root_path: String,
    pub manifest_path: String,
    pub contributes: PluginContributions,
    /// `true` when the plugin's directory exists locally under
    /// `~/.claude/plugins/marketplaces/<mk>/{plugins,external_plugins}/<name>/`.
    /// `false` for catalog-only entries surfaced via the `claude plugin
    /// list --available` CLI but not yet fetched to disk.
    #[serde(default)]
    pub is_downloaded: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginContributions {
    pub commands: u32,
    pub agents: u32,
    pub skills: u32,
    pub hooks: bool,
    pub mcp: bool,
}

// ── Public scan entry point ──────────────────────────────────────────────────

pub fn scan_all_plugins() -> Vec<PluginItem> {
    let Some(claude_dir) = get_claude_dir() else {
        return vec![];
    };
    let plugins_dir = claude_dir.join("plugins");
    let marketplaces_dir = plugins_dir.join("marketplaces");
    if !marketplaces_dir.is_dir() {
        return vec![];
    }

    let enabled = read_enabled_plugins();
    let install_counts = read_install_counts(&plugins_dir);

    let mut results = Vec::new();
    let Ok(entries) = fs::read_dir(&marketplaces_dir) else {
        return results;
    };
    for entry in entries.flatten() {
        let mp_dir = entry.path();
        if !mp_dir.is_dir() {
            continue;
        }
        let Some(mp_name) = mp_dir.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        scan_plugin_dir(
            &mp_dir.join("plugins"),
            &mp_name,
            "internal",
            &enabled,
            &install_counts,
            &mut results,
        );
        scan_plugin_dir(
            &mp_dir.join("external_plugins"),
            &mp_name,
            "external",
            &enabled,
            &install_counts,
            &mut results,
        );
    }

    results.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    results
}

// ── Scan + CLI catalog merge ─────────────────────────────────────────────────

/// Returns the union of:
///   - everything `scan_all_plugins` finds on disk (`is_downloaded: true`)
///   - everything `claude plugin list --json --available` returns whose
///     `pluginId` isn't already in the on-disk set (`is_downloaded: false`)
///
/// On CLI failure (binary missing, non-zero exit, garbled JSON) we silently
/// fall back to the on-disk scan only — Plugins page should still render.
pub fn scan_with_catalog() -> Vec<PluginItem> {
    let scanned = scan_all_plugins();
    let enabled = read_enabled_plugins();
    match claude_cli::list_plugins(true) {
        Ok(resp) => merge_with_catalog(scanned, &resp.available, &enabled),
        Err(_) => scanned,
    }
}

/// Pure helper: append catalog-only entries (those whose `pluginId` is not
/// already present in `scanned`) and re-sort by name. Exposed for testing.
pub fn merge_with_catalog(
    mut scanned: Vec<PluginItem>,
    catalog: &[CliPlugin],
    enabled: &BTreeMap<String, bool>,
) -> Vec<PluginItem> {
    let on_disk: BTreeSet<String> =
        scanned.iter().map(|p| p.plugin_id.clone()).collect();
    for cp in catalog {
        if on_disk.contains(&cp.plugin_id) {
            continue;
        }
        let enabled_flag = enabled.get(&cp.plugin_id).copied().unwrap_or(false);
        scanned.push(PluginItem {
            name: cp.name.clone(),
            description: cp.description.clone(),
            author: None,
            version: None,
            homepage: None,
            marketplace: cp.marketplace_name.clone(),
            source_kind: "catalog".to_string(),
            plugin_id: cp.plugin_id.clone(),
            enabled: enabled_flag,
            install_count: cp.install_count,
            root_path: String::new(),
            manifest_path: String::new(),
            contributes: PluginContributions {
                commands: 0,
                agents: 0,
                skills: 0,
                hooks: false,
                mcp: false,
            },
            is_downloaded: false,
        });
    }
    scanned.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    scanned
}

// ── Per-directory scan ────────────────────────────────────────────────────────

fn scan_plugin_dir(
    dir: &Path,
    marketplace: &str,
    source_kind: &str,
    enabled: &BTreeMap<String, bool>,
    install_counts: &BTreeMap<String, u64>,
    out: &mut Vec<PluginItem>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let plugin_dir = entry.path();
        if !plugin_dir.is_dir() {
            continue;
        }
        let manifest_path = plugin_dir.join(".claude-plugin").join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };

        let dir_name = plugin_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let name = json
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(dir_name);
        let description = json
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let version = json
            .get("version")
            .and_then(|v| v.as_str())
            .map(String::from);
        let homepage = json
            .get("homepage")
            .and_then(|v| v.as_str())
            .map(String::from);
        let author = json
            .get("author")
            .and_then(|a| a.get("name").and_then(|v| v.as_str()).map(String::from))
            .or_else(|| {
                json.get("author")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });

        let plugin_id = format!("{}@{}", name, marketplace);
        let enabled_flag = enabled.get(&plugin_id).copied().unwrap_or(false);
        let install_count = install_counts.get(&plugin_id).copied();
        let contributes = scan_contributions(&plugin_dir);

        out.push(PluginItem {
            name,
            description,
            author,
            version,
            homepage,
            marketplace: marketplace.to_string(),
            source_kind: source_kind.to_string(),
            plugin_id,
            enabled: enabled_flag,
            install_count,
            root_path: plugin_dir.to_string_lossy().to_string(),
            manifest_path: manifest_path.to_string_lossy().to_string(),
            contributes,
            is_downloaded: true,
        });
    }
}

fn scan_contributions(plugin_dir: &Path) -> PluginContributions {
    let count_md = |sub: &str| -> u32 {
        let target = plugin_dir.join(sub);
        let Ok(rd) = fs::read_dir(&target) else {
            return 0;
        };
        rd.flatten()
            .filter(|e| {
                e.path().extension().and_then(|s| s.to_str()) == Some("md")
                    && e.path().file_name().and_then(|s| s.to_str())
                        != Some("README.md")
            })
            .count() as u32
    };
    let count_skills = || -> u32 {
        let target = plugin_dir.join("skills");
        let Ok(rd) = fs::read_dir(&target) else {
            return 0;
        };
        rd.flatten()
            .filter(|e| {
                let p = e.path();
                if p.is_dir() {
                    p.join("SKILL.md").is_file()
                } else {
                    p.extension().and_then(|s| s.to_str()) == Some("md")
                }
            })
            .count() as u32
    };
    PluginContributions {
        commands: count_md("commands"),
        agents: count_md("agents"),
        skills: count_skills(),
        hooks: plugin_dir.join("hooks").is_dir() || plugin_dir.join("hooks.json").is_file(),
        mcp: plugin_dir.join(".mcp.json").is_file() || plugin_dir.join("mcp.json").is_file(),
    }
}

// ── Sidecar files: enabledPlugins + install counts ───────────────────────────

fn read_enabled_plugins() -> BTreeMap<String, bool> {
    let Some(home) = dirs::home_dir() else {
        return BTreeMap::new();
    };
    let path = home.join(".claude.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    parse_enabled_plugins(&raw)
}

fn parse_enabled_plugins(raw: &str) -> BTreeMap<String, bool> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    if let Some(obj) = json.get("enabledPlugins").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            let enabled = v.as_bool().unwrap_or_else(|| {
                v.get("enabled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
            });
            out.insert(k.clone(), enabled);
        }
    }
    out
}

fn read_install_counts(plugins_dir: &Path) -> BTreeMap<String, u64> {
    let path = plugins_dir.join("install-counts-cache.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    parse_install_counts(&raw)
}

fn parse_install_counts(raw: &str) -> BTreeMap<String, u64> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    if let Some(arr) = json.get("counts").and_then(|v| v.as_array()) {
        for item in arr {
            if let (Some(plugin), Some(n)) = (
                item.get("plugin").and_then(|v| v.as_str()),
                item.get("unique_installs").and_then(|v| v.as_u64()),
            ) {
                out.insert(plugin.to_string(), n);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enabled_plugins_handles_bool_values() {
        let raw = r#"{"enabledPlugins": {"foo@mk": true, "bar@mk": false}}"#;
        let out = parse_enabled_plugins(raw);
        assert_eq!(out.get("foo@mk"), Some(&true));
        assert_eq!(out.get("bar@mk"), Some(&false));
    }

    #[test]
    fn parse_enabled_plugins_handles_object_values() {
        let raw = r#"{"enabledPlugins": {"foo@mk": {"enabled": true}}}"#;
        let out = parse_enabled_plugins(raw);
        assert_eq!(out.get("foo@mk"), Some(&true));
    }

    #[test]
    fn parse_enabled_plugins_handles_missing_field() {
        let raw = r#"{"otherField": 42}"#;
        let out = parse_enabled_plugins(raw);
        assert!(out.is_empty());
    }

    #[test]
    fn parse_install_counts_extracts_pairs() {
        let raw = r#"{
            "version": 1,
            "counts": [
                {"plugin": "foo@mk", "unique_installs": 100},
                {"plugin": "bar@mk", "unique_installs": 5}
            ]
        }"#;
        let out = parse_install_counts(raw);
        assert_eq!(out.get("foo@mk"), Some(&100));
        assert_eq!(out.get("bar@mk"), Some(&5));
    }

    #[test]
    fn parse_install_counts_handles_malformed() {
        let out = parse_install_counts("not json");
        assert!(out.is_empty());
    }

    #[test]
    fn scan_contributions_counts_md_files_in_subdirs() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let root = tmp.path();
        // commands/foo.md, commands/bar.md, commands/README.md (excluded)
        std::fs::create_dir(root.join("commands")).unwrap();
        std::fs::write(root.join("commands/foo.md"), "x").unwrap();
        std::fs::write(root.join("commands/bar.md"), "x").unwrap();
        std::fs::write(root.join("commands/README.md"), "x").unwrap();
        // agents/baz.md
        std::fs::create_dir(root.join("agents")).unwrap();
        std::fs::write(root.join("agents/baz.md"), "x").unwrap();
        // skills/dir-skill/SKILL.md (counts) + skills/flat.md (counts)
        std::fs::create_dir_all(root.join("skills/dir-skill")).unwrap();
        std::fs::write(root.join("skills/dir-skill/SKILL.md"), "x").unwrap();
        std::fs::write(root.join("skills/flat.md"), "x").unwrap();
        // hooks/ dir present
        std::fs::create_dir(root.join("hooks")).unwrap();
        // .mcp.json present
        std::fs::write(root.join(".mcp.json"), "{}").unwrap();

        let c = scan_contributions(root);
        assert_eq!(c.commands, 2, "README.md should be excluded");
        assert_eq!(c.agents, 1);
        assert_eq!(c.skills, 2);
        assert!(c.hooks);
        assert!(c.mcp);
    }

    #[test]
    fn scan_contributions_empty_when_no_subdirs() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = scan_contributions(tmp.path());
        assert_eq!(c.commands, 0);
        assert_eq!(c.agents, 0);
        assert_eq!(c.skills, 0);
        assert!(!c.hooks);
        assert!(!c.mcp);
    }

    fn make_scanned(plugin_id: &str, name: &str) -> PluginItem {
        PluginItem {
            name: name.to_string(),
            description: "scanned".to_string(),
            author: None,
            version: None,
            homepage: None,
            marketplace: "mk".to_string(),
            source_kind: "internal".to_string(),
            plugin_id: plugin_id.to_string(),
            enabled: false,
            install_count: None,
            root_path: format!("/tmp/{name}"),
            manifest_path: format!("/tmp/{name}/.claude-plugin/plugin.json"),
            contributes: PluginContributions {
                commands: 0,
                agents: 0,
                skills: 0,
                hooks: false,
                mcp: false,
            },
            is_downloaded: true,
        }
    }

    fn make_catalog(plugin_id: &str, name: &str, mk: &str) -> CliPlugin {
        CliPlugin {
            plugin_id: plugin_id.to_string(),
            name: name.to_string(),
            description: format!("desc-{name}"),
            marketplace_name: mk.to_string(),
            source: serde_json::Value::Null,
            install_count: Some(123),
        }
    }

    #[test]
    fn merge_with_catalog_appends_only_new_items() {
        let scanned = vec![
            make_scanned("alpha@mk", "alpha"),
            make_scanned("bravo@mk", "bravo"),
        ];
        let catalog = vec![
            // Already on disk — must be deduped, not duplicated.
            make_catalog("alpha@mk", "alpha", "mk"),
            // New entry — should be appended with is_downloaded=false.
            make_catalog("charlie@mk", "charlie", "mk"),
        ];
        let enabled = BTreeMap::new();
        let merged = merge_with_catalog(scanned, &catalog, &enabled);

        assert_eq!(merged.len(), 3, "alpha must not be duplicated");
        let charlie = merged
            .iter()
            .find(|p| p.plugin_id == "charlie@mk")
            .expect("charlie present");
        assert!(!charlie.is_downloaded);
        assert_eq!(charlie.source_kind, "catalog");
        assert_eq!(charlie.install_count, Some(123));
        assert!(charlie.root_path.is_empty());
    }

    #[test]
    fn merge_with_catalog_carries_enabled_flag_for_catalog_only_items() {
        let scanned: Vec<PluginItem> = vec![];
        let catalog = vec![make_catalog("delta@mk", "delta", "mk")];
        let mut enabled = BTreeMap::new();
        enabled.insert("delta@mk".to_string(), true);

        let merged = merge_with_catalog(scanned, &catalog, &enabled);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].enabled, "enabled map should be honoured");
    }

    #[test]
    fn merge_with_catalog_sorts_by_name() {
        let scanned = vec![make_scanned("zulu@mk", "zulu")];
        let catalog = vec![
            make_catalog("alpha@mk", "alpha", "mk"),
            make_catalog("mike@mk", "mike", "mk"),
        ];
        let merged = merge_with_catalog(scanned, &catalog, &BTreeMap::new());
        let order: Vec<&str> = merged.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(order, vec!["alpha", "mike", "zulu"]);
    }
}
