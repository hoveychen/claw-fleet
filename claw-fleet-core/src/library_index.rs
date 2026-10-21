//! Full-text index over the library: every wiki doc and every artifact, in one
//! SQLite FTS5 table, so a search ranks both stores against each other.
//!
//! ## A cache, not a store
//!
//! The wiki and the artifact store stay the source of truth — one directory
//! per item, no global index, so concurrent writers from several processes
//! never contend (see [`crate::wiki`] / [`crate::artifacts`]). This index is a
//! derived cache of them. [`LibraryIndex::sync`] reconciles it against the
//! stores with a stat-only scan: an item is re-read only when the fingerprint
//! of its metadata files (`doc.json` + `summary.json`, `meta.json` +
//! `text.txt`) changed, a body is re-read only when the item's current
//! version changed, and rows whose directory is gone are dropped. Every search
//! syncs first, so a doc published by the CLI in another process is findable
//! the next time anyone searches, without any writer having to know the index
//! exists.
//!
//! ## Its own database file
//!
//! `~/.fleet/library/search.db`, not new tables in `fleet-search.db`:
//! - that file's migration stamps one `PRAGMA user_version` for the session
//!   schema, and two schemas sharing one stamp cannot be bumped independently;
//! - the session indexer holds long write transactions while it folds
//!   transcripts, which library writes would queue behind;
//! - deleting this file is a complete, safe rebuild.
//!
//! Several processes (desktop, `fleet serve`, CLI, MCP) may sync at once. WAL
//! plus `busy_timeout` serializes their write transactions; every write is an
//! idempotent upsert keyed by item, so two processes reconciling the same
//! change just do it twice.
//!
//! ## Text
//!
//! CJK is stored segmented per character ([`crate::fts_text::segment_cjk`]),
//! the same approach the session index measured against trigram. The FTS
//! table is contentless; the raw body lives in `items.body` and snippets are
//! cut from it ([`crate::fts_text::build_snippet`]).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::artifacts::{self, Artifact};
use crate::fts_text::{build_snippet, query_terms, sanitize_fts_query_prefix, segment_cjk};
use crate::library::{self, LibraryFolder, Roots, ITEM_ARTIFACT, ITEM_DOC};
use crate::wiki::{self, WikiDoc};

/// Bumped whenever the indexed shape changes; an older database has its
/// `items` and `library_fts` tables dropped and rebuilt on open. `meta` is
/// kept (it holds the summary policy's activation time, which must survive a
/// rebuild).
///
/// History:
///   1 — initial.
const SCHEMA_VERSION: i64 = 1;

/// Database file name under the library root.
pub const DB_FILE: &str = "search.db";

/// Most characters of an item's body that are indexed.
pub const MAX_BODY_CHARS: usize = 200_000;

/// `body_state` values.
pub const BODY_DONE: &str = "done";
/// An artifact whose text has not been extracted yet ([`LibraryIndex::extract_pending`]).
pub const BODY_PENDING: &str = "pending";
/// Nothing to extract (video, audio, archive, an image, a failed parse).
pub const BODY_NONE: &str = "none";

/// Artifact kinds whose bytes may hold text worth extracting.
const EXTRACTABLE_KINDS: &[&str] = &["text", "pdf", "doc", "sheet", "slides"];

/// bm25 column weights: title, folder, tags, summary, body.
const RANK_WEIGHTS: &str = "10.0, 3.0, 6.0, 4.0, 1.0";

// ── Types ────────────────────────────────────────────────────────────────────

/// One search hit, from either store.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHit {
    /// [`ITEM_DOC`] or [`ITEM_ARTIFACT`].
    pub kind: String,
    /// The doc's slug or the artifact's id.
    pub id: String,
    pub title: String,
    /// Project key ([`library::project_key`]).
    pub project: String,
    pub workspace_path: String,
    /// Schema folder the item is filed in; `""` = unfiled.
    pub folder: String,
    /// Where it sits in its own store (a slug directory, an artifact's
    /// effective directory).
    pub location: String,
    /// Which field the snippet was cut from: `content`, `summary`, `title`,
    /// or `meta` (matched on folder/tags only).
    pub field: String,
    /// Excerpt around the first match with `<mark>…</mark>`; empty for a
    /// title/meta-only hit.
    pub snippet: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub updated_ms: u64,
    /// The artifact's coarse kind; empty for docs.
    pub artifact_kind: String,
    /// bm25 score, lower is better.
    pub rank: f64,
}

/// What to search.
#[derive(Clone, Debug, Default)]
pub struct SearchOpts {
    /// Restrict to one project: a workspace path, resolved with
    /// [`library::project_key`] so a worktree matches its repo.
    pub workspace: Option<String>,
    /// Restrict to one store ([`ITEM_DOC`] / [`ITEM_ARTIFACT`]).
    pub kind: Option<String>,
    /// 0 = a default of 50.
    pub limit: usize,
}

/// What one [`LibraryIndex::sync`] did.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncStats {
    pub scanned: usize,
    pub upserted: usize,
    pub refiled: usize,
    pub removed: usize,
}

/// Counts for the index's status line.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub docs: usize,
    pub artifacts: usize,
    pub with_body: usize,
    pub pending_extraction: usize,
    pub with_summary: usize,
}

// ── Index ────────────────────────────────────────────────────────────────────

pub struct LibraryIndex {
    conn: Connection,
}

/// One item as read from its store, ready to upsert.
struct Row {
    key: String,
    kind: &'static str,
    item_id: String,
    dir: String,
    project: String,
    workspace_path: String,
    folder: String,
    /// What filing is decided from: a doc's slug, an artifact's `path`.
    filed: String,
    location: String,
    title: String,
    summary: String,
    tags: Vec<String>,
    updated_ms: u64,
    created_ms: u64,
    artifact_kind: String,
    version: String,
    summary_version: String,
    meta_sig: String,
    body_sig: String,
    body: String,
    body_state: &'static str,
}

/// The parts of an indexed row a sync compares against.
struct Known {
    id: i64,
    meta_sig: String,
    filed: String,
    body_sig: String,
    project: String,
    folder: String,
}

pub fn db_path(roots: &Roots) -> PathBuf {
    roots.library.join(DB_FILE)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `mtime_ns:len` of a file, `-` when it is missing.
fn stat_sig(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(m) => {
            let ns = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{ns}:{}", m.len())
        }
        Err(_) => "-".to_string(),
    }
}

fn mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn clip_chars(s: String, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s,
    }
}

impl LibraryIndex {
    /// Open (or create) the index for the real stores.
    pub fn open() -> Result<Self, String> {
        Self::open_at(&db_path(&Roots::home()?))
    }

    pub fn open_at(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(path).map_err(|e| format!("sqlite open: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 10000;",
        )
        .map_err(|e| format!("sqlite pragma: {e}"))?;

        // Drop before create: `CREATE … IF NOT EXISTS` cannot change the shape
        // of a table that already exists (see `search_index` for the history).
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);
        if version < SCHEMA_VERSION {
            conn.execute_batch(
                "DROP TABLE IF EXISTS library_fts;
                 DROP TABLE IF EXISTS items;",
            )
            .map_err(|e| format!("sqlite migrate drop: {e}"))?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS items (
                 id              INTEGER PRIMARY KEY,
                 key             TEXT NOT NULL UNIQUE,
                 kind            TEXT NOT NULL,
                 item_id         TEXT NOT NULL,
                 dir             TEXT NOT NULL,
                 project         TEXT NOT NULL,
                 workspace_path  TEXT NOT NULL,
                 folder          TEXT NOT NULL,
                 filed           TEXT NOT NULL,
                 location        TEXT NOT NULL,
                 title           TEXT NOT NULL,
                 summary         TEXT NOT NULL,
                 tags            TEXT NOT NULL,
                 updated_ms      INTEGER NOT NULL,
                 created_ms      INTEGER NOT NULL,
                 artifact_kind   TEXT NOT NULL,
                 version         TEXT NOT NULL,
                 summary_version TEXT NOT NULL,
                 meta_sig        TEXT NOT NULL,
                 body_sig        TEXT NOT NULL,
                 body            TEXT NOT NULL,
                 body_state      TEXT NOT NULL,
                 summary_claim_until_ms INTEGER NOT NULL DEFAULT 0,
                 summary_failures       INTEGER NOT NULL DEFAULT 0,
                 summary_retry_ms       INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS items_kind_dir ON items(kind, dir);
             CREATE INDEX IF NOT EXISTS items_project ON items(project);
             CREATE VIRTUAL TABLE IF NOT EXISTS library_fts USING fts5(
                 title, folder, tags, summary, body,
                 content='', contentless_delete=1, tokenize='unicode61'
             );
             CREATE TABLE IF NOT EXISTS meta (
                 k TEXT PRIMARY KEY,
                 v TEXT NOT NULL
             );",
        )
        .map_err(|e| format!("sqlite schema: {e}"))?;
        if version < SCHEMA_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
                .map_err(|e| format!("sqlite migrate stamp: {e}"))?;
        }
        let index = Self { conn };
        // The summary policy's "new item" line: set once, on first open.
        if index.meta_get("activated_ms").is_none() {
            index.meta_set("activated_ms", &now_ms().to_string())?;
        }
        Ok(index)
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn meta_get(&self, k: &str) -> Option<String> {
        self.conn
            .query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn meta_set(&self, k: &str, v: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(k, v) VALUES (?1, ?2)
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                params![k, v],
            )
            .map(|_| ())
            .map_err(|e| format!("meta write: {e}"))
    }

    // ── Sync ─────────────────────────────────────────────────────────────────

    /// Reconcile the index with both stores. Cheap when nothing changed: one
    /// `read_dir` per store and a couple of `stat`s per item.
    pub fn sync(&self, roots: &Roots) -> Result<SyncStats, String> {
        let mut stats = SyncStats::default();
        let folders = folders_by_project(roots);
        let mut keys: HashMap<String, String> = HashMap::new();
        let mut project_of = |ws: &str| -> String {
            keys.entry(ws.to_string())
                .or_insert_with(|| library::project_key(ws))
                .clone()
        };

        let mut known: HashMap<(String, String), Known> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, filed, kind, dir, meta_sig, body_sig, project, folder FROM items")
                .map_err(|e| format!("prepare: {e}"))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        (r.get::<_, String>(2)?, r.get::<_, String>(3)?),
                        Known {
                            id: r.get(0)?,
                            filed: r.get(1)?,
                            meta_sig: r.get(4)?,
                            body_sig: r.get(5)?,
                            project: r.get(6)?,
                            folder: r.get(7)?,
                        },
                    ))
                })
                .map_err(|e| format!("query: {e}"))?;
            for row in rows.flatten() {
                known.insert(row.0, row.1);
            }
        }

        // Collect the work outside the write transaction: reading bodies is
        // file I/O and must not hold the database lock.
        let mut upserts: Vec<Row> = Vec::new();
        let mut refiles: Vec<(i64, String)> = Vec::new();
        let mut seen: Vec<(String, String)> = Vec::new();

        for dir in subdirs(&roots.wiki) {
            let doc_dir = roots.wiki.join(&dir);
            let meta_sig = format!(
                "{}|{}",
                stat_sig(&doc_dir.join("doc.json")),
                stat_sig(&doc_dir.join(wiki::SUMMARY_FILE))
            );
            if meta_sig.starts_with('-') {
                continue;
            }
            stats.scanned += 1;
            let k = (ITEM_DOC.to_string(), dir.clone());
            seen.push(k.clone());
            match known.get(&k) {
                Some(old) if old.meta_sig == meta_sig => {
                    // Unchanged item, but the schema around it may not be.
                    let folder = folders
                        .get(&old.project)
                        .and_then(|f| library::doc_folder(f, &old.filed))
                        .unwrap_or_default();
                    if folder != old.folder {
                        refiles.push((old.id, folder));
                    }
                }
                old => {
                    let Ok(doc) = wiki::get_doc_in(&roots.wiki, &dirname_slug(&dir)) else {
                        continue;
                    };
                    let project = project_of(&doc.workspace_path);
                    let folder = folders
                        .get(&project)
                        .and_then(|f| library::doc_folder(f, &doc.slug))
                        .unwrap_or_default();
                    upserts.push(doc_row(
                        roots,
                        &doc,
                        dir,
                        project,
                        folder,
                        meta_sig,
                        old.map(|o| o.body_sig.as_str()),
                    ));
                }
            }
        }

        for dir in subdirs(&roots.artifacts) {
            let adir = roots.artifacts.join(&dir);
            let meta_sig = format!(
                "{}|{}",
                stat_sig(&adir.join("meta.json")),
                stat_sig(&adir.join("text.txt"))
            );
            if meta_sig.starts_with('-') {
                continue;
            }
            stats.scanned += 1;
            let k = (ITEM_ARTIFACT.to_string(), dir.clone());
            seen.push(k.clone());
            match known.get(&k) {
                Some(old) if old.meta_sig == meta_sig => {
                    let in_schema = !old.filed.is_empty()
                        && folders
                            .get(&old.project)
                            .is_some_and(|f| f.iter().any(|x| x.path == old.filed));
                    let folder = if in_schema { old.filed.clone() } else { String::new() };
                    if folder != old.folder {
                        refiles.push((old.id, folder));
                    }
                }
                old => {
                    let Ok(a) = artifacts::get_in(&roots.artifacts, &dir) else {
                        continue;
                    };
                    let project = project_of(&a.workspace_path);
                    let folder = folders
                        .get(&project)
                        .and_then(|f| library::artifact_folder(f, &a))
                        .unwrap_or_default();
                    upserts.push(artifact_row(
                        roots,
                        &a,
                        dir,
                        project,
                        folder,
                        meta_sig,
                        old.map(|o| o.body_sig.as_str()),
                    ));
                }
            }
        }

        let seen: std::collections::HashSet<(String, String)> = seen.into_iter().collect();
        let gone: Vec<i64> = known
            .iter()
            .filter(|(k, _)| !seen.contains(*k))
            .map(|(_, v)| v.id)
            .collect();

        if upserts.is_empty() && refiles.is_empty() && gone.is_empty() {
            return Ok(stats);
        }

        let tx = self.begin_immediate()?;
        for row in &upserts {
            upsert(&tx, row)?;
        }
        for (id, folder) in &refiles {
            set_folder(&tx, *id, folder)?;
        }
        for id in &gone {
            delete_row(&tx, *id)?;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        stats.upserted = upserts.len();
        stats.refiled = refiles.len();
        stats.removed = gone.len();
        Ok(stats)
    }

    fn begin_immediate(&self) -> Result<rusqlite::Transaction<'_>, String> {
        rusqlite::Transaction::new_unchecked(&self.conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| format!("begin: {e}"))
    }

    /// Extract the text of artifacts still marked [`BODY_PENDING`], until
    /// `budget` runs out. Returns how many were done. The text is stored in
    /// the index only — the artifact store is never written.
    pub fn extract_pending(&self, roots: &Roots, budget: Duration) -> Result<usize, String> {
        let started = Instant::now();
        let pending: Vec<(String, String, String)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT key, item_id, meta_sig FROM items
                     WHERE body_state = ?1 ORDER BY updated_ms DESC LIMIT 200",
                )
                .map_err(|e| format!("prepare: {e}"))?;
            let rows = stmt
                .query_map(params![BODY_PENDING], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| format!("query: {e}"))?;
            rows.flatten().collect()
        };
        let mut done = 0;
        for (key, id, meta_sig) in pending {
            if started.elapsed() >= budget {
                break;
            }
            let Ok(a) = artifacts::get_in(&roots.artifacts, &id) else {
                continue;
            };
            let blob = artifacts::blob_path(&roots.artifacts, &a);
            let x = crate::library_ingest::extract::extract(&blob, &a.name);
            let text = clip_chars(x.text, MAX_BODY_CHARS);
            let state = if text.trim().is_empty() { BODY_NONE } else { BODY_DONE };
            let tx = self.begin_immediate()?;
            // Only if the item did not change while we were extracting.
            let row: Option<(i64, String, String, String, String)> = tx
                .query_row(
                    "SELECT id, title, folder, tags, summary FROM items
                     WHERE key = ?1 AND meta_sig = ?2 AND body_state = ?3",
                    params![key, meta_sig, BODY_PENDING],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()
                .map_err(|e| format!("lookup: {e}"))?;
            if let Some((rowid, title, folder, tags, summary)) = row {
                tx.execute(
                    "UPDATE items SET body = ?2, body_state = ?3, body_sig = ?4 WHERE id = ?1",
                    params![rowid, text, state, format!("{}:x", a.current_version)],
                )
                .map_err(|e| format!("update body: {e}"))?;
                let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
                write_fts(&tx, rowid, &title, &folder, &tags, &summary, &text)?;
                done += 1;
            }
            tx.commit().map_err(|e| format!("commit: {e}"))?;
        }
        Ok(done)
    }

    // ── Search ───────────────────────────────────────────────────────────────

    /// Ranked hits across both stores. Does not sync — see [`search`].
    pub fn search(&self, query: &str, opts: &SearchOpts) -> Result<Vec<LibraryHit>, String> {
        let fts = sanitize_fts_query_prefix(query.trim());
        if fts.is_empty() {
            return Ok(Vec::new());
        }
        let terms = query_terms(query);
        let limit = if opts.limit == 0 { 50 } else { opts.limit };
        let project = opts
            .workspace
            .as_deref()
            .filter(|w| !w.trim().is_empty())
            .map(library::project_key);

        let sql = format!(
            "SELECT i.kind, i.item_id, i.title, i.project, i.workspace_path, i.folder,
                    i.location, i.summary, i.tags, i.updated_ms, i.artifact_kind, i.body,
                    bm25(library_fts, {RANK_WEIGHTS}) AS score
             FROM library_fts JOIN items i ON i.id = library_fts.rowid
             WHERE library_fts MATCH ?1
               AND (?2 IS NULL OR i.project = ?2)
               AND (?3 IS NULL OR i.kind = ?3)
             ORDER BY score, i.updated_ms DESC
             LIMIT ?4"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(|e| format!("prepare search: {e}"))?;
        let rows = stmt
            .query_map(
                params![fts, project, opts.kind, limit as i64],
                |r| {
                    let title: String = r.get(2)?;
                    let summary: String = r.get(7)?;
                    let body: String = r.get(11)?;
                    let tags: String = r.get(8)?;
                    let (field, snippet) = if let Some(s) = build_snippet(&body, &terms) {
                        ("content", s)
                    } else if let Some(s) = build_snippet(&summary, &terms) {
                        ("summary", s)
                    } else if build_snippet(&title, &terms).is_some() {
                        ("title", String::new())
                    } else {
                        ("meta", String::new())
                    };
                    Ok(LibraryHit {
                        kind: r.get(0)?,
                        id: r.get(1)?,
                        title,
                        project: r.get(3)?,
                        workspace_path: r.get(4)?,
                        folder: r.get(5)?,
                        location: r.get(6)?,
                        field: field.to_string(),
                        snippet,
                        summary,
                        tags: serde_json::from_str(&tags).unwrap_or_default(),
                        updated_ms: r.get::<_, i64>(9)? as u64,
                        artifact_kind: r.get(10)?,
                        rank: r.get(12)?,
                    })
                },
            )
            .map_err(|e| format!("search: {e}"))?;
        Ok(rows.flatten().collect())
    }

    pub fn stats(&self) -> Result<IndexStats, String> {
        self.conn
            .query_row(
                "SELECT
                    COALESCE(SUM(kind = 'doc'), 0),
                    COALESCE(SUM(kind = 'artifact'), 0),
                    COALESCE(SUM(body_state = 'done'), 0),
                    COALESCE(SUM(body_state = 'pending'), 0),
                    COALESCE(SUM(summary != ''), 0)
                 FROM items",
                [],
                |r| {
                    Ok(IndexStats {
                        docs: r.get::<_, i64>(0)? as usize,
                        artifacts: r.get::<_, i64>(1)? as usize,
                        with_body: r.get::<_, i64>(2)? as usize,
                        pending_extraction: r.get::<_, i64>(3)? as usize,
                        with_summary: r.get::<_, i64>(4)? as usize,
                    })
                },
            )
            .map_err(|e| format!("stats: {e}"))
    }
}

// ── Rows ─────────────────────────────────────────────────────────────────────

/// Immediate subdirectories of a store root, by name.
fn subdirs(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect()
}

/// Inverse of [`wiki::slug_to_dirname`] (`a%2Fb` → `a/b`).
fn dirname_slug(dir: &str) -> String {
    dir.replace("%2F", "/")
}

fn folders_by_project(roots: &Roots) -> HashMap<String, Vec<LibraryFolder>> {
    let mut out: HashMap<String, Vec<LibraryFolder>> = HashMap::new();
    for f in library::all_folders(roots) {
        out.entry(f.workspace_path.clone()).or_default().push(f);
    }
    // `wiki_prefix` is derived on read; all_folders already filled it in.
    out
}

/// The plain text of a doc's current entry file.
fn doc_body(roots: &Roots, doc: &WikiDoc) -> String {
    let Ok(file) = wiki::get_file_in(&roots.wiki, &doc.slug, "current", &doc.entry) else {
        return String::new();
    };
    let raw = String::from_utf8_lossy(&file.bytes).into_owned();
    let text = if doc.kind == "markdown" {
        raw
    } else {
        wiki::strip_html(&raw).split_whitespace().collect::<Vec<_>>().join(" ")
    };
    clip_chars(text, MAX_BODY_CHARS)
}

fn doc_row(
    roots: &Roots,
    doc: &WikiDoc,
    dir: String,
    project: String,
    folder: String,
    meta_sig: String,
    old_body_sig: Option<&str>,
) -> Row {
    let body_sig = format!("{}:{}", doc.current_version, doc.entry);
    // An unchanged version keeps the body already indexed (`body` empty with
    // `BODY_DONE` + matching sig means "keep" to `upsert`).
    let (body, body_state) = if old_body_sig == Some(body_sig.as_str()) {
        (None, BODY_DONE)
    } else {
        (Some(doc_body(roots, doc)), BODY_DONE)
    };
    Row {
        key: format!("{ITEM_DOC}:{}", doc.slug),
        kind: ITEM_DOC,
        item_id: doc.slug.clone(),
        dir,
        project,
        workspace_path: doc.workspace_path.clone(),
        folder,
        filed: doc.slug.clone(),
        location: library::slug_dir(&doc.slug).to_string(),
        title: if doc.title.is_empty() {
            wiki::slug_basename(&doc.slug).to_string()
        } else {
            doc.title.clone()
        },
        summary: doc.summary.clone(),
        tags: doc.tags.clone(),
        updated_ms: doc.updated_ms.max(doc.created_ms),
        created_ms: doc.created_ms,
        artifact_kind: String::new(),
        version: doc.current_version.clone(),
        summary_version: doc.summary_version.clone(),
        meta_sig,
        body_sig,
        body: body.unwrap_or_else(|| KEEP_BODY.to_string()),
        body_state,
    }
}

/// Sentinel body meaning "the indexed body is still current".
const KEEP_BODY: &str = "\u{0}keep";

fn artifact_row(
    roots: &Roots,
    a: &Artifact,
    dir: String,
    project: String,
    folder: String,
    meta_sig: String,
    old_body_sig: Option<&str>,
) -> Row {
    let text_file = roots.artifacts.join(&a.id).join("text.txt");
    let current_added = a
        .versions
        .iter()
        .find(|v| v.id == a.current_version)
        .map(|v| v.added_ms)
        .unwrap_or(a.created_ms);
    // An ingester's text.txt is only trusted for the version it was written
    // for: a newer version makes it stale (one second of slack for clocks).
    let fresh_text = mtime_ms(&text_file)
        .filter(|m| m + 1000 >= current_added)
        .and_then(|_| artifacts::read_text_in(&roots.artifacts, &a.id))
        .filter(|t| !t.trim().is_empty());
    let (body, body_sig, body_state) = match fresh_text {
        Some(t) => (
            Some(clip_chars(t, MAX_BODY_CHARS)),
            format!("{}:t", a.current_version),
            BODY_DONE,
        ),
        None => {
            let extracted_sig = format!("{}:x", a.current_version);
            if old_body_sig == Some(extracted_sig.as_str()) {
                (None, extracted_sig, BODY_DONE)
            } else if EXTRACTABLE_KINDS.contains(&a.kind.as_str()) {
                (Some(String::new()), format!("{}:p", a.current_version), BODY_PENDING)
            } else {
                (Some(String::new()), format!("{}:n", a.current_version), BODY_NONE)
            }
        }
    };
    let title = if a.title.is_empty() { a.name.clone() } else { a.title.clone() };
    // The file name is searchable too when the title replaced it.
    let title = if a.name != title { format!("{title} · {}", a.name) } else { title };
    let summary = match (a.summary.trim(), a.note.trim()) {
        (s, "") => s.to_string(),
        ("", n) => n.to_string(),
        (s, n) => format!("{s}\n{n}"),
    };
    Row {
        key: format!("{ITEM_ARTIFACT}:{}", a.id),
        kind: ITEM_ARTIFACT,
        item_id: a.id.clone(),
        dir,
        project,
        workspace_path: a.workspace_path.clone(),
        folder,
        filed: a.path.clone(),
        location: artifacts::effective_directory(a),
        title,
        summary,
        tags: a.tags.clone(),
        updated_ms: a.created_ms,
        created_ms: a
            .versions
            .last()
            .map(|v| v.added_ms)
            .unwrap_or(a.created_ms),
        artifact_kind: a.kind.clone(),
        version: a.current_version.clone(),
        // Artifacts carry no summary version of their own: one that already
        // has a summary (the upload organizer's, or ours) is not due again.
        summary_version: if a.summary.trim().is_empty() {
            String::new()
        } else {
            a.current_version.clone()
        },
        meta_sig,
        body_sig,
        body: body.unwrap_or_else(|| KEEP_BODY.to_string()),
        body_state,
    }
}

fn write_fts(
    tx: &Connection,
    rowid: i64,
    title: &str,
    folder: &str,
    tags: &[String],
    summary: &str,
    body: &str,
) -> Result<(), String> {
    tx.execute("DELETE FROM library_fts WHERE rowid = ?1", params![rowid])
        .map_err(|e| format!("fts delete: {e}"))?;
    tx.execute(
        "INSERT INTO library_fts(rowid, title, folder, tags, summary, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            rowid,
            segment_cjk(title),
            segment_cjk(folder),
            segment_cjk(&tags.join(" ")),
            segment_cjk(summary),
            segment_cjk(body),
        ],
    )
    .map_err(|e| format!("fts insert: {e}"))?;
    Ok(())
}

fn upsert(tx: &Connection, row: &Row) -> Result<(), String> {
    let keep_body = row.body == KEEP_BODY;
    let tags_json = serde_json::to_string(&row.tags).unwrap_or_else(|_| "[]".into());
    // ON CONFLICT keeps the row id stable — it is also the FTS rowid — and
    // leaves the summarizer's claim/failure columns alone.
    tx.execute(
        "INSERT INTO items(key, kind, item_id, dir, project, workspace_path, folder, location,
                           title, summary, tags, updated_ms, created_ms, artifact_kind, version,
                           summary_version, meta_sig, body_sig, body, body_state, filed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 CASE WHEN ?20 THEN '' ELSE ?19 END, ?21, ?22)
         ON CONFLICT(key) DO UPDATE SET
             kind = excluded.kind, item_id = excluded.item_id, dir = excluded.dir,
             project = excluded.project, workspace_path = excluded.workspace_path,
             folder = excluded.folder, filed = excluded.filed, location = excluded.location, title = excluded.title,
             summary = excluded.summary, tags = excluded.tags, updated_ms = excluded.updated_ms,
             created_ms = excluded.created_ms, artifact_kind = excluded.artifact_kind,
             version = excluded.version, summary_version = excluded.summary_version,
             meta_sig = excluded.meta_sig, body_sig = excluded.body_sig,
             body = CASE WHEN ?20 THEN items.body ELSE excluded.body END,
             body_state = CASE WHEN ?20 THEN items.body_state ELSE excluded.body_state END",
        params![
            row.key,
            row.kind,
            row.item_id,
            row.dir,
            row.project,
            row.workspace_path,
            row.folder,
            row.location,
            row.title,
            row.summary,
            tags_json,
            row.updated_ms as i64,
            row.created_ms as i64,
            row.artifact_kind,
            row.version,
            row.summary_version,
            row.meta_sig,
            row.body_sig,
            row.body,
            keep_body,
            row.body_state,
            row.filed,
        ],
    )
    .map_err(|e| format!("items upsert: {e}"))?;
    let (rowid, body): (i64, String) = tx
        .query_row(
            "SELECT id, body FROM items WHERE key = ?1",
            params![row.key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("items lookup: {e}"))?;
    write_fts(tx, rowid, &row.title, &row.folder, &row.tags, &row.summary, &body)
}

fn set_folder(tx: &Connection, id: i64, folder: &str) -> Result<(), String> {
    let (title, tags, summary, body): (String, String, String, String) = tx
        .query_row(
            "SELECT title, tags, summary, body FROM items WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| format!("items lookup: {e}"))?;
    tx.execute("UPDATE items SET folder = ?2 WHERE id = ?1", params![id, folder])
        .map_err(|e| format!("items refile: {e}"))?;
    let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
    write_fts(tx, id, &title, folder, &tags, &summary, &body)
}

fn delete_row(tx: &Connection, id: i64) -> Result<(), String> {
    tx.execute("DELETE FROM library_fts WHERE rowid = ?1", params![id])
        .map_err(|e| format!("fts delete: {e}"))?;
    tx.execute("DELETE FROM items WHERE id = ?1", params![id])
        .map_err(|e| format!("items delete: {e}"))?;
    Ok(())
}

// ── Entry points ─────────────────────────────────────────────────────────────

/// Time a search may spend extracting artifact text the index has not seen
/// yet. The rest is picked up by later searches or the background worker.
const SEARCH_EXTRACT_BUDGET: Duration = Duration::from_millis(1500);

/// Sync, then search — the call every surface makes.
pub fn search(query: &str, opts: &SearchOpts) -> Result<Vec<LibraryHit>, String> {
    search_in(&Roots::home()?, query, opts)
}

pub fn search_in(roots: &Roots, query: &str, opts: &SearchOpts) -> Result<Vec<LibraryHit>, String> {
    let index = LibraryIndex::open_at(&db_path(roots))?;
    index.sync(roots)?;
    index.extract_pending(roots, SEARCH_EXTRACT_BUDGET)?;
    index.search(query, opts)
}

#[cfg(test)]
#[path = "library_index_tests.rs"]
mod tests;
