//! Full-text search index for session content using SQLite FTS5.
//!
//! Tracks each JSONL file by `(mtime_ms, byte_offset)` so that:
//! - Unchanged files are skipped entirely (no duplicate indexing).
//! - Appended content is indexed incrementally from the last byte offset.
//! - Truncated/rewritten files are fully re-indexed.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::log_debug;

/// Bumped whenever the set of indexed fields changes. On open, a database
/// stamped with an older version has its FTS content + per-file offsets wiped
/// so the next scan fully re-indexes every session under the new rules.
///
/// History:
///   1 — initial (user/assistant message bodies only)
///   2 — also index the `ai-title` line's `aiTitle` field
///   3 — CJK text is stored segmented per character (see [`segment_cjk`]);
///       rows written under v2 hold unsegmented runs and can never match a
///       Chinese substring query, so they must be rebuilt.
///   4 — added the `raw` column holding the author's untouched text, which
///       snippets are cut from; v3 rows have no `raw` to cut.
///   5 — added the `line_no` column (1-based line of the record in its jsonl)
///       so a hit can be read back as one item (`search_scoped` / the
///       `fleet__history` tool); v4 rows cannot be located.
const SCHEMA_VERSION: i64 = 5;

// ── SearchHit ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub session_id: String,
    pub jsonl_path: String,
    pub snippet: String,
    pub rank: f64,
}

/// One matching *record* from [`SearchIndex::search_scoped`]: unlike
/// [`SearchHit`] it is not collapsed per session and carries the record's line
/// so the caller can read the whole item back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScopedHit {
    pub session_id: String,
    pub jsonl_path: String,
    /// 1-based line number of the record within `jsonl_path`.
    pub line_no: i64,
    pub snippet: String,
    pub rank: f64,
}

// ── SearchIndex ──────────────────────────────────────────────────────────────

pub struct SearchIndex {
    conn: Connection,
}

impl SearchIndex {
    /// Open (or create) the search database at `~/.fleet/fleet-search.db`.
    pub fn open() -> Result<Self, String> {
        let db_path = crate::session::real_home_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?
            .join(".fleet")
            .join("fleet-search.db");
        Self::open_at(&db_path)
    }

    /// Open (or create) the search database at a custom path.
    pub fn open_at(db_path: &std::path::Path) -> Result<Self, String> {
        // Ensure parent dir exists.
        if let Some(parent) = db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let conn = Connection::open(db_path).map_err(|e| format!("sqlite open: {e}"))?;

        // Pragmas for performance.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| format!("sqlite pragma: {e}"))?;

        // Migration runs BEFORE the CREATEs. `CREATE TABLE IF NOT EXISTS` is a
        // no-op against an existing table, so it can never add a column to one
        // — a v3 database would keep its 3-column `session_fts` and every
        // 4-column INSERT would fail. Dropping first is what actually lets the
        // shape change between versions; clearing rows alone is not enough.
        let db_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if db_version < SCHEMA_VERSION {
            conn.execute_batch(
                "DROP TABLE IF EXISTS session_fts;
                 DROP TABLE IF EXISTS index_meta;",
            )
            .map_err(|e| format!("sqlite migrate drop: {e}"))?;
        }

        // Create schema if missing.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS index_meta (
                 jsonl_path  TEXT PRIMARY KEY,
                 session_id  TEXT NOT NULL,
                 mtime_ms    INTEGER NOT NULL,
                 byte_offset INTEGER NOT NULL,
                 line_count  INTEGER NOT NULL DEFAULT 0
             );

             CREATE VIRTUAL TABLE IF NOT EXISTS session_fts USING fts5(
                 session_id UNINDEXED,
                 jsonl_path UNINDEXED,
                 content,
                 raw UNINDEXED,
                 line_no UNINDEXED,
                 tokenize='unicode61'
             );",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;

        if db_version < SCHEMA_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
                .map_err(|e| format!("sqlite migrate stamp: {e}"))?;
        }

        Ok(Self { conn })
    }

    /// Index a single session JSONL file incrementally.
    ///
    /// - If the file hasn't changed (same mtime and size), does nothing.
    /// - If the file has grown, reads only the new bytes from the last offset.
    /// - If the file was truncated, deletes old rows and re-indexes from scratch.
    pub fn index_session(&self, jsonl_path: &str, session_id: &str) -> Result<(), String> {
        // Skip non-file paths (e.g. cursor:// URIs).
        if jsonl_path.contains("://") {
            return Ok(());
        }

        let meta = fs::metadata(jsonl_path).map_err(|e| format!("stat {jsonl_path}: {e}"))?;
        let file_size = meta.len() as i64;
        let mtime_ms = meta
            .modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Look up what we've already indexed.
        let existing: Option<(i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT mtime_ms, byte_offset, line_count FROM index_meta WHERE jsonl_path = ?1",
                params![jsonl_path],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();

        let (start_offset, start_line) = match existing {
            Some((saved_mtime, saved_offset, saved_lines)) => {
                // Nothing changed — skip.
                if saved_mtime == mtime_ms && saved_offset == file_size {
                    return Ok(());
                }
                // File was truncated/rewritten — purge and re-index.
                if file_size < saved_offset {
                    self.remove_session(jsonl_path)?;
                    (0i64, 0i64)
                } else {
                    // File grew — read from where we left off.
                    (saved_offset, saved_lines)
                }
            }
            // Brand-new file.
            None => (0, 0),
        };

        // Read new content.
        let mut file =
            fs::File::open(jsonl_path).map_err(|e| format!("open {jsonl_path}: {e}"))?;
        if start_offset > 0 {
            file.seek(SeekFrom::Start(start_offset as u64))
                .map_err(|e| format!("seek {jsonl_path}: {e}"))?;
        }

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("read {jsonl_path}: {e}"))?;

        // Consume only up to the last newline. We race the CLI's writes, so the
        // tail of a growing transcript is routinely a half-flushed record.
        // Reading to EOF would hand that fragment to serde_json, fail, and still
        // save an offset past it — the record's first half would never be re-read
        // and the line would stay unindexed forever. Leaving the fragment behind
        // lets the next pass index it once it is complete.
        let consumed = match buf.iter().rposition(|b| *b == b'\n') {
            Some(i) => i + 1,
            None => 0,
        };
        let chunk = String::from_utf8_lossy(&buf[..consumed]);

        let mut new_lines = 0i64;

        let tx = self.conn.unchecked_transaction().map_err(|e| format!("tx: {e}"))?;

        for line in chunk.lines() {
            if line.trim().is_empty() {
                new_lines += 1;
                continue;
            }

            let parsed: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => {
                    new_lines += 1;
                    continue;
                }
            };

            let text = extract_searchable_text(&parsed);
            if !text.is_empty() {
                // `content` is segmented so `unicode61` tokenizes CJK per
                // character; `raw` keeps the author's exact text so snippets
                // can be cut from it verbatim (see `build_snippet`).
                let segmented = segment_cjk(&text);
                // `new_lines` counts the lines already folded from this chunk,
                // so the current record sits at start_line + new_lines (0-based)
                // — +1 for the 1-based number a reader seeks to.
                let line_no = start_line + new_lines + 1;
                tx.execute(
                    "INSERT INTO session_fts(session_id, jsonl_path, content, raw, line_no)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session_id, jsonl_path, segmented, text, line_no],
                )
                .map_err(|e| format!("fts insert: {e}"))?;
            }

            new_lines += 1;
        }

        // Update metadata.
        tx.execute(
            "INSERT OR REPLACE INTO index_meta(jsonl_path, session_id, mtime_ms, byte_offset, line_count)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                jsonl_path,
                session_id,
                mtime_ms,
                // What we actually folded — NOT file_size. A half-flushed tail
                // record leaves this short of the file end on purpose, so the
                // next pass re-reads it once it is terminated.
                start_offset + consumed as i64,
                start_line + new_lines,
            ],
        )
        .map_err(|e| format!("meta upsert: {e}"))?;

        tx.commit().map_err(|e| format!("commit: {e}"))?;

        Ok(())
    }

    /// Remove all indexed data for a session.
    pub fn remove_session(&self, jsonl_path: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM session_fts WHERE jsonl_path = ?1",
                params![jsonl_path],
            )
            .map_err(|e| format!("fts delete: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM index_meta WHERE jsonl_path = ?1",
                params![jsonl_path],
            )
            .map_err(|e| format!("meta delete: {e}"))?;
        Ok(())
    }

    /// Remove index entries for sessions that no longer exist on disk.
    pub fn cleanup_stale(&self, live_paths: &HashSet<String>) -> Result<(), String> {
        let mut stmt = self
            .conn
            .prepare("SELECT jsonl_path FROM index_meta")
            .map_err(|e| format!("prepare: {e}"))?;
        let indexed_paths: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("query: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        for path in &indexed_paths {
            if !live_paths.contains(path) {
                self.remove_session(path)?;
            }
        }
        Ok(())
    }

    /// Full-text search. Returns sessions matching the query, with snippets.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }

        // Escape FTS5 special characters and wrap each token in quotes for safety.
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }

        let terms = query_terms(query);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, jsonl_path,
                        snippet(session_fts, 2, '<mark>', '</mark>', '…', 40) as snip,
                        rank, raw
                 FROM session_fts
                 WHERE session_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| format!("prepare search: {e}"))?;

        // Deduplicate by session — keep best rank per session.
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        let rows = stmt
            .query_map(params![fts_query, limit * 5], |row| {
                let sql_snippet: String = row.get(2)?;
                let raw: String = row.get(4).unwrap_or_default();
                // Preferred: cut the snippet straight out of the author's text,
                // so no indexing artefact can reach the UI. Fall back to the
                // segmented column only when no term is locatable in `raw`.
                let snippet = build_snippet(&raw, &terms)
                    .unwrap_or_else(|| desegment_cjk(&sql_snippet));
                Ok(SearchHit {
                    session_id: row.get(0)?,
                    jsonl_path: row.get(1)?,
                    snippet,
                    rank: row.get(3)?,
                })
            })
            .map_err(|e| format!("search query: {e}"))?;

        for hit in rows.flatten() {
            let key = hit.jsonl_path.clone();
            if seen.insert(key) {
                results.push(hit);
                if results.len() >= limit {
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Record-level search restricted to the given transcripts, for an agent
    /// recovering its own history (`fleet__history`). Every matching record is
    /// returned (no per-session collapse) with its line number, best rank
    /// first. `jsonl_paths` empty → no hits.
    pub fn search_scoped(
        &self,
        jsonl_paths: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<ScopedHit>, String> {
        let query = query.trim();
        if query.is_empty() || jsonl_paths.is_empty() {
            return Ok(vec![]);
        }
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let terms = query_terms(query);

        // One bound placeholder per path; the set is small (a session plus its
        // relay predecessors), so a dynamic IN list is fine.
        let placeholders = (0..jsonl_paths.len())
            .map(|i| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT session_id, jsonl_path, line_no,
                    snippet(session_fts, 2, '<mark>', '</mark>', '…', 40) as snip,
                    rank, raw
             FROM session_fts
             WHERE session_fts MATCH ?1 AND jsonl_path IN ({placeholders})
             ORDER BY rank
             LIMIT ?2"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("prepare scoped search: {e}"))?;

        let mut bound: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(jsonl_paths.len() + 2);
        bound.push(Box::new(fts_query));
        bound.push(Box::new(limit.max(1) as i64));
        for p in jsonl_paths {
            bound.push(Box::new(p.clone()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = bound.iter().map(|b| b.as_ref()).collect();

        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let sql_snippet: String = row.get(3)?;
                let raw: String = row.get(5).unwrap_or_default();
                let snippet =
                    build_snippet(&raw, &terms).unwrap_or_else(|| desegment_cjk(&sql_snippet));
                Ok(ScopedHit {
                    session_id: row.get(0)?,
                    jsonl_path: row.get(1)?,
                    line_no: row.get::<_, i64>(2).unwrap_or(0),
                    snippet,
                    rank: row.get(4)?,
                })
            })
            .map_err(|e| format!("scoped search query: {e}"))?;

        Ok(rows.flatten().collect())
    }

    /// Bulk-index a batch of sessions. Called after each scan cycle.
    pub fn index_batch(&self, sessions: &[(String, String)]) {
        for (path, id) in sessions {
            if let Err(e) = self.index_session(path, id) {
                log_debug(&format!("search index error for {path}: {e}"));
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract all searchable text from a single JSONL line.
///
/// Also what `session_history::read` renders when an agent reads a record back
/// by line, so what is searchable and what is readable stay one definition.
pub(crate) fn extract_searchable_text(val: &Value) -> String {
    let msg_type = val["type"].as_str().unwrap_or("");

    // The session's AI-generated title lives on its own line; index it so a
    // session is findable by its human-readable title, not just transcript body.
    if msg_type == "ai-title" {
        return val["aiTitle"].as_str().unwrap_or("").to_string();
    }

    // Otherwise only index user and assistant messages.
    if msg_type != "user" && msg_type != "assistant" {
        return String::new();
    }

    let content = &val["message"]["content"];

    // Content can be a plain string (user messages).
    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    // Or an array of content blocks.
    let blocks = match content.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    let mut parts = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(t) = block["text"].as_str() {
                    parts.push(t);
                }
            }
            Some("thinking") => {
                if let Some(t) = block["thinking"].as_str() {
                    parts.push(t);
                }
            }
            Some("tool_use") => {
                if let Some(name) = block["name"].as_str() {
                    parts.push(name);
                }
            }
            _ => {}
        }
    }

    parts.join(" ")
}

/// True for scripts that `unicode61` does not segment: it treats a whole run of
/// them as a single token, so only a query equal to that entire run can match.
///
/// Covers Han (incl. ext-A and compatibility), kana, and Hangul syllables.
///
/// CJK punctuation and fullwidth forms are included deliberately. They are not
/// "characters to search for", but they sit *inside* Chinese runs, and leaving
/// them out strands a separator next to them that `desegment_cjk` then cannot
/// remove — snippets came back reading `已合并 ，main` instead of `已合并，main`.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303F}'   // CJK symbols + punctuation (。、「」…)
        | '\u{3040}'..='\u{30FF}' // hiragana + katakana
        | '\u{3400}'..='\u{4DBF}' // CJK ext A
        | '\u{4E00}'..='\u{9FFF}' // CJK unified
        | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
        | '\u{F900}'..='\u{FAFF}' // CJK compatibility
        | '\u{FF00}'..='\u{FFEF}' // fullwidth forms (，！？：；)
    )
}

/// Surround every CJK character with spaces so `unicode61` emits one token per
/// character instead of one token per run. Applied to both the indexed text and
/// the query, which turns a Chinese query into a phrase of single-character
/// tokens — i.e. a substring match against the original run.
///
/// Chosen over `tokenize='trigram'` after measuring both on a 5,000-line sample
/// of the real corpus: trigram inflated the index 2.9x (8.9 MB vs 3.1 MB) *and*
/// cannot match two-character words at all, which are the most common shape in
/// Chinese. This approach measured 3.1 MB — identical to the status quo.
fn segment_cjk(text: &str) -> String {
    // Fast path: pure-ASCII input (the overwhelming majority of lines) is
    // returned untouched, so English-only corpora pay nothing.
    if !text.chars().any(is_cjk) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + text.len() / 2);
    let mut prev_cjk = false;
    for c in text.chars() {
        let cur_cjk = is_cjk(c);
        // Insert a separator only at a CJK boundary where one is not already
        // present. Skipping it when `c` is itself whitespace keeps a space the
        // original text had from being doubled.
        if (cur_cjk || prev_cjk)
            && !c.is_whitespace()
            && !out.is_empty()
            && !out.ends_with(' ')
        {
            out.push(' ');
        }
        out.push(c);
        prev_cjk = cur_cjk;
    }
    out
}

/// Number of characters of context shown around a match in a snippet.
const SNIPPET_RADIUS: usize = 40;

/// Split a user query into the terms to highlight, in original (unsegmented)
/// form. Mirrors [`sanitize_fts_query`]'s whitespace split so the highlighted
/// terms are exactly the ones that were searched for.
fn query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Find the first case-insensitive occurrence of `needle` in `hay`, returning a
/// byte offset. ASCII-lowercases both sides; CJK is unaffected by case folding.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    hay.to_lowercase().find(&needle.to_lowercase()).and_then(|i| {
        // `to_lowercase` can change byte lengths (e.g. 'İ'), which would make
        // the offset meaningless. Fall back to an exact search in that case.
        if hay.to_lowercase().len() == hay.len() {
            Some(i)
        } else {
            hay.find(needle)
        }
    })
}

/// Build a display snippet from the **original** text, so nothing the indexer
/// did to make CJK searchable can leak into the UI.
///
/// This replaces FTS5's `snippet()`, which can only cut from the segmented
/// `content` column and therefore returned text with a space between every CJK
/// character. Cutting from `raw` sidesteps the whole reconstruction problem:
/// there is no separator to guess about, because none was ever inserted.
fn build_snippet(raw: &str, terms: &[String]) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();

    // Byte offset -> char index, so we can work in characters (CJK-safe).
    let byte_to_char: Vec<(usize, usize)> = raw
        .char_indices()
        .enumerate()
        .map(|(ci, (bi, _))| (bi, ci))
        .collect();
    let char_index_of = |byte: usize| -> usize {
        byte_to_char
            .binary_search_by_key(&byte, |(b, _)| *b)
            .map(|i| byte_to_char[i].1)
            .unwrap_or_else(|i| byte_to_char.get(i).map(|(_, c)| *c).unwrap_or(chars.len()))
    };

    // Locate every term; anchor the window on the earliest one.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        if let Some(byte) = find_ci(raw, term) {
            let start = char_index_of(byte);
            spans.push((start, start + term.chars().count()));
        }
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_unstable();

    let anchor = spans[0].0;
    let win_start = anchor.saturating_sub(SNIPPET_RADIUS);
    let win_end = (spans[0].1 + SNIPPET_RADIUS).min(chars.len());

    // Emit the window, wrapping any span that falls inside it.
    let mut out = String::new();
    if win_start > 0 {
        out.push('…');
    }
    let mut i = win_start;
    while i < win_end {
        if let Some(&(s, e)) = spans.iter().find(|&&(s, _)| s == i) {
            let e = e.min(win_end);
            out.push_str("<mark>");
            out.extend(&chars[s..e]);
            out.push_str("</mark>");
            i = e;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    if win_end < chars.len() {
        out.push('…');
    }
    Some(out)
}

/// Undo [`segment_cjk`] for display: drop the single space FTS5 hands back
/// between two CJK characters. Spaces adjacent to non-CJK are left alone, so
/// `"决 策 卡 walks fleet"` restores to `"决策卡 walks fleet"`.
///
/// Only used as a fallback now that [`build_snippet`] cuts from `raw`: it still
/// covers the case where a match lives in a different indexed row than the one
/// whose `raw` we are holding.
fn desegment_cjk(text: &str) -> String {
    if !text.chars().any(is_cjk) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' '
            && i > 0
            && i + 1 < chars.len()
            && is_cjk(chars[i - 1])
            && is_cjk(chars[i + 1])
        {
            i += 1; // drop this separator
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Sanitize user input for FTS5 MATCH syntax.
/// Splits on whitespace, quotes each token, joins with spaces (implicit AND).
///
/// A token containing CJK is segmented first, so the quoted result is a phrase
/// query over consecutive single-character tokens — `"决 策"` matches `决策`
/// anywhere inside an indexed run, but still requires the two characters to be
/// adjacent and in order.
fn sanitize_fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            // Remove any embedded double quotes to prevent FTS5 syntax errors.
            let clean = token.replace('"', "");
            let clean = segment_cjk(&clean);
            let clean = clean.trim();
            if clean.is_empty() {
                String::new()
            } else {
                format!("\"{}\"", clean)
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_ai_title_text() {
        let line = json!({
            "type": "ai-title",
            "aiTitle": "Continue extracting decision history",
        });
        assert_eq!(
            extract_searchable_text(&line),
            "Continue extracting decision history"
        );
    }

    /// The indexer races the CLI's writes, so it routinely sees a transcript
    /// whose last line is only half-flushed. That fragment must be indexed once
    /// it is complete — not skipped forever.
    ///
    /// The bug: reading to EOF consumed the fragment, `serde_json` rejected it,
    /// and the saved `byte_offset` was still set to the full file size. The next
    /// pass therefore resumed *past* the fragment, so the second half of that
    /// record was never re-read and the line stayed permanently unindexed.
    #[test]
    fn a_half_written_line_is_indexed_once_it_is_complete() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("fleet-search-partial-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("partial.jsonl");
        let _ = fs::remove_file(&jsonl);

        let complete = r#"{"type":"user","message":{"content":"first message"}}"#;
        let racy = r#"{"type":"user","message":{"content":"zqunique partial phrase"}}"#;
        let split = racy.len() / 2;

        // The CLI has flushed one whole record plus half of the next.
        fs::write(&jsonl, format!("{complete}\n{}", &racy[..split])).unwrap();

        let db = dir.join("idx-partial.db");
        let _ = fs::remove_file(&db);
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(jsonl.to_str().unwrap(), "sess-p").unwrap();

        // The CLI finishes writing that record.
        let mut f = fs::OpenOptions::new().append(true).open(&jsonl).unwrap();
        f.write_all(format!("{}\n", &racy[split..]).as_bytes()).unwrap();
        drop(f);

        idx.index_session(jsonl.to_str().unwrap(), "sess-p").unwrap();

        let hits = idx.search("zqunique", 10).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "a line that was half-written during an earlier pass must still be \
             indexed once complete"
        );

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    #[test]
    fn search_finds_title_only_session() {
        // A session whose distinctive phrase appears ONLY in its ai-title line,
        // never in any user/assistant message body.
        let dir = std::env::temp_dir().join("fleet-search-title-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("title-only.jsonl");
        let body = "{\"type\":\"ai-title\",\"aiTitle\":\"Continue extracting decision history\"}\n\
                    {\"type\":\"user\",\"message\":{\"content\":\"hello world\"}}\n";
        fs::write(&jsonl, body).unwrap();

        let db = dir.join("idx.db");
        let _ = fs::remove_file(&db);
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(jsonl.to_str().unwrap(), "sess-1").unwrap();

        let hits = idx.search("Continue extracting", 10).unwrap();
        assert_eq!(hits.len(), 1, "title-only phrase should be findable");
        assert_eq!(hits[0].session_id, "sess-1");

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    #[test]
    fn stale_version_db_is_rebuilt_on_open() {
        let dir = std::env::temp_dir().join("fleet-search-rebuild-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("rebuild.jsonl");
        fs::write(
            &jsonl,
            "{\"type\":\"ai-title\",\"aiTitle\":\"unique rebuild phrase\"}\n",
        )
        .unwrap();

        let db = dir.join("rebuild.db");
        let _ = fs::remove_file(&db);

        // Simulate a DB indexed under the old (v1) schema: index, then stamp
        // an older user_version so the next open triggers a rebuild.
        {
            let idx = SearchIndex::open_at(&db).unwrap();
            idx.index_session(jsonl.to_str().unwrap(), "sess-r").unwrap();
            idx.conn
                .execute_batch("PRAGMA user_version = 1;")
                .unwrap();
        }

        // Reopen: migration must clear index_meta so the file re-indexes.
        let idx = SearchIndex::open_at(&db).unwrap();
        let remaining: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM index_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "stale-version DB should be wiped on open");

        // After re-indexing, the title is findable.
        idx.index_session(jsonl.to_str().unwrap(), "sess-r").unwrap();
        let hits = idx.search("unique rebuild phrase", 10).unwrap();
        assert_eq!(hits.len(), 1);

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    /// FTS5's `unicode61` tokenizer does not segment CJK: a run of Chinese
    /// characters becomes **one** token, so only a query equal to that whole
    /// run matches and every substring query silently returns nothing.
    ///
    /// Measured on the real 194 MB index before the fix: `worktree` hit 4,364
    /// rows while `决策` hit 4 — and `决策` scored *fewer* hits than the longer
    /// `决策卡`, which is the tell. Chinese search was broken at every entry
    /// point (desktop box, `fleet search`, mobile relay, `mcp_inspect`).
    #[test]
    fn cjk_substring_queries_match() {
        let dir = std::env::temp_dir().join("fleet-search-cjk-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("cjk.jsonl");
        fs::write(
            &jsonl,
            "{\"type\":\"user\",\"message\":{\"content\":\"这条会话讨论了决策卡的实现细节\"}}\n",
        )
        .unwrap();

        let db = dir.join("cjk.db");
        let _ = fs::remove_file(&db);
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(jsonl.to_str().unwrap(), "sess-cjk").unwrap();

        // Two-character word — the most common shape in Chinese, and the case
        // an ngram tokenizer of size 3 would still miss.
        assert_eq!(
            idx.search("决策", 10).unwrap().len(),
            1,
            "a two-character Chinese substring must be findable"
        );
        // Three-character substring taken from the middle of the run.
        assert_eq!(
            idx.search("决策卡", 10).unwrap().len(),
            1,
            "a three-character Chinese substring must be findable"
        );
        // A word at the very end of the run.
        assert_eq!(
            idx.search("细节", 10).unwrap().len(),
            1,
            "a Chinese word at the end of the run must be findable"
        );
        // Mixed CJK + ASCII must keep working in the same query.
        assert_eq!(
            idx.search("实现", 10).unwrap().len(),
            1,
            "an interior Chinese word must be findable"
        );
        // A phrase that is absent must still miss — the fix must not make
        // everything match everything.
        assert_eq!(
            idx.search("永动机", 10).unwrap().len(),
            0,
            "an absent Chinese phrase must not match"
        );

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    /// A database whose `session_fts` predates the `raw` column must have the
    /// table **dropped**, not just emptied — `CREATE TABLE IF NOT EXISTS` will
    /// not widen an existing table, so a 4-column INSERT against the old shape
    /// fails with "table session_fts has 3 columns but 4 values were supplied".
    #[test]
    fn a_db_with_the_old_column_set_is_rebuilt_not_just_emptied() {
        let dir = std::env::temp_dir().join("fleet-search-oldcols-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("oldcols.jsonl");
        fs::write(
            &jsonl,
            "{\"type\":\"user\",\"message\":{\"content\":\"决策卡与工具\"}}\n",
        )
        .unwrap();

        let db = dir.join("oldcols.db");
        let _ = fs::remove_file(&db);

        // Hand-build a pre-`raw` (v3-shaped) database.
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE index_meta (
                     jsonl_path TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                     mtime_ms INTEGER NOT NULL, byte_offset INTEGER NOT NULL,
                     line_count INTEGER NOT NULL DEFAULT 0);
                 CREATE VIRTUAL TABLE session_fts USING fts5(
                     session_id UNINDEXED, jsonl_path UNINDEXED, content,
                     tokenize='unicode61');
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        }

        // Opening must rebuild the table with the new shape, and indexing must
        // then succeed rather than erroring on the column count.
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(jsonl.to_str().unwrap(), "sess-old")
            .expect("indexing must succeed after the table is rebuilt");
        assert_eq!(idx.search("决策卡", 10).unwrap().len(), 1);

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    /// The content column is stored segmented, so `snippet()` hands back text
    /// with a space between every CJK character. That is an index-side detail
    /// and must never reach the UI.
    #[test]
    fn cjk_snippets_are_restored_for_display() {
        let dir = std::env::temp_dir().join("fleet-search-cjk-snippet-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("snip.jsonl");
        fs::write(
            &jsonl,
            "{\"type\":\"user\",\"message\":{\"content\":\"决策卡走 fleet__ask 工具\"}}\n",
        )
        .unwrap();

        let db = dir.join("snip.db");
        let _ = fs::remove_file(&db);
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(jsonl.to_str().unwrap(), "sess-s").unwrap();

        let hits = idx.search("决策卡", 10).unwrap();
        assert_eq!(hits.len(), 1);
        let snip = &hits[0].snippet;
        assert!(
            snip.contains("<mark>决策卡</mark>"),
            "the matched term must be highlighted contiguously, got: {snip}"
        );
        assert!(
            !snip.contains("决 策"),
            "snippet must not leak the segmented form, got: {snip}"
        );
        assert!(
            snip.contains("fleet__ask"),
            "ASCII text must survive, got: {snip}"
        );
        // Snippets are now cut from `raw`, so the text is byte-for-byte what
        // the author wrote once the <mark> wrappers are removed.
        assert_eq!(
            snip.replace("<mark>", "").replace("</mark>", ""),
            "决策卡走 fleet__ask 工具",
            "snippet must reproduce the original text exactly"
        );

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    /// The whole point of the `raw` column: a CJK↔ASCII boundary must come back
    /// exactly as written, with no separator the indexer introduced.
    #[test]
    fn cjk_ascii_boundary_survives_verbatim() {
        let dir = std::env::temp_dir().join("fleet-search-boundary-test");
        let _ = fs::create_dir_all(&dir);
        let jsonl = dir.join("boundary.jsonl");
        // Both spacing conventions in one line: no space around `main`, and a
        // conventional space around `fleet`.
        let original = "已合并，main全绿，使用 fleet 工具。下一步";
        fs::write(
            &jsonl,
            format!(
                "{{\"type\":\"user\",\"message\":{{\"content\":\"{original}\"}}}}\n"
            ),
        )
        .unwrap();

        let db = dir.join("boundary.db");
        let _ = fs::remove_file(&db);
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(jsonl.to_str().unwrap(), "sess-b").unwrap();

        // `main` is searchable even though it is welded between CJK…
        assert_eq!(idx.search("main", 10).unwrap().len(), 1);
        let hits = idx.search("已合并", 10).unwrap();
        assert_eq!(hits.len(), 1);
        let plain = hits[0]
            .snippet
            .replace("<mark>", "")
            .replace("</mark>", "");
        // …and the snippet is the author's text, both conventions intact.
        assert_eq!(plain, original, "snippet must be verbatim");
        assert!(
            plain.contains("已合并，main全绿"),
            "no space may be introduced at a CJK↔ASCII boundary: {plain}"
        );
        assert!(
            plain.contains("使用 fleet 工具"),
            "a space the author typed must be preserved: {plain}"
        );

        let _ = fs::remove_file(&jsonl);
        let _ = fs::remove_file(&db);
    }

    /// `search_scoped` is the agent-facing query: it must stay inside the given
    /// transcripts, return every matching record (not one per session), and
    /// carry a line number that points at the record — including for records
    /// folded in a later incremental pass, where the number must continue from
    /// the saved `line_count` rather than restart at 1.
    #[test]
    fn scoped_search_returns_every_record_with_its_line_number() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("fleet-search-scoped-test");
        let _ = fs::create_dir_all(&dir);
        let own = dir.join("own.jsonl");
        let other = dir.join("other.jsonl");
        fs::write(
            &own,
            "{\"type\":\"user\",\"message\":{\"content\":\"zqscoped first\"}}\n\
             {\"type\":\"progress\"}\n\
             {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"zqscoped third\"}]}}\n",
        )
        .unwrap();
        fs::write(
            &other,
            "{\"type\":\"user\",\"message\":{\"content\":\"zqscoped elsewhere\"}}\n",
        )
        .unwrap();

        let db = dir.join("scoped.db");
        let _ = fs::remove_file(&db);
        let idx = SearchIndex::open_at(&db).unwrap();
        idx.index_session(own.to_str().unwrap(), "sess-own").unwrap();
        idx.index_session(other.to_str().unwrap(), "sess-other").unwrap();

        // A second, incremental pass: the line number must be 4, not 1.
        let mut f = fs::OpenOptions::new().append(true).open(&own).unwrap();
        f.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"zqscoped fourth\"}}\n")
            .unwrap();
        drop(f);
        idx.index_session(own.to_str().unwrap(), "sess-own").unwrap();

        let mut hits = idx
            .search_scoped(&[own.to_str().unwrap().to_string()], "zqscoped", 10)
            .unwrap();
        hits.sort_by_key(|h| h.line_no);
        let lines: Vec<i64> = hits.iter().map(|h| h.line_no).collect();
        assert_eq!(lines, vec![1, 3, 4], "every record, located, none from `other`: {hits:?}");
        assert!(hits.iter().all(|h| h.session_id == "sess-own"));

        // The unscoped search still collapses to one hit per session.
        assert_eq!(idx.search("zqscoped", 10).unwrap().len(), 2);
        // Empty scope → nothing, rather than everything.
        assert!(idx.search_scoped(&[], "zqscoped", 10).unwrap().is_empty());

        let _ = fs::remove_file(&own);
        let _ = fs::remove_file(&other);
        let _ = fs::remove_file(&db);
    }

    /// Round-trip guard: desegmentation must not eat spaces that were in the
    /// original text, and must be a no-op for pure ASCII.
    #[test]
    fn segment_roundtrip_preserves_meaningful_spaces() {
        assert_eq!(segment_cjk("hello world"), "hello world");
        assert_eq!(desegment_cjk("hello world"), "hello world");
        assert_eq!(desegment_cjk(&segment_cjk("决策卡")), "决策卡");
        assert_eq!(
            desegment_cjk(&segment_cjk("决策卡 walks fleet")),
            "决策卡 walks fleet"
        );
        // A space the user actually typed between two Chinese words is the one
        // case we cannot distinguish; document the known behaviour.
        assert_eq!(desegment_cjk(&segment_cjk("决策 卡")), "决策卡");
        // Punctuation inside a Chinese run restores cleanly.
        assert_eq!(desegment_cjk(&segment_cjk("完成。下一步")), "完成。下一步");
        assert_eq!(
            desegment_cjk(&segment_cjk("决策卡，工具，面板")),
            "决策卡，工具，面板"
        );

        // At a CJK↔ASCII boundary `desegment_cjk` cannot restore the original:
        // `unicode61` does not split there either (indexing `已合并main全绿` as
        // one run makes even `main` unsearchable), so `segment_cjk` *must* put
        // a separator in, and afterwards it is indistinguishable from a space
        // the author typed.
        //
        // This is why snippets are cut from the `raw` column instead — see
        // `cjk_ascii_boundary_survives_verbatim`. The lossy round-trip below is
        // only the fallback path's behaviour, pinned here so it stays known.
        assert_eq!(
            desegment_cjk(&segment_cjk("已合并，main全绿")),
            "已合并， main 全绿"
        );
    }
}
