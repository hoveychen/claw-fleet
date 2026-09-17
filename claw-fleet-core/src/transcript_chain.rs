//! Heal a Claude Code transcript whose `parentUuid` chain has a hole in it.
//!
//! Claude Code reconstructs a resumed session's message history by walking
//! `parentUuid` backwards from the transcript's tail. When a link points at a
//! uuid that is not in the file, the walk stops there and everything older is
//! silently dropped — `claude --resume <id> -p continue` then starts the turn
//! with an essentially empty history while still appending to the same JSONL,
//! so from the outside it looks like a *different* session woke up.
//!
//! That is not hypothetical. On 2026-09-17 session `eeb14eae` lost 149K tokens
//! of context this way: the model emitted a tool call the CLI could not parse,
//! the failed `assistant` row was never written to the JSONL, but the retry
//! prompt the CLI injected right after it still carried that never-persisted
//! row's uuid as its `parentUuid`. The hole is permanent — every later resume
//! re-reads the same broken file — and the amnesiac agent, left with only
//! Fleet's `TASKS.md` injection for context, read a bare "continue" as
//! "continue that plan" and went to work on an unrelated task.
//!
//! The repair is deliberately the smallest one that can be right: a dangling
//! `parentUuid` is re-pointed at the nearest preceding row that has a `uuid`.
//! The missing row really is gone (it was never persisted), so closing the gap
//! is what the CLI itself would have written had the row landed. Every other
//! byte of every line is preserved verbatim — the file is rewritten from the
//! original strings, not re-serialised from parsed JSON.

use std::path::{Path, PathBuf};

/// What [`repair_dangling_parents`] did to one transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainRepair {
    /// Number of rows whose `parentUuid` was re-pointed.
    pub repaired: usize,
    /// Where the untouched original was copied before rewriting.
    pub backup: Option<PathBuf>,
}

/// Directory holding the pre-repair copies, under Fleet's own state dir so a
/// repair never litters the user's `~/.claude/projects/`. `None` when Fleet has
/// no state dir at all, which is the one case where we skip the backup rather
/// than refuse the repair.
fn backup_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("transcript-chain-backups"))
}

/// Re-point every dangling `parentUuid` in `path` at the nearest preceding row
/// that has a `uuid`. Returns `None` when the file is already whole (the
/// overwhelmingly common case — nothing is read a second time and nothing is
/// written).
///
/// Only ever call this for a session that is *not* currently running: the
/// rewrite is a whole-file replace, and a concurrent append would be lost. The
/// length re-check just before the rename turns that race into a no-op repair
/// rather than a truncated transcript, but it cannot make a live rewrite safe.
pub fn repair_dangling_parents(path: &Path) -> Result<Option<ChainRepair>, String> {
    repair_into(path, backup_dir())
}

fn repair_into(path: &Path, backups: Option<PathBuf>) -> Result<Option<ChainRepair>, String> {
    let original = std::fs::read_to_string(path)
        .map_err(|e| format!("read transcript {}: {}", path.display(), e))?;

    let lines: Vec<&str> = original.lines().collect();
    let parsed: Vec<Option<serde_json::Value>> = lines
        .iter()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect();

    let uuids: std::collections::HashSet<&str> = parsed
        .iter()
        .filter_map(|v| v.as_ref()?.get("uuid")?.as_str())
        .collect();

    // `fixes[i] = Some(uuid)` — row i's parentUuid must become that uuid.
    let mut fixes: Vec<Option<String>> = vec![None; lines.len()];
    let mut last_uuid: Option<&str> = None;
    for (i, value) in parsed.iter().enumerate() {
        let Some(v) = value else { continue };
        if let Some(parent) = v.get("parentUuid").and_then(|p| p.as_str()) {
            // Nothing precedes it — leave the row alone rather than invent a
            // root; a headless resume of a file that starts mid-chain has
            // nothing to recover anyway.
            if !uuids.contains(parent) {
                if let Some(anchor) = last_uuid {
                    fixes[i] = Some(anchor.to_string());
                }
            }
        }
        if let Some(u) = v.get("uuid").and_then(|u| u.as_str()) {
            last_uuid = Some(u);
        }
    }

    let repaired = fixes.iter().filter(|f| f.is_some()).count();
    if repaired == 0 {
        return Ok(None);
    }

    let backup = match backups {
        Some(dir) => write_backup(&dir, path, &original)?,
        None => None,
    };

    let mut out = String::with_capacity(original.len());
    for (i, line) in lines.iter().enumerate() {
        match &fixes[i] {
            // Re-serialising the parsed value is safe here *only* because this
            // row is being changed anyway; untouched rows keep their exact
            // bytes.
            Some(anchor) => {
                let mut v = parsed[i].clone().expect("only parsed rows get fixes");
                v["parentUuid"] = serde_json::Value::String(anchor.clone());
                out.push_str(&serde_json::to_string(&v).map_err(|e| e.to_string())?);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }

    // The file grew while we were working — another process is appending, so
    // this session is not the dead one the caller took it for. Abandon the
    // repair rather than truncate a live transcript.
    let now_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if now_len != original.len() as u64 {
        return Err(format!(
            "transcript {} changed under us ({} → {} bytes) — repair abandoned",
            path.display(),
            original.len(),
            now_len
        ));
    }

    let tmp = path.with_extension("jsonl.chain-repair");
    std::fs::write(&tmp, out.as_bytes()).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} -> {}: {}", tmp.display(), path.display(), e))?;

    Ok(Some(ChainRepair { repaired, backup }))
}

fn write_backup(dir: &Path, path: &Path, original: &str) -> Result<Option<PathBuf>, String> {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Err(format!("create {}: {}", dir.display(), e));
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcript");
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let dest = dir.join(format!("{stem}-{stamp}.jsonl"));
    std::fs::write(&dest, original.as_bytes())
        .map_err(|e| format!("write backup {}: {}", dest.display(), e))?;
    Ok(Some(dest))
}

/// Locate `session_id`'s transcript and repair it. Returns `Ok(None)` when the
/// transcript is whole or cannot be found — a missing transcript is not an
/// error here, it just means there is nothing to resume from on this machine.
pub fn repair_session(session_id: &str) -> Result<Option<ChainRepair>, String> {
    let Some(path) = crate::session::find_session_jsonl(session_id) else {
        return Ok(None);
    };
    repair_dangling_parents(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(uuid: &str, parent: Option<&str>, kind: &str) -> String {
        let parent = match parent {
            Some(p) => format!("\"{p}\""),
            None => "null".to_string(),
        };
        format!(r#"{{"parentUuid":{parent},"uuid":"{uuid}","type":"{kind}"}}"#)
    }

    fn write(dir: &Path, lines: &[String]) -> PathBuf {
        let p = dir.join("s.jsonl");
        std::fs::write(&p, format!("{}\n", lines.join("\n"))).unwrap();
        p
    }

    #[test]
    fn whole_chain_is_left_untouched() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            &[
                row("a", None, "user"),
                row("b", Some("a"), "assistant"),
                row("c", Some("b"), "user"),
            ],
        );
        let before = std::fs::read_to_string(&p).unwrap();
        assert_eq!(repair_into(&p, Some(d.path().join("bk"))).unwrap(), None);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
    }

    /// The 2026-09-17 `eeb14eae` shape: a retry prompt whose parent — the
    /// assistant row with the unparseable tool call — was never persisted.
    #[test]
    fn dangling_parent_is_relinked_to_the_preceding_row() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            &[
                row("a", None, "user"),
                row("b", Some("a"), "assistant"),
                row("c", Some("never-written"), "user"),
                row("e", Some("c"), "assistant"),
            ],
        );
        let r = repair_into(&p, Some(d.path().join("bk"))).unwrap().unwrap();
        assert_eq!(r.repaired, 1);

        let healed: Vec<serde_json::Value> = std::fs::read_to_string(&p)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(healed.len(), 4);
        assert_eq!(healed[2]["parentUuid"], "b");
        // Everything else is preserved, so the tail still walks to the root.
        assert_eq!(healed[3]["parentUuid"], "c");
        assert_eq!(healed[0]["parentUuid"], serde_json::Value::Null);

        let backup = std::fs::read_to_string(r.backup.unwrap()).unwrap();
        assert!(backup.contains("never-written"));
    }

    /// Rows with no `uuid` at all (`queue-operation`, `last-prompt`, `mode`)
    /// sit between real ones in a live transcript; the anchor must skip them.
    #[test]
    fn uuidless_rows_are_not_used_as_anchors() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            &[
                row("a", None, "user"),
                r#"{"type":"queue-operation"}"#.to_string(),
                row("c", Some("gone"), "user"),
            ],
        );
        assert_eq!(
            repair_into(&p, Some(d.path().join("bk")))
                .unwrap()
                .unwrap()
                .repaired,
            1
        );
        let last: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&p).unwrap().lines().last().unwrap())
                .unwrap();
        assert_eq!(last["parentUuid"], "a");
    }

    /// A dangle with nothing before it cannot be anchored; leave it be rather
    /// than fabricate a root.
    #[test]
    fn dangle_with_no_preceding_uuid_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), &[row("a", Some("gone"), "user")]);
        assert_eq!(repair_into(&p, Some(d.path().join("bk"))).unwrap(), None);
    }

    /// Unparseable lines must survive byte-for-byte — a transcript can carry a
    /// truncated tail line from a killed process.
    #[test]
    fn unparseable_lines_are_preserved() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            &[
                row("a", None, "user"),
                row("b", Some("gone"), "assistant"),
                r#"{"truncated":"#.to_string(),
            ],
        );
        assert_eq!(
            repair_into(&p, Some(d.path().join("bk")))
                .unwrap()
                .unwrap()
                .repaired,
            1
        );
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains(r#"{"truncated":"#));
    }
}
