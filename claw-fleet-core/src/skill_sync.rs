//! Cross-runtime user-skill management for Claude Code and Codex.
//!
//! Fleet keeps an authoritative copy under `~/.fleet/skills/<slug>` and
//! projects it into each runtime's native discovery root. Existing unmanaged
//! skills are never overwritten: callers must explicitly adopt one first.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::session::{get_fleet_dir, real_home_dir};

const SKILL_FILE: &str = "SKILL.md";
const COPY_MARKER: &str = ".fleet-managed.json";
const MAX_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SkillTarget {
    ClaudeCode,
    Codex,
}

impl SkillTarget {
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SkillCompatibility {
    Both,
    ClaudeOnly,
    CodexOnly,
    Incompatible,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSyncState {
    Shared,
    Partial,
    Conflict,
    Unmanaged,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSyncEntry {
    pub slug: String,
    pub state: SkillSyncState,
    pub compatibility: SkillCompatibility,
    pub warnings: Vec<String>,
    pub canonical_path: Option<String>,
    pub claude_path: Option<String>,
    pub codex_path: Option<String>,
    pub claude_managed: bool,
    pub codex_managed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSyncAction {
    pub slug: String,
    pub target: String,
    pub action: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSyncReport {
    pub items: Vec<SkillSyncEntry>,
    pub actions: Vec<SkillSyncAction>,
    pub conflicts: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyMarker {
    canonical_path: String,
    content_hash: String,
}

#[derive(Clone)]
struct Roots {
    canonical: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
}

fn roots() -> Result<Roots, String> {
    let home = real_home_dir().ok_or_else(|| "cannot determine user home".to_string())?;
    let fleet = get_fleet_dir().ok_or_else(|| "cannot determine Fleet directory".to_string())?;
    Ok(Roots {
        canonical: fleet.join("skills"),
        claude: home.join(".claude").join("skills"),
        codex: home.join(".agents").join("skills"),
    })
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && !slug.starts_with('.')
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn target_path(roots: &Roots, slug: &str, target: SkillTarget) -> PathBuf {
    match target {
        SkillTarget::ClaudeCode => roots.claude.join(slug),
        SkillTarget::Codex => roots.codex.join(slug),
    }
}

fn scan_skill_dirs(root: &Path, allow_flat: bool) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.join(SKILL_FILE).is_file() {
            out.insert(name.to_string(), path);
        } else if allow_flat
            && path.is_file()
            && path.extension().and_then(|v| v.to_str()) == Some("md")
        {
            if let Some(stem) = path.file_stem().and_then(|v| v.to_str()) {
                out.insert(stem.to_string(), path);
            }
        }
    }
    out
}

fn skill_file(root: &Path) -> PathBuf {
    if root.is_file() {
        root.to_path_buf()
    } else {
        root.join(SKILL_FILE)
    }
}

fn compatibility(root: &Path) -> (SkillCompatibility, Vec<String>) {
    let path = skill_file(root);
    let Ok(body) = fs::read_to_string(&path) else {
        return (
            SkillCompatibility::Incompatible,
            vec![format!("cannot read {}", path.display())],
        );
    };
    let mut warnings = Vec::new();
    let mut invalid_metadata = false;
    if !(body.starts_with("---\n") || body.starts_with("---\r\n")) {
        warnings.push("SKILL.md has no YAML frontmatter".to_string());
        invalid_metadata = true;
    }
    if !body
        .lines()
        .any(|line| line.trim_start().starts_with("name:"))
    {
        warnings.push("frontmatter has no name field".to_string());
        invalid_metadata = true;
    }
    if !body
        .lines()
        .any(|line| line.trim_start().starts_with("description:"))
    {
        warnings.push("frontmatter has no description field".to_string());
        invalid_metadata = true;
    }

    let claude_only = ["AskUserQuestion", "EnterPlanMode", "ExitPlanMode"]
        .iter()
        .any(|needle| body.contains(needle));
    let codex_only = ["request_user_input", "functions.exec", "apply_patch tool"]
        .iter()
        .any(|needle| body.contains(needle));
    if claude_only {
        warnings.push("references Claude Code-specific tools".to_string());
    }
    if codex_only {
        warnings.push("references Codex-specific tools".to_string());
    }
    let level = if invalid_metadata {
        SkillCompatibility::Incompatible
    } else {
        match (claude_only, codex_only) {
            (false, false) => SkillCompatibility::Both,
            (true, false) => SkillCompatibility::ClaudeOnly,
            (false, true) => SkillCompatibility::CodexOnly,
            (true, true) => SkillCompatibility::Incompatible,
        }
    };
    (level, warnings)
}

fn collect_files(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("skill tree exceeds maximum depth {MAX_DEPTH}"));
    }
    let mut entries = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.file_name().and_then(|v| v.to_str()) == Some(COPY_MARKER) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "skill contains unsupported symlink: {}",
                path.display()
            ));
        }
        if meta.is_dir() {
            collect_files(root, &path, depth + 1, out)?;
        } else if meta.is_file() {
            out.push(
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn content_hash(root: &Path) -> Result<String, String> {
    if root.is_file() {
        let mut hasher = Sha256::new();
        hasher.update(SKILL_FILE.as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(root).map_err(|e| e.to_string())?);
        hasher.update([0]);
        return Ok(format!("{:x}", hasher.finalize()));
    }
    let mut files = Vec::new();
    collect_files(root, root, 0, &mut files)?;
    let mut hasher = Sha256::new();
    for rel in files {
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(root.join(&rel)).map_err(|e| e.to_string())?);
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    if source.is_file() {
        fs::copy(source, destination.join(SKILL_FILE)).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(source, source, 0, &mut files)?;
    for rel in files {
        let to = destination.join(&rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(source.join(&rel), &to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn points_to(path: &Path, canonical: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return fs::canonicalize(path).ok() == fs::canonicalize(canonical).ok();
    }
    let marker = path.join(COPY_MARKER);
    let Ok(body) = fs::read_to_string(marker) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<CopyMarker>(&body) else {
        return false;
    };
    fs::canonicalize(marker.canonical_path).ok() == fs::canonicalize(canonical).ok()
        && content_hash(path).ok().as_deref() == Some(marker.content_hash.as_str())
}

#[cfg(unix)]
fn symlink_dir(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn symlink_dir(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, destination)
}

fn create_projection(canonical: &Path, destination: &Path) -> Result<String, String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match symlink_dir(canonical, destination) {
        Ok(()) => Ok("linked".to_string()),
        Err(_) => {
            let file_name = destination
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| "invalid projection destination".to_string())?;
            let staging = destination.with_file_name(format!(
                ".{file_name}.fleet-copy-staging-{}",
                std::process::id()
            ));
            if fs::symlink_metadata(&staging).is_ok() {
                return Err(format!(
                    "projection staging path already exists: {}",
                    staging.display()
                ));
            }
            if let Err(error) = copy_tree(canonical, &staging) {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
            let marker = CopyMarker {
                canonical_path: fs::canonicalize(canonical)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .to_string(),
                content_hash: content_hash(canonical)?,
            };
            fs::write(
                staging.join(COPY_MARKER),
                serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if let Err(error) = fs::rename(&staging, destination) {
                let _ = fs::remove_dir_all(&staging);
                return Err(error.to_string());
            }
            Ok("copied".to_string())
        }
    }
}

fn project_one(
    roots: &Roots,
    slug: &str,
    target: SkillTarget,
    apply: bool,
) -> Result<Option<SkillSyncAction>, String> {
    let canonical = roots.canonical.join(slug);
    let destination = target_path(roots, slug, target);
    if points_to(&destination, &canonical) {
        return Ok(None);
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(format!(
            "{} already has an unmanaged skill '{}': {}",
            target.label(),
            slug,
            destination.display()
        ));
    }
    let action = if apply {
        create_projection(&canonical, &destination)?
    } else {
        "would-link".to_string()
    };
    Ok(Some(SkillSyncAction {
        slug: slug.to_string(),
        target: target.label().to_string(),
        action,
        path: destination.to_string_lossy().to_string(),
    }))
}

/// Return a merged view of Fleet-managed and runtime-native user skills.
pub fn inventory() -> Result<Vec<SkillSyncEntry>, String> {
    let roots = roots()?;
    let canonical = scan_skill_dirs(&roots.canonical, false);
    let claude = scan_skill_dirs(&roots.claude, true);
    let codex = scan_skill_dirs(&roots.codex, false);
    let slugs: BTreeSet<_> = canonical
        .keys()
        .chain(claude.keys())
        .chain(codex.keys())
        .cloned()
        .collect();
    let mut entries = Vec::new();
    for slug in slugs {
        let canonical_path = canonical.get(&slug);
        let claude_path = claude.get(&slug);
        let codex_path = codex.get(&slug);
        let claude_managed = canonical_path
            .zip(claude_path)
            .is_some_and(|(canonical, path)| points_to(path, canonical));
        let codex_managed = canonical_path
            .zip(codex_path)
            .is_some_and(|(canonical, path)| points_to(path, canonical));
        let state = if canonical_path.is_none() {
            SkillSyncState::Unmanaged
        } else if claude_managed && codex_managed {
            SkillSyncState::Shared
        } else if (claude_path.is_some() && !claude_managed)
            || (codex_path.is_some() && !codex_managed)
        {
            SkillSyncState::Conflict
        } else {
            SkillSyncState::Partial
        };
        let inspect = canonical_path
            .or(claude_path)
            .or(codex_path)
            .expect("slug came from one inventory");
        let (compatibility, warnings) = compatibility(inspect);
        entries.push(SkillSyncEntry {
            slug,
            state,
            compatibility,
            warnings,
            canonical_path: canonical_path.map(|p| p.to_string_lossy().to_string()),
            claude_path: claude_path.map(|p| p.to_string_lossy().to_string()),
            codex_path: codex_path.map(|p| p.to_string_lossy().to_string()),
            claude_managed,
            codex_managed,
        });
    }
    Ok(entries)
}

/// Preview or apply projections for every Fleet-managed skill.
pub fn sync(apply: bool) -> Result<SkillSyncReport, String> {
    let roots = roots()?;
    let canonical = scan_skill_dirs(&roots.canonical, false);
    let mut report = SkillSyncReport::default();
    for slug in canonical.keys() {
        for target in [SkillTarget::ClaudeCode, SkillTarget::Codex] {
            match project_one(&roots, slug, target, apply) {
                Ok(Some(action)) => report.actions.push(action),
                Ok(None) => {}
                Err(conflict) => report.conflicts.push(conflict),
            }
        }
    }
    report.items = inventory()?;
    Ok(report)
}

fn source_location(path: &Path, roots: &Roots) -> Result<(String, PathBuf), String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("canonicalize {}: {e}", path.display()))?;
    let candidates = [(&roots.claude, true), (&roots.codex, false)];
    for (root, allow_flat) in candidates {
        let Ok(canonical_root) = root.canonicalize() else {
            continue;
        };
        if !path.starts_with(&canonical_root) {
            continue;
        }
        let relative = path
            .strip_prefix(&canonical_root)
            .map_err(|e| e.to_string())?;
        let first = relative
            .components()
            .next()
            .ok_or_else(|| "skill path points at the discovery root".to_string())?;
        let logical = root.join(first.as_os_str());
        if logical.is_file() {
            if !allow_flat || logical.extension().and_then(|v| v.to_str()) != Some("md") {
                return Err("only Claude flat .md skills can be adopted".to_string());
            }
            let slug = logical
                .file_stem()
                .and_then(|v| v.to_str())
                .ok_or_else(|| "invalid skill filename".to_string())?;
            return Ok((slug.to_string(), logical));
        }
        if !logical.join(SKILL_FILE).is_file() {
            return Err("skill directory has no SKILL.md".to_string());
        }
        let slug = logical
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| "invalid skill directory".to_string())?;
        return Ok((slug.to_string(), logical));
    }
    Err("only global Claude or Codex user skills can be adopted".to_string())
}

fn replace_source_with_projection(
    source: &Path,
    destination: &Path,
    canonical: &Path,
) -> Result<(), String> {
    if source != destination && fs::symlink_metadata(destination).is_ok() {
        return Err(format!(
            "projection destination already exists: {}",
            destination.display()
        ));
    }
    let backup = source.with_extension(format!("fleet-adopt-backup-{}", std::process::id()));
    if fs::symlink_metadata(&backup).is_ok() {
        return Err(format!(
            "temporary backup already exists: {}",
            backup.display()
        ));
    }
    fs::rename(source, &backup).map_err(|e| e.to_string())?;
    let result = create_projection(canonical, destination);
    match result {
        Ok(_) => {
            if backup.is_dir() {
                fs::remove_dir_all(&backup).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(&backup).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Err(error) => {
            let _ = if destination.is_dir() {
                fs::remove_dir_all(destination)
            } else {
                fs::remove_file(destination)
            };
            fs::rename(&backup, source).map_err(|restore| {
                format!("{error}; additionally failed to restore source: {restore}")
            })?;
            Err(error)
        }
    }
}

/// Adopt one existing global user skill and project it to both runtimes.
pub fn adopt(path: &Path) -> Result<SkillSyncReport, String> {
    let roots = roots()?;
    let (slug, source) = source_location(path, &roots)?;
    if !valid_slug(&slug) {
        return Err(format!("invalid skill slug: {slug}"));
    }
    let canonical = roots.canonical.join(&slug);
    fs::create_dir_all(&roots.canonical).map_err(|e| e.to_string())?;
    if canonical.exists() {
        if content_hash(&canonical)? != content_hash(&source)? {
            return Err(format!(
                "Fleet already manages different content for '{slug}'"
            ));
        }
    } else {
        let staging = roots
            .canonical
            .join(format!(".{slug}.staging-{}", std::process::id()));
        if let Err(error) = copy_tree(&source, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        fs::rename(&staging, &canonical).map_err(|e| e.to_string())?;
    }

    let source_target = if source.starts_with(&roots.claude) {
        SkillTarget::ClaudeCode
    } else {
        SkillTarget::Codex
    };
    let destination = target_path(&roots, &slug, source_target);
    if !points_to(&destination, &canonical) {
        replace_source_with_projection(&source, &destination, &canonical)?;
    }
    sync(true)
}

/// Remove one managed runtime projection without touching the canonical copy.
pub fn unlink(slug: &str, target: SkillTarget) -> Result<SkillSyncAction, String> {
    if !valid_slug(slug) {
        return Err(format!("invalid skill slug: {slug}"));
    }
    let roots = roots()?;
    let canonical = roots.canonical.join(slug);
    let path = target_path(&roots, slug, target);
    if !points_to(&path, &canonical) {
        return Err("refusing to remove an unmanaged skill".to_string());
    }
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    } else {
        fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    }
    Ok(SkillSyncAction {
        slug: slug.to_string(),
        target: target.label().to_string(),
        action: "unlinked".to_string(),
        path: path.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn new(path: &Path) -> Self {
            let old = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", path) };
            Self(old)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(old) => std::env::set_var("FLEET_HOME", old),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    fn write_skill(root: &Path, slug: &str, body: &str) -> PathBuf {
        let dir = root.join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKILL_FILE), body).unwrap();
        dir
    }

    #[test]
    fn adopt_projects_to_both_runtimes() {
        let _lock = crate::session::fleet_home_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new(temp.path());
        let source = write_skill(
            &temp.path().join(".claude/skills"),
            "portable",
            "---\nname: portable\ndescription: Works everywhere\n---\nBody",
        );
        let report = adopt(&source.join(SKILL_FILE)).unwrap();
        let item = report
            .items
            .iter()
            .find(|item| item.slug == "portable")
            .unwrap();
        assert_eq!(item.state, SkillSyncState::Shared);
        assert!(item.claude_managed);
        assert!(item.codex_managed);
        assert!(temp
            .path()
            .join(".fleet/skills/portable/SKILL.md")
            .is_file());
    }

    #[test]
    fn sync_reports_conflict_without_overwriting() {
        let _lock = crate::session::fleet_home_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new(temp.path());
        write_skill(
            &temp.path().join(".fleet/skills"),
            "same-name",
            "---\nname: same-name\ndescription: canonical\n---\n",
        );
        let foreign = write_skill(
            &temp.path().join(".agents/skills"),
            "same-name",
            "---\nname: same-name\ndescription: foreign\n---\n",
        );
        let report = sync(true).unwrap();
        assert_eq!(report.conflicts.len(), 1);
        assert!(fs::read_to_string(foreign.join(SKILL_FILE))
            .unwrap()
            .contains("foreign"));
        assert!(temp
            .path()
            .join(".claude/skills/same-name/SKILL.md")
            .is_file());
    }

    #[test]
    fn unlink_refuses_unmanaged_content() {
        let _lock = crate::session::fleet_home_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new(temp.path());
        write_skill(
            &temp.path().join(".agents/skills"),
            "foreign",
            "---\nname: foreign\ndescription: foreign\n---\n",
        );
        let error = unlink("foreign", SkillTarget::Codex).unwrap_err();
        assert!(error.contains("unmanaged"));
        assert!(temp
            .path()
            .join(".agents/skills/foreign/SKILL.md")
            .is_file());
    }

    #[test]
    fn compatibility_marks_runtime_specific_tools() {
        let temp = tempfile::tempdir().unwrap();
        let claude = write_skill(
            temp.path(),
            "claude",
            "---\nname: claude\ndescription: x\n---\nUse AskUserQuestion",
        );
        assert_eq!(compatibility(&claude).0, SkillCompatibility::ClaudeOnly);
        let codex = write_skill(
            temp.path(),
            "codex",
            "---\nname: codex\ndescription: x\n---\nUse request_user_input",
        );
        assert_eq!(compatibility(&codex).0, SkillCompatibility::CodexOnly);
    }

    #[test]
    fn adopt_normalizes_a_flat_claude_skill() {
        let _lock = crate::session::fleet_home_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new(temp.path());
        let flat = temp.path().join(".claude/skills/flat.md");
        fs::create_dir_all(flat.parent().unwrap()).unwrap();
        fs::write(&flat, "---\nname: flat\ndescription: flat skill\n---\nBody").unwrap();

        let report = adopt(&flat).unwrap();
        let item = report
            .items
            .iter()
            .find(|item| item.slug == "flat")
            .unwrap();
        assert_eq!(item.state, SkillSyncState::Shared);
        assert!(!flat.exists());
        assert!(temp.path().join(".claude/skills/flat/SKILL.md").is_file());
        assert!(temp.path().join(".agents/skills/flat/SKILL.md").is_file());
    }
}
