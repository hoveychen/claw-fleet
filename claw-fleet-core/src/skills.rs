//! Skills scanning for Claude Code and Codex.
//!
//! Supports two layouts:
//!   • Directory-based: `~/.claude/skills/<name>/SKILL.md`
//!   • Flat file:       `~/.claude/skills/<name>.md`
//!
//! Name and description are extracted from YAML frontmatter when present.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::session::get_claude_dir;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillItem {
    /// Agent family that discovers this skill (`claude-code` or `codex`).
    #[serde(default = "default_skill_source")]
    pub source: String,
    /// Discovery scope (`user`, `repo`, `admin`, or `system`).
    #[serde(default = "default_skill_scope")]
    pub scope: String,
    /// Skill name — from frontmatter `name:` or the directory/file stem.
    pub name: String,
    /// Short description — from frontmatter `description:` or empty string.
    pub description: String,
    /// Absolute path to the skill file (`SKILL.md` or `<name>.md`).
    pub path: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
    /// Built-in/admin skills are inspectable but not removable from Fleet.
    #[serde(default = "default_true")]
    pub can_delete: bool,
}

fn default_skill_source() -> String {
    "claude-code".to_string()
}
fn default_skill_scope() -> String {
    "user".to_string()
}
fn default_true() -> bool {
    true
}

/// A file or directory inside a skill's root directory. Returned as a flat list;
/// the frontend reconstructs the tree by splitting `relative_path` on `/`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileEntry {
    /// Final path component (e.g. `foo.py`, `scripts`).
    pub name: String,
    /// Path relative to the skill root, with forward slashes (e.g. `scripts/foo.py`).
    pub relative_path: String,
    /// Absolute filesystem path; suitable for passing to `read_skill_file`.
    pub absolute_path: String,
    pub size_bytes: u64,
    pub is_dir: bool,
}

// ── Scan ──────────────────────────────────────────────────────────────────────

pub fn scan_all_skills() -> Vec<SkillItem> {
    scan_all_skills_for_workspaces(&[])
}

/// Scan global skills plus `.agents/skills` roots belonging to known
/// workspaces. Codex intentionally permits duplicate names from different
/// scopes, so de-duplication is by canonical SKILL.md path only.
pub fn scan_all_skills_for_workspaces(workspaces: &[String]) -> Vec<SkillItem> {
    let mut results = Vec::new();
    let mut seen_roots = std::collections::HashSet::new();

    if let Some(claude_dir) = get_claude_dir() {
        scan_skill_root(
            &claude_dir.join("skills"),
            "claude-code",
            "user",
            true,
            true,
            &mut seen_roots,
            &mut results,
        );
    }

    if let Some(home) = crate::session::real_home_dir() {
        scan_skill_root(
            &home.join(".agents").join("skills"),
            "codex",
            "user",
            true,
            false,
            &mut seen_roots,
            &mut results,
        );
    }

    if let Some(codex_home) = codex_home_dir() {
        scan_skill_root(
            &codex_home.join("skills"),
            "codex",
            "system",
            false,
            false,
            &mut seen_roots,
            &mut results,
        );
    }

    scan_skill_root(
        Path::new("/etc/codex/skills"),
        "codex",
        "admin",
        false,
        false,
        &mut seen_roots,
        &mut results,
    );

    for workspace in workspaces {
        for root in repo_skill_roots(Path::new(workspace)) {
            scan_skill_root(
                &root,
                "codex",
                "repo",
                true,
                false,
                &mut seen_roots,
                &mut results,
            );
        }
    }

    results.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.scope.cmp(&b.scope))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    results
}

fn codex_home_dir() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::session::real_home_dir().map(|h| h.join(".codex")))
}

#[allow(clippy::too_many_arguments)]
fn scan_skill_root(
    root: &Path,
    source: &str,
    scope: &str,
    can_delete: bool,
    allow_flat: bool,
    seen_roots: &mut std::collections::HashSet<PathBuf>,
    results: &mut Vec<SkillItem>,
) {
    let Ok(canonical_root) = fs::canonicalize(root) else {
        return;
    };
    if !seen_roots.insert(canonical_root.clone()) {
        return;
    }

    let Ok(entries) = fs::read_dir(&canonical_root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Directory-based skill: <name>/SKILL.md
            let skill_file = path.join("SKILL.md");
            if skill_file.is_file() {
                if let Some(item) = read_skill_item(&skill_file, &path, source, scope, can_delete) {
                    results.push(item);
                }
            } else if scope == "system" {
                // Codex bundles its built-ins under `$CODEX_HOME/skills/.system/*`.
                let Ok(children) = fs::read_dir(&path) else {
                    continue;
                };
                for child in children.flatten() {
                    let child_path = child.path();
                    let child_skill = child_path.join("SKILL.md");
                    if child_skill.is_file() {
                        if let Some(item) =
                            read_skill_item(&child_skill, &child_path, source, scope, can_delete)
                        {
                            results.push(item);
                        }
                    }
                }
            }
        } else if allow_flat && path.is_file() {
            // Flat skill file: <name>.md
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Some(item) = read_skill_item(&path, &path, source, scope, can_delete) {
                    results.push(item);
                }
            }
        }
    }
}

fn repo_skill_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut current = workspace.to_path_buf();
    loop {
        let candidate = current.join(".agents").join("skills");
        if candidate.is_dir() {
            roots.push(candidate);
        }
        let at_repo_root = current.join(".git").exists();
        if at_repo_root || !current.pop() {
            break;
        }
    }
    roots
}

fn read_skill_item(
    skill_file: &Path,
    name_source: &Path,
    source: &str,
    scope: &str,
    can_delete: bool,
) -> Option<SkillItem> {
    let metadata = fs::metadata(skill_file).ok()?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let size_bytes = metadata.len();

    let content = fs::read_to_string(skill_file).ok()?;
    let (name, description) = parse_frontmatter(&content, name_source);

    Some(SkillItem {
        source: source.to_string(),
        scope: scope.to_string(),
        name,
        description,
        path: skill_file.to_string_lossy().to_string(),
        size_bytes,
        modified_ms,
        can_delete,
    })
}

/// Parse YAML frontmatter between `---` delimiters for `name:` and `description:`.
fn parse_frontmatter(content: &str, name_source: &Path) -> (String, String) {
    let fallback_name = name_source
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let rest = if let Some(r) = content.strip_prefix("---\n") {
        r
    } else if let Some(r) = content.strip_prefix("---\r\n") {
        r
    } else {
        return (fallback_name, String::new());
    };

    let Some(end) = rest.find("\n---") else {
        return (fallback_name, String::new());
    };

    let frontmatter = &rest[..end];
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;

    for line in frontmatter.lines() {
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        }
    }

    (
        name.unwrap_or(fallback_name),
        description.unwrap_or_default(),
    )
}

// ── List files inside a skill ────────────────────────────────────────────────

const MAX_SKILL_TREE_DEPTH: usize = 6;

/// List every file and subdirectory inside a skill's root.
///
/// `skill_path` is the same value carried in `SkillItem.path` — the path to
/// `SKILL.md` for directory-based skills, or the flat `<name>.md` file for
/// flat skills. For flat skills this returns a single entry (the file itself).
pub fn list_skill_files(skill_path: &str) -> Result<Vec<SkillFileEntry>, String> {
    let canonical = fs::canonicalize(skill_path).map_err(|e| e.to_string())?;
    let (canonical_skills_dir, _) = allowed_skill_root(&canonical)
        .ok_or_else(|| "path is outside allowed skills directories".to_string())?;

    let root: PathBuf = if canonical.is_file() {
        let parent = canonical
            .parent()
            .ok_or_else(|| "invalid skill path".to_string())?;
        if parent == canonical_skills_dir.as_path() {
            // Flat skill (~/.claude/skills/<name>.md): single-file entry.
            let metadata = fs::metadata(&canonical).map_err(|e| e.to_string())?;
            let name = canonical
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            return Ok(vec![SkillFileEntry {
                relative_path: name.clone(),
                absolute_path: canonical.to_string_lossy().to_string(),
                size_bytes: metadata.len(),
                is_dir: false,
                name,
            }]);
        }
        parent.to_path_buf()
    } else if canonical.is_dir() {
        canonical.clone()
    } else {
        return Err("skill path does not exist".into());
    };

    let mut entries = Vec::new();
    walk_skill_dir(&root, &root, &mut entries, 0);
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(entries)
}

fn walk_skill_dir(root: &Path, dir: &Path, out: &mut Vec<SkillFileEntry>, depth: usize) {
    if depth > MAX_SKILL_TREE_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let is_dir = metadata.is_dir();
        let rel = path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .map(|s| s.replace('\\', "/"))
            .unwrap_or_default();
        out.push(SkillFileEntry {
            name: name.to_string(),
            relative_path: rel,
            absolute_path: path.to_string_lossy().to_string(),
            size_bytes: if is_dir { 0 } else { metadata.len() },
            is_dir,
        });
        if is_dir {
            walk_skill_dir(root, &path, out, depth + 1);
        }
    }
}

// ── Read file content ─────────────────────────────────────────────────────────

// ── Delete a skill ────────────────────────────────────────────────────────────

/// Delete a skill identified by its `skill_path` (the same value carried in
/// `SkillItem.path`).
///
/// * Directory-based skill (`~/.claude/skills/<name>/SKILL.md`) → recursively
///   removes the parent directory.
/// * Flat skill (`~/.claude/skills/<name>.md`) → removes the single file.
///
/// The path is canonicalized and must resolve inside `~/.claude/skills/` —
/// anything outside is rejected.
pub fn delete_skill(skill_path: &str) -> Result<(), String> {
    let canonical = fs::canonicalize(skill_path).map_err(|e| e.to_string())?;
    let (canonical_skills_dir, can_delete) = allowed_skill_root(&canonical)
        .ok_or_else(|| "path is outside allowed skills directories".to_string())?;
    if !can_delete {
        return Err("built-in and admin skills cannot be deleted".into());
    }
    if canonical == canonical_skills_dir {
        return Err("refusing to delete the skills root".into());
    }

    if canonical.is_file() {
        let parent = canonical
            .parent()
            .ok_or_else(|| "invalid skill path".to_string())?;
        if parent == canonical_skills_dir.as_path() {
            // Flat skill: drop just the file.
            return fs::remove_file(&canonical).map_err(|e| e.to_string());
        }
        // Directory-based skill: drop the whole containing directory.
        // Sanity: parent must itself be inside skills_dir.
        if !parent.starts_with(&canonical_skills_dir) {
            return Err("skill parent escapes skills directory".into());
        }
        return fs::remove_dir_all(parent).map_err(|e| e.to_string());
    }

    if canonical.is_dir() {
        return fs::remove_dir_all(&canonical).map_err(|e| e.to_string());
    }

    Err("skill path does not exist".into())
}

pub fn read_skill_file(path: &str) -> Result<String, String> {
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if allowed_skill_root(&canonical).is_none() {
        return Err("path is outside allowed skills directories".into());
    }

    fs::read_to_string(path).map_err(|e| e.to_string())
}

/// Resolve the discovery root containing `path` and whether Fleet may remove
/// skills from it. Repo `.agents/skills` roots are recognized structurally so
/// repositories outside the user's home remain inspectable over local/remote
/// backends as well.
fn allowed_skill_root(path: &Path) -> Option<(PathBuf, bool)> {
    let mut candidates = Vec::new();
    if let Some(claude_dir) = get_claude_dir() {
        candidates.push((claude_dir.join("skills"), true));
    }
    if let Some(home) = crate::session::real_home_dir() {
        candidates.push((home.join(".agents").join("skills"), true));
    }
    if let Some(codex_home) = codex_home_dir() {
        candidates.push((codex_home.join("skills"), false));
    }
    candidates.push((PathBuf::from("/etc/codex/skills"), false));

    for (root, can_delete) in candidates {
        if let Ok(root) = fs::canonicalize(root) {
            if path.starts_with(&root) {
                return Some((root, can_delete));
            }
        }
    }

    for ancestor in path.ancestors() {
        if ancestor.file_name().and_then(|n| n.to_str()) == Some("skills")
            && ancestor
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some(".agents")
        {
            return Some((ancestor.to_path_buf(), true));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_extracts_name_and_description() {
        let content =
            "---\nname: fleet\ndescription: Monitor agents\nallowed-tools: Bash\n---\n\n# Body";
        let path = Path::new("/tmp/fleet/SKILL.md");
        let (name, desc) = parse_frontmatter(content, path);
        assert_eq!(name, "fleet");
        assert_eq!(desc, "Monitor agents");
    }

    #[test]
    fn parse_frontmatter_falls_back_to_stem() {
        let content = "No frontmatter here.";
        let path = Path::new("/tmp/my-skill.md");
        let (name, desc) = parse_frontmatter(content, path);
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "");
    }

    #[test]
    fn parse_frontmatter_partial_fields() {
        let content = "---\nname: custom\n---\nContent";
        let path = Path::new("/tmp/other.md");
        let (name, desc) = parse_frontmatter(content, path);
        assert_eq!(name, "custom");
        assert_eq!(desc, "");
    }

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }

    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    fn make_skills_dir(home: &Path) -> PathBuf {
        let d = home.join(".claude").join("skills");
        fs::create_dir_all(&d).unwrap();
        d
    }

    struct CodexHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl CodexHomeOverride {
        fn new(path: &Path) -> Self {
            let prev = std::env::var_os("CODEX_HOME");
            unsafe { std::env::set_var("CODEX_HOME", path) };
            Self { prev }
        }
    }

    impl Drop for CodexHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(path) => std::env::set_var("CODEX_HOME", path),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
        }
    }

    #[test]
    fn scans_codex_user_repo_and_system_scopes() {
        let _lock = crate::session::fleet_home_lock();
        let temp = tempfile::tempdir().unwrap();
        let _home = FleetHomeOverride::new(temp.path());
        let codex_home = temp.path().join(".codex");
        let _codex = CodexHomeOverride::new(&codex_home);

        let user = temp.path().join(".agents/skills/user-skill");
        fs::create_dir_all(&user).unwrap();
        fs::write(
            user.join("SKILL.md"),
            "---\nname: user-skill\ndescription: user\n---\n",
        )
        .unwrap();

        let system = codex_home.join("skills/.system/system-skill");
        fs::create_dir_all(&system).unwrap();
        fs::write(
            system.join("SKILL.md"),
            "---\nname: system-skill\ndescription: system\n---\n",
        )
        .unwrap();

        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let repo_skill = repo.join(".agents/skills/repo-skill");
        fs::create_dir_all(&repo_skill).unwrap();
        fs::write(
            repo_skill.join("SKILL.md"),
            "---\nname: repo-skill\ndescription: repo\n---\n",
        )
        .unwrap();

        let skills = scan_all_skills_for_workspaces(&[repo.to_string_lossy().to_string()]);
        assert!(skills
            .iter()
            .any(|skill| skill.name == "user-skill" && skill.scope == "user"));
        assert!(skills
            .iter()
            .any(|skill| skill.name == "repo-skill" && skill.scope == "repo"));
        let system_item = skills
            .iter()
            .find(|skill| skill.name == "system-skill")
            .unwrap();
        assert_eq!(system_item.source, "codex");
        assert!(!system_item.can_delete);
        assert!(delete_skill(&system_item.path)
            .unwrap_err()
            .contains("cannot be deleted"));
        assert!(read_skill_file(&system_item.path)
            .unwrap()
            .contains("description: system"));
    }

    #[test]
    fn delete_skill_removes_directory_based_skill() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let skills_dir = make_skills_dir(tmp.path());

        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        fs::write(&skill_md, "---\nname: my-skill\n---\nBody").unwrap();
        fs::write(skill_dir.join("scripts").join("run.sh"), "echo hi").unwrap();

        delete_skill(skill_md.to_str().unwrap()).unwrap();
        assert!(!skill_dir.exists(), "skill directory should be gone");
        assert!(skills_dir.exists(), "skills root must remain");
    }

    #[test]
    fn delete_skill_removes_flat_md_file() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let skills_dir = make_skills_dir(tmp.path());

        let flat = skills_dir.join("flat-skill.md");
        fs::write(&flat, "# flat").unwrap();

        delete_skill(flat.to_str().unwrap()).unwrap();
        assert!(!flat.exists(), "flat skill file should be gone");
        assert!(skills_dir.exists(), "skills root must remain");
    }

    #[test]
    fn delete_skill_rejects_path_outside_skills_dir() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let _skills_dir = make_skills_dir(tmp.path());

        // Put a victim file in ~/.claude/ but outside skills/
        let victim = tmp.path().join(".claude").join("victim.md");
        fs::write(&victim, "do not delete").unwrap();

        let err = delete_skill(victim.to_str().unwrap()).unwrap_err();
        assert!(
            err.contains("outside allowed skills directories"),
            "got: {err}"
        );
        assert!(victim.exists(), "victim file must NOT be deleted");
    }
}
