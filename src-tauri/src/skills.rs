//! Skills scanning — reads Claude Code skill files from `~/.claude/skills/`.
//!
//! Supports two layouts:
//!   • Directory-based: `~/.claude/skills/<name>/SKILL.md`
//!   • Flat file:       `~/.claude/skills/<name>.md`
//!
//! Name and description are extracted from YAML frontmatter when present.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::session::get_claude_dir;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillItem {
    /// Skill name — from frontmatter `name:` or the directory/file stem.
    pub name: String,
    /// Short description — from frontmatter `description:` or empty string.
    pub description: String,
    /// Absolute path to the skill file (`SKILL.md` or `<name>.md`).
    pub path: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
}

// ── Scan ──────────────────────────────────────────────────────────────────────

pub fn scan_all_skills() -> Vec<SkillItem> {
    let Some(claude_dir) = get_claude_dir() else {
        return vec![];
    };
    let skills_dir = claude_dir.join("skills");
    if !skills_dir.is_dir() {
        return vec![];
    }

    let Ok(entries) = fs::read_dir(&skills_dir) else {
        return vec![];
    };

    let mut results = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Directory-based skill: <name>/SKILL.md
            let skill_file = path.join("SKILL.md");
            if skill_file.is_file() {
                if let Some(item) = read_skill_item(&skill_file, &path) {
                    results.push(item);
                }
            }
        } else if path.is_file() {
            // Flat skill file: <name>.md
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Some(item) = read_skill_item(&path, &path) {
                    results.push(item);
                }
            }
        }
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
}

fn read_skill_item(skill_file: &Path, name_source: &Path) -> Option<SkillItem> {
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
        name,
        description,
        path: skill_file.to_string_lossy().to_string(),
        size_bytes,
        modified_ms,
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

// ── Read file content ─────────────────────────────────────────────────────────

pub fn read_skill_file(path: &str) -> Result<String, String> {
    // Safety: only allow reading from ~/.claude/skills/
    let claude_dir = get_claude_dir().ok_or("cannot determine home dir")?;
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let skills_dir = claude_dir.join("skills");

    let allowed = fs::canonicalize(&skills_dir)
        .map(|s| canonical.starts_with(s))
        .unwrap_or(false);

    if !allowed {
        return Err("path is outside allowed skills directory".into());
    }

    fs::read_to_string(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_extracts_name_and_description() {
        let content = "---\nname: fleet\ndescription: Monitor agents\nallowed-tools: Bash\n---\n\n# Body";
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
}
