//! Real-data replay for the prd-context dedupe.
//!
//! Ignored by default — it reads THIS machine's `~/.claude/projects` transcripts
//! and `~/.codex/sessions` rollouts:
//!
//! ```text
//! cargo test -p claw-fleet-core --test prd_context_dedup_realdata -- --ignored --nocapture
//! ```
//!
//! The unit tests build synthetic logs; this one proves the probes agree with
//! the shapes Claude Code and Codex actually write. For every log that carries
//! an injected plan reminder it replays the last copy found there and asserts
//! the probe would (a) skip that exact text and (b) still inject a changed one.

use std::path::{Path, PathBuf};

use claw_fleet_core::prd_context_dedup::{claude_needs_injection, codex_needs_injection};

const MARK: &str = "re-injected on every prompt by Fleet PRD Discipline mode";

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(jsonl_files(&p));
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
    out
}

/// The newest injected reminder in a log, by the same rule the probes use:
/// walk backwards, a compaction boundary ends the search, and only the last
/// [`TAIL_BYTES`] count — a copy older than the probes' window is a deliberate
/// miss (a wasted injection, never a missing one), not a disagreement.
fn last_reminder(path: &Path, claude: bool) -> Option<String> {
    let text = tail(path)?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if claude {
            if v.get("isCompactSummary").and_then(|x| x.as_bool()) == Some(true)
                || v.get("subtype").and_then(|x| x.as_str()) == Some("compact_boundary")
            {
                return None;
            }
            if v.get("isSidechain").and_then(|x| x.as_bool()) == Some(true) {
                continue;
            }
            let joined: String = v
                .pointer("/attachment/content")
                .and_then(|c| c.as_array())
                .map(|parts| parts.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            // A reminder over Claude Code's inline limit is stored out of line
            // and the attachment holds a `<persisted-output>` pointer instead
            // of the text. Nothing to compare against, so the probes inject —
            // don't count those logs as replays.
            if joined.contains(MARK) && joined.trim_start().starts_with("<system-reminder>") {
                return Some(joined);
            }
        } else {
            if v.get("type").and_then(|x| x.as_str()) == Some("compacted") {
                return None;
            }
            if v.pointer("/payload/role").and_then(|x| x.as_str()) != Some("user") {
                continue;
            }
            let joined: String = v
                .pointer("/payload/content")
                .and_then(|c| c.as_array())
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                        .collect()
                })
                .unwrap_or_default();
            if !joined.contains(MARK) {
                continue;
            }
            let Some(end) = joined.find(CLOSE).map(|i| i + CLOSE.len()) else {
                continue;
            };
            return Some(joined[..end].to_string());
        }
    }
    None
}

const CLOSE: &str = "</system-reminder>";
/// Must match `prd_context_dedup::TAIL_BYTES`.
const TAIL_BYTES: u64 = 1024 * 1024;

fn tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    if start > 0 {
        s = s.split_once('\n')?.1.to_string();
    }
    Some(s)
}

fn replay(files: &[PathBuf], claude: bool) -> usize {
    let mut checked = 0;
    for f in files {
        let Some(reminder) = last_reminder(f, claude) else {
            continue;
        };
        let probe = if claude {
            claude_needs_injection
        } else {
            codex_needs_injection
        };
        assert!(
            !probe(f, &reminder),
            "identical reminder must be recognised in {}",
            f.display()
        );
        let changed = format!("{reminder}\n(changed)");
        assert!(
            probe(f, &changed),
            "changed reminder must still inject in {}",
            f.display()
        );
        checked += 1;
    }
    checked
}

#[test]
#[ignore = "reads this machine's real transcripts/rollouts; run with --ignored"]
fn probes_agree_with_real_logs() {
    let home = claw_fleet_core::session::real_home_dir().expect("home dir");

    let claude_files = jsonl_files(&home.join(".claude/projects"));
    let n_claude = replay(&claude_files, true);
    eprintln!(
        "claude: {n_claude} logs replayed out of {} scanned",
        claude_files.len()
    );

    let codex_files = jsonl_files(&home.join(".codex/sessions"));
    let n_codex = replay(&codex_files, false);
    eprintln!(
        "codex: {n_codex} logs replayed out of {} scanned",
        codex_files.len()
    );

    assert!(
        n_claude > 0 && n_codex > 0,
        "no real injected reminders found — this machine cannot exercise the replay"
    );
}
