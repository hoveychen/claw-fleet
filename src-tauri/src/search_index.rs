//! Full-text search index for session content using SQLite FTS5.
//!
//! Tracks each JSONL file by `(mtime_ms, byte_offset)` so that:
//! - Unchanged files are skipped entirely (no duplicate indexing).
//! - Appended content is indexed incrementally from the last byte offset.
//! - Truncated/rewritten files are fully re-indexed.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::log_debug;

// ── SearchHit ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
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
    /// Open (or create) the search database at `~/.claude/fleet-search.db`.
    pub fn open() -> Result<Self, String> {
        let db_path = dirs::home_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?
            .join(".claude")
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

        let reader = BufReader::new(file);
        let mut new_lines = 0i64;

        let tx = self.conn.unchecked_transaction().map_err(|e| format!("tx: {e}"))?;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                new_lines += 1;
                continue;
            }

            let parsed: Value = match serde_json::from_str(&line) {
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
                file_size,
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
    // Only index user and assistant messages.
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
