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
const SCHEMA_VERSION: i64 = 2;

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
                 tokenize='unicode61'
             );",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;

        // One-time migration: if the DB was built under an older field set,
        // wipe content + offsets so the next scan re-indexes everything.
        let db_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if db_version < SCHEMA_VERSION {
            conn.execute_batch(&format!(
                "DELETE FROM session_fts;
                 DELETE FROM index_meta;
                 PRAGMA user_version = {SCHEMA_VERSION};"
            ))
            .map_err(|e| format!("sqlite migrate: {e}"))?;
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
                tx.execute(
                    "INSERT INTO session_fts(session_id, jsonl_path, content) VALUES (?1, ?2, ?3)",
                    params![session_id, jsonl_path, text],
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

        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, jsonl_path,
                        snippet(session_fts, 2, '<mark>', '</mark>', '…', 40) as snip,
                        rank
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
                Ok(SearchHit {
                    session_id: row.get(0)?,
                    jsonl_path: row.get(1)?,
                    snippet: row.get(2)?,
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
fn extract_searchable_text(val: &Value) -> String {
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

/// Sanitize user input for FTS5 MATCH syntax.
/// Splits on whitespace, quotes each token, joins with spaces (implicit AND).
fn sanitize_fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            // Remove any embedded double quotes to prevent FTS5 syntax errors.
            let clean = token.replace('"', "");
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
}
