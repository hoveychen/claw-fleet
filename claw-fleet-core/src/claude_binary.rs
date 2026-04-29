//! Claude CLI binary discovery and selection.
//!
//! Enumerates every place Claude Code might be installed on the user's machine,
//! ranks the candidates by priority, and exposes a user-overridable resolver.
//!
//! Why this matters: a single user can simultaneously have Claude Code installed
//! by the native installer (`~/.local/bin/claude`), Homebrew, npm, **and**
//! bundled inside the VS Code / Cursor extension. The extension bundles
//! (`~/.{vscode,cursor}/extensions/anthropic.claude-code-<ver>-<plat>-<arch>/resources/native-binary/claude`)
//! are 200MB+ files that the older `which claude` + hardcoded path probe never
//! found, so users who only installed the IDE extension showed up as "Claude not
//! detected" even though they had a working CLI on disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::real_home_dir;

/// One candidate `claude` binary on disk.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeBinary {
    pub path: String,
    pub source: ClaudeBinarySource,
    /// Parsed version (e.g. "2.1.123") when the source carries one in its
    /// directory name. `None` for PATH/installer/Homebrew where the binary is
    /// just `claude` without an embedded version.
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeBinarySource {
    /// Whatever `which claude` / `where claude` returned.
    Path,
    /// `~/.local/bin/claude` (Anthropic native installer).
    NativeInstaller,
    /// `/opt/homebrew/bin/claude`.
    Homebrew,
    /// `~/.npm-global/bin/claude`.
    NpmGlobal,
    /// `/usr/local/bin/claude`.
    UsrLocalBin,
    /// `~/.vscode/extensions/anthropic.claude-code-*/resources/native-binary/claude`.
    VsCodeExtension,
    /// `~/.vscode-insiders/extensions/anthropic.claude-code-*/...`.
    VsCodeInsidersExtension,
    /// `~/.cursor/extensions/anthropic.claude-code-*/...`.
    CursorExtension,
    /// `~/.windsurf/extensions/anthropic.claude-code-*/...`.
    WindsurfExtension,
}

impl ClaudeBinarySource {
    /// Stable kebab-case key used for i18n lookup on the frontend.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::NativeInstaller => "native-installer",
            Self::Homebrew => "homebrew",
            Self::NpmGlobal => "npm-global",
            Self::UsrLocalBin => "usr-local-bin",
            Self::VsCodeExtension => "vscode-extension",
            Self::VsCodeInsidersExtension => "vscode-insiders-extension",
            Self::CursorExtension => "cursor-extension",
            Self::WindsurfExtension => "windsurf-extension",
        }
    }
}

/// Persisted user preference: when `override_path` is set and the file still
/// exists, callers will use it instead of the auto-picked candidate.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ClaudeBinaryConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_path: Option<String>,
}

impl ClaudeBinaryConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or("Cannot determine config path")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }
}

fn config_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet").join("claude-binary.json"))
}

/// Enumerate every Claude binary candidate on disk, ranked by priority.
/// Duplicates (same canonical path) are collapsed, keeping the first hit.
pub fn discover() -> Vec<ClaudeBinary> {
    let mut out: Vec<ClaudeBinary> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut push = |bin: ClaudeBinary, out: &mut Vec<ClaudeBinary>, seen: &mut std::collections::HashSet<String>| {
        let canon = std::fs::canonicalize(&bin.path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| bin.path.clone());
        if seen.insert(canon) {
            out.push(bin);
        }
    };

    // 1. PATH lookup.
    if let Some(p) = which_claude() {
        push(ClaudeBinary { path: p, source: ClaudeBinarySource::Path, version: None }, &mut out, &mut seen);
    }

    let home = real_home_dir();

    // 2-5. Hardcoded standard install locations.
    let standard = [
        (home.as_ref().map(|h| h.join(".local").join("bin").join("claude")), ClaudeBinarySource::NativeInstaller),
        (Some(PathBuf::from("/opt/homebrew/bin/claude")), ClaudeBinarySource::Homebrew),
        (home.as_ref().map(|h| h.join(".npm-global").join("bin").join("claude")), ClaudeBinarySource::NpmGlobal),
        (Some(PathBuf::from("/usr/local/bin/claude")), ClaudeBinarySource::UsrLocalBin),
    ];
    for (path_opt, source) in standard {
        if let Some(path) = path_opt {
            if path.exists() {
                push(
                    ClaudeBinary { path: path.to_string_lossy().to_string(), source, version: None },
                    &mut out,
                    &mut seen,
                );
            }
        }
    }

    // 6-9. IDE extension bundles. Each editor stores the binary at the same
    // relative path inside its extension dir. We pick the highest-version
    // extension under each editor — users routinely accumulate stale versions
    // (one user's machine had 5 in ~/.vscode/extensions side by side).
    if let Some(home) = home.as_ref() {
        let editor_roots: &[(PathBuf, ClaudeBinarySource)] = &[
            (home.join(".vscode").join("extensions"), ClaudeBinarySource::VsCodeExtension),
            (home.join(".vscode-insiders").join("extensions"), ClaudeBinarySource::VsCodeInsidersExtension),
            (home.join(".cursor").join("extensions"), ClaudeBinarySource::CursorExtension),
            (home.join(".windsurf").join("extensions"), ClaudeBinarySource::WindsurfExtension),
        ];
        for (root, source) in editor_roots {
            if let Some(bin) = scan_editor_extensions(root, source.clone()) {
                push(bin, &mut out, &mut seen);
            }
        }
    }

    out
}

/// Resolve the binary fleet should actually use:
/// - if `override_path` is set and points to an existing file, use it;
/// - otherwise the first candidate from [`discover`].
pub fn resolve(override_path: Option<&str>) -> Option<ClaudeBinary> {
    if let Some(p) = override_path {
        if !p.is_empty() && Path::new(p).exists() {
            // If the override matches a discovered candidate's path, prefer
            // that record so the UI/logs keep the source label.
            if let Some(found) = discover().into_iter().find(|c| c.path == p) {
                return Some(found);
            }
            return Some(ClaudeBinary {
                path: p.to_string(),
                source: ClaudeBinarySource::Path,
                version: None,
            });
        }
    }
    discover().into_iter().next()
}

/// Find the highest-version `anthropic.claude-code-*` extension dir under
/// `root` that contains a `resources/native-binary/claude` file.
fn scan_editor_extensions(root: &Path, source: ClaudeBinarySource) -> Option<ClaudeBinary> {
    if !root.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    let mut best: Option<(Vec<u32>, String, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix("anthropic.claude-code-") else { continue };
        // rest ~ "2.1.123-darwin-arm64"; version is the leading "X.Y.Z" segment.
        let ver_str = rest.split('-').next()?.to_string();
        let Some(ver) = parse_version(&ver_str) else { continue };
        let bin = entry.path().join("resources").join("native-binary").join("claude");
        if !bin.is_file() { continue }
        match &best {
            Some((cur, _, _)) if *cur >= ver => {}
            _ => best = Some((ver, ver_str, bin)),
        }
    }
    best.map(|(_, ver_str, bin)| ClaudeBinary {
        path: bin.to_string_lossy().to_string(),
        source,
        version: Some(ver_str),
    })
}

fn parse_version(s: &str) -> Option<Vec<u32>> {
    s.split('.').map(|p| p.parse::<u32>().ok()).collect()
}

fn which_claude() -> Option<String> {
    #[cfg(unix)]
    let cmd = "which";
    #[cfg(not(unix))]
    let cmd = "where";
    let output = std::process::Command::new(cmd).arg("claude").output().ok()?;
    if !output.status.success() { return None }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // `where` on Windows can list multiple matches separated by newlines —
    // take only the first.
    let first = s.lines().next()?.trim().to_string();
    if first.is_empty() { None } else { Some(first) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_version_orders_correctly() {
        let mut versions = vec![
            parse_version("2.1.94").unwrap(),
            parse_version("2.1.123").unwrap(),
            parse_version("2.1.118").unwrap(),
            parse_version("2.1.120").unwrap(),
            parse_version("2.1.121").unwrap(),
        ];
        versions.sort();
        assert_eq!(versions.last().unwrap(), &vec![2, 1, 123]);
        assert_eq!(versions.first().unwrap(), &vec![2, 1, 94]);
    }

    #[test]
    fn parse_version_rejects_non_numeric_segments() {
        assert!(parse_version("2.1.123-rc1").is_none());
        assert!(parse_version("foo").is_none());
        assert!(parse_version("").is_none());
    }

    #[test]
    fn scan_editor_extensions_picks_highest_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Mirror the real layout the user has on disk: 5 versions, the binary
        // we want is buried inside the highest one's resources/native-binary.
        for v in &["2.1.94", "2.1.118", "2.1.120", "2.1.121", "2.1.123"] {
            let dir = root.join(format!("anthropic.claude-code-{}-darwin-arm64", v))
                .join("resources").join("native-binary");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("claude"), b"fake").unwrap();
        }
        // Plus a stray non-matching dir to make sure we ignore it.
        fs::create_dir_all(root.join("ms-vscode.cpptools")).unwrap();

        let bin = scan_editor_extensions(root, ClaudeBinarySource::VsCodeExtension).unwrap();
        assert_eq!(bin.version.as_deref(), Some("2.1.123"));
        assert_eq!(bin.source, ClaudeBinarySource::VsCodeExtension);
        assert!(bin.path.contains("anthropic.claude-code-2.1.123"));
    }

    #[test]
    fn scan_editor_extensions_skips_dirs_missing_native_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Two extension dirs; only the older one has the binary file. We must
        // pick the older one rather than returning the newer broken install.
        for v in &["2.1.94", "2.1.123"] {
            fs::create_dir_all(root.join(format!("anthropic.claude-code-{}-darwin-arm64", v))).unwrap();
        }
        let dir94 = root.join("anthropic.claude-code-2.1.94-darwin-arm64")
            .join("resources").join("native-binary");
        fs::create_dir_all(&dir94).unwrap();
        fs::write(dir94.join("claude"), b"fake").unwrap();

        let bin = scan_editor_extensions(root, ClaudeBinarySource::CursorExtension).unwrap();
        assert_eq!(bin.version.as_deref(), Some("2.1.94"));
    }

    #[test]
    fn scan_editor_extensions_returns_none_when_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("does-not-exist");
        assert!(scan_editor_extensions(&root, ClaudeBinarySource::WindsurfExtension).is_none());
    }

    #[test]
    fn resolve_uses_override_when_path_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("my-claude");
        fs::write(&fake, b"fake").unwrap();
        let resolved = resolve(Some(fake.to_str().unwrap())).unwrap();
        assert_eq!(resolved.path, fake.to_string_lossy());
    }

    #[test]
    fn resolve_ignores_override_when_path_missing() {
        // Override points to a non-existent file → fall back to discover().
        // We can't assert what discover returns (depends on host), but we can
        // assert resolve does not crash and does not return the bogus path.
        let resolved = resolve(Some("/does/not/exist/claude"));
        if let Some(b) = resolved {
            assert_ne!(b.path, "/does/not/exist/claude");
        }
    }
}
