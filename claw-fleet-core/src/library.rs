//! 资料 (Library) — the wiki ([`crate::wiki`]) and the artifact store
//! ([`crate::artifacts`]) presented as one folder tree per project, and the
//! per-project folder schema that tree hangs on.
//!
//! Both stores stay exactly as they are. What this module adds is the one thing
//! neither had: a list of folders a project is *supposed* to be organized
//! into, each with a one-line description of what belongs there. A technical
//! research project and a personal invoice project organize differently, so
//! the schema is per project rather than a global taxonomy.
//!
//! ## Mapping the two stores onto one folder
//!
//! A schema folder has a user-facing `path` (`/`-separated, any script —
//! `发票/2026` is as valid as `design/notes`). Each store files an item into it
//! its own way:
//!
//! - **Artifacts** carry a free-text `path` already, so an artifact is in folder
//!   `F` when `artifact.path == F`. Only the explicit `path` counts: the
//!   directory [`crate::artifacts::effective_directory`] derives from where the
//!   producing agent wrote the file is exactly the "agents put things anywhere"
//!   shape this tree exists to fix, and treating it as filing would make an
//!   artifact impossible to unfile whenever its source directory happened to
//!   share a folder's name. It is still reported, as the item's `location`.
//! - **Wiki docs** have no folder field — a folder is the slug's directory part
//!   — and slugs are lowercase ASCII. So every folder also has a derived
//!   [`LibraryFolder::wiki_prefix`]: one ASCII segment per path segment. A doc
//!   is in `F` when its slug's directory equals `F`'s prefix.
//!
//!   Each segment comes from that folder's own [`LibraryFolder::key`] when it
//!   has one (`发票` with key `invoices`), else from [`segment_key`] of its
//!   name. Keys exist because a hash is the only thing `segment_key` can make
//!   of a CJK name, and `f-1a2b3c/receipt-0915` is not a slug anyone can read.
//!   The upload organizer writes a key for every folder it drafts; a folder
//!   made by hand without one keeps the derived segment.
//!
//!   Every prefix starts with the project's [namespace](project_namespace_in)
//!   (`dayday/research`, not `research`): slugs are global across projects,
//!   and the first re-filed project took bare `research/…` and `design/…`, so
//!   every later project with a research folder would have collided with it
//!   or got `-2` slugs. Docs filed before namespacing are moved onto the
//!   namespaced prefix by [`migrate_namespaces_in`].
//!
//! An item whose folder is not in the schema is reported with `folder: None`
//! and shows under the tree's "unfiled" bucket. Nothing here invents a folder
//! from where an item happens to sit — that is how the tree got messy.
//!
//! ## Project identity
//!
//! The schema is keyed by [`project_key`]: the git checkout root, so a repo and
//! all its worktrees (`.worktrees/<id>`, Fleet's `~/.fleet/worktrees/…`) share
//! one tree — the same rule [`crate::wiki::workspace_contains`] uses. Outside a
//! checkout the workspace path itself is the key.
//!
//! ## On-disk layout
//!
//! Scan-dir, one file per folder, for the reason the wiki and artifact modules
//! give: several agents (and, later, the AI organizer) may write at once, and
//! a single shared file would make every one of them contend on it.
//!
//! ```text
//! ~/.fleet/library/projects/<h(project key)>/
//!   project.json               # ProjectRecord: the project's slug namespace
//!   folders/<h(path)>.json     # LibraryFolder
//!   schema.lock                # serializes multi-file edits (create with
//!                              # ancestors, rename of a subtree)
//! ```
//!
//! Paths are hashed into file names because a folder path can be 16 levels of
//! 64-char CJK names — far past a file name's 255 bytes. The real path lives in
//! the JSON.
//!
//! ## The artifact store's old `folders.json`
//!
//! Before the schema, the Artifacts page kept user-made folders in one
//! `~/.fleet/artifacts/folders.json`. That file is folded in: whenever it
//! exists, its folders are imported (missing ones only) and it is renamed to
//! `folders.json.imported`. "Whenever", not "once": an older build still
//! running would recreate the file, and a one-shot marker would silently drop
//! what it wrote. The `artifacts::*_folder` functions are now wrappers over
//! this module, so the Artifacts page, the phone and `fleet serve` all read and
//! write the schema.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifacts::{self, is_at_or_under, repath, Artifact};
use crate::wiki::{self, WikiDoc};

/// `LibraryItem::kind` of a wiki document.
pub const ITEM_DOC: &str = "doc";
/// `LibraryItem::kind` of an artifact.
pub const ITEM_ARTIFACT: &str = "artifact";

/// Deepest a folder may nest. A wiki slug holds at most
/// [`wiki::MAX_SLUG_DEPTH`] segments and the project namespace and the doc's
/// own name are two of them, so a deeper folder could hold artifacts but never
/// a document.
pub const MAX_FOLDER_DEPTH: usize = wiki::MAX_SLUG_DEPTH - 2;

/// Longest a folder's wiki prefix may be. Leaves room under
/// [`wiki::MAX_SLUG_LEN`] for a `/` and a reasonable doc name.
const MAX_PREFIX_LEN: usize = wiki::MAX_SLUG_LEN - 32;

// ── Types ────────────────────────────────────────────────────────────────────

/// [`FolderOrigin::source`] of a folder an agent created while filing a doc or
/// an artifact.
pub const ORIGIN_AGENT: &str = "agent";
/// [`FolderOrigin::source`] of a folder an upload's classifier created.
pub const ORIGIN_UPLOAD: &str = "upload";
/// [`FolderOrigin::source`] of a folder re-filing a project's existing items
/// created (`library_ingest::refile`).
pub const ORIGIN_REFILE: &str = "refile";

/// Who made a folder the user did not make, and why. A folder carrying one
/// that is not yet [`accepted`](FolderOrigin::accepted_ms) is shown with a
/// marker in the tree until the user accepts, renames or deletes it — the
/// schema is the user's, and an AI growing it must be visible.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderOrigin {
    /// [`ORIGIN_AGENT`], [`ORIGIN_UPLOAD`] or [`ORIGIN_REFILE`].
    pub source: String,
    /// Why no existing folder fitted, in the creator's words.
    #[serde(default)]
    pub reason: String,
    /// The agent session that created it.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The upload or re-file batch that created it.
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub created_ms: u64,
    /// When the user accepted it; `None` = still marked as AI-created.
    #[serde(default)]
    pub accepted_ms: Option<u64>,
}

impl FolderOrigin {
    pub fn agent(session_id: Option<&str>, reason: &str) -> Self {
        Self {
            source: ORIGIN_AGENT.to_string(),
            reason: reason.trim().to_string(),
            session_id: session_id.filter(|s| !s.is_empty()).map(str::to_string),
            created_ms: now_ms(),
            ..Default::default()
        }
    }

    pub fn upload(batch_id: &str, reason: &str) -> Self {
        Self {
            source: ORIGIN_UPLOAD.to_string(),
            reason: reason.trim().to_string(),
            batch_id: Some(batch_id.to_string()),
            created_ms: now_ms(),
            ..Default::default()
        }
    }

    pub fn refile(batch_id: &str, reason: &str) -> Self {
        Self { source: ORIGIN_REFILE.to_string(), ..Self::upload(batch_id, reason) }
    }
}

/// One folder of a project's schema.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    /// The project ([`project_key`]) the folder belongs to.
    pub workspace_path: String,
    /// Normalized `/`-separated path, never empty.
    pub path: String,
    /// One line on what belongs here. Shown to the user beside the folder and,
    /// later, to the agents and the organizer that file into it.
    #[serde(default)]
    pub description: String,
    /// Readable ASCII slug segment for this folder's own (last) path segment,
    /// e.g. `invoices` for `发票`. Empty = derive it with [`segment_key`].
    /// Set once at creation; a rename keeps it, so docs filed here keep their
    /// slugs when the display name changes.
    #[serde(default)]
    pub key: String,
    /// Where wiki docs filed here live: the parent's prefix plus this folder's
    /// key. Derived on every read; the persisted copy is informational only.
    #[serde(default)]
    pub wiki_prefix: String,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
    /// Set when an agent or an upload created the folder; `None` = the user
    /// made it (or it predates origins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<FolderOrigin>,
}

impl LibraryFolder {
    /// Created by an AI and not yet accepted by the user.
    pub fn awaiting_review(&self) -> bool {
        self.origin.as_ref().is_some_and(|o| o.accepted_ms.is_none())
    }
}

/// One wiki doc or artifact, placed in its project's tree.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    /// [`ITEM_DOC`] or [`ITEM_ARTIFACT`].
    pub kind: String,
    /// The doc's slug or the artifact's id.
    pub id: String,
    pub title: String,
    /// The workspace recorded on the item itself (may be a worktree).
    pub workspace_path: String,
    /// Schema folder path the item is filed in; `None` = unfiled.
    #[serde(default)]
    pub folder: Option<String>,
    /// Where the item actually sits in its own store: a doc's slug directory,
    /// an artifact's effective directory. For an unfiled item this is the hint
    /// of where it came from.
    #[serde(default)]
    pub location: String,
    pub updated_ms: u64,
    /// The artifact's coarse kind (`pdf`, `image`, …); empty for docs.
    #[serde(default)]
    pub artifact_kind: String,
    /// The AI summary (a doc's `summary.json`, an artifact's `meta.json`), so
    /// a folder's listing can say what each item is without opening it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The artifact's file name, mime and size — what a client needs to build
    /// its blob URL and decide whether a thumbnail is worth fetching. Empty
    /// for docs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mime: String,
    #[serde(default, skip_serializing_if = "is_zero_size")]
    pub size_bytes: u64,
}

fn is_zero_size(v: &u64) -> bool {
    *v == 0
}

/// One project's tree: its schema folders and every item, filed or not.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryProject {
    /// The project key.
    pub workspace_path: String,
    pub workspace_name: String,
    /// Sorted by path, so a parent always precedes its children.
    pub folders: Vec<LibraryFolder>,
    /// Newest first. In an [`outline`] only the newest few per folder, with
    /// no summary or tags.
    pub items: Vec<LibraryItem>,
    /// Set only by [`outline`]: how many items are filed directly in each
    /// folder path, `""` keying the unfiled ones. Absent from a full tree,
    /// where `items` is the count.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, usize>,
}

/// What a folder rename re-filed.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderRename {
    pub moved_docs: usize,
    pub moved_artifacts: usize,
}

/// The three store roots, so every operation is testable against temp dirs.
#[derive(Clone, Debug)]
pub struct Roots {
    pub library: PathBuf,
    pub wiki: PathBuf,
    pub artifacts: PathBuf,
}

impl Roots {
    /// The real stores under `~/.fleet`.
    pub fn home() -> Result<Self, String> {
        let fleet = crate::session::get_fleet_dir()
            .ok_or_else(|| "cannot determine home dir".to_string())?;
        Ok(Self {
            library: fleet.join("library"),
            wiki: fleet.join("wiki"),
            artifacts: fleet.join("artifacts"),
        })
    }
}

// ── Keys ─────────────────────────────────────────────────────────────────────

/// The project a workspace belongs to: its git checkout root (worktrees
/// resolve to the main checkout), or the resolved path itself outside a repo.
pub fn project_key(workspace: &str) -> String {
    if workspace.trim().is_empty() {
        return String::new();
    }
    let resolved = wiki::resolve_workspace_path(Path::new(workspace));
    match wiki::repo_root(Path::new(&resolved)) {
        Some(root) => root.display().to_string(),
        None => resolved,
    }
}

fn short_hash(s: &str, hex_len: usize) -> String {
    let digest = Sha256::digest(s.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..hex_len].to_string()
}

/// The wiki slug segment a folder name maps to.
///
/// A plain ASCII name maps to its slug form (`Design Notes` → `design-notes`),
/// so a technical project's docs keep readable slugs. Anything else — `发票`,
/// `2026 发票` — cannot be spelled in a slug, so it maps to whatever ASCII it
/// has plus a short hash of the full name (`f-1a2b3c`, `2026-9f8e7d`): stable,
/// so the same folder name always lands on the same prefix, and distinct, so
/// `2026 发票` and `2026 收据` do not share one.
pub fn segment_key(segment: &str) -> String {
    let ascii = wiki::normalize_segment(segment);
    if segment.is_ascii() && !ascii.is_empty() {
        return ascii.chars().take(wiki::MAX_SLUG_SEGMENT).collect();
    }
    let hash = short_hash(segment, 6);
    if ascii.is_empty() {
        format!("f-{hash}")
    } else {
        let head: String = ascii.chars().take(wiki::MAX_SLUG_SEGMENT - 7).collect();
        format!("{}-{hash}", head.trim_end_matches('-'))
    }
}

/// A folder path's wiki prefix when no folder on it has a key: [`segment_key`]
/// per segment. [`prefix_in`] is the schema-aware version.
pub fn wiki_prefix(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(segment_key)
        .collect::<Vec<_>>()
        .join("/")
}

/// Longest a folder key may be. Shorter than a slug segment allows, so a few
/// nested keys still leave room for the doc's own name.
pub const MAX_KEY_LEN: usize = 32;

/// Canonical form of a proposed folder key, or `None` when nothing usable is
/// left (`发票` → `None`, `Invoices 2026` → `invoices-2026`).
pub fn normalize_key(raw: &str) -> Option<String> {
    let k: String = wiki::normalize_segment(raw).chars().take(MAX_KEY_LEN).collect();
    let k = k.trim_matches('-').to_string();
    (!k.is_empty()).then_some(k)
}

/// The key a folder gets when its creator named none: the pinyin of a CJK
/// name (`发票` → `fapiao`, `2026 发票` → `2026-fapiao`), so docs filed there
/// get slugs a person can read. Empty for a plain ASCII name, which
/// [`segment_key`] already spells readably, and for a name with nothing
/// pinyin can render.
pub fn auto_key(segment: &str) -> String {
    use pinyin::ToPinyin;
    if segment.is_ascii() {
        return String::new();
    }
    // A run of hanzi becomes one word of joined syllables (`fapiao`), a run of
    // ASCII letters/digits stays a word, anything else separates words.
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_is_hanzi = false;
    for c in segment.chars() {
        let (piece, hanzi) = match c.to_pinyin() {
            Some(p) => (p.plain().to_string(), true),
            None if c.is_ascii_alphanumeric() => (c.to_ascii_lowercase().to_string(), false),
            None => (String::new(), false),
        };
        if piece.is_empty() || (!cur.is_empty() && hanzi != cur_is_hanzi) {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        }
        cur.push_str(&piece);
        cur_is_hanzi = hanzi;
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    normalize_key(&words.join(" ")).unwrap_or_default()
}

fn leaf_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[i + 1..]).unwrap_or(path)
}

/// `path`'s wiki prefix within `folders` of the project whose namespace is
/// `ns`: the namespace, then per segment its folder's [`LibraryFolder::key`]
/// when that folder is in the list and has one, else [`segment_key`] of the
/// segment's name. An empty `ns` gives the prefix as it was before namespacing.
fn prefix_in(ns: &str, folders: &[LibraryFolder], path: &str) -> String {
    let by_path: HashMap<&str, &LibraryFolder> =
        folders.iter().map(|f| (f.path.as_str(), f)).collect();
    prefix_with(ns, &by_path, path)
}

fn prefix_with(ns: &str, by_path: &HashMap<&str, &LibraryFolder>, path: &str) -> String {
    let rel = prefixes(path)
        .iter()
        .map(|p| match by_path.get(p.as_str()).filter(|f| !f.key.is_empty()) {
            Some(f) => f.key.clone(),
            None => segment_key(leaf_of(p)),
        })
        .collect::<Vec<_>>()
        .join("/");
    if ns.is_empty() {
        rel
    } else {
        format!("{ns}/{rel}")
    }
}

/// Fill in every folder's `wiki_prefix` from the list itself.
fn assign_prefixes(ns: &str, folders: &mut [LibraryFolder]) {
    let computed: Vec<String> = {
        let by_path: HashMap<&str, &LibraryFolder> =
            folders.iter().map(|f| (f.path.as_str(), f)).collect();
        folders.iter().map(|f| prefix_with(ns, &by_path, &f.path)).collect()
    };
    for (f, p) in folders.iter_mut().zip(computed) {
        f.wiki_prefix = p;
    }
}

/// A folder's wiki prefix without the project namespace — what it was before
/// namespacing, and what agents used to write as a slug's directory.
pub(crate) fn rel_prefix(folder: &LibraryFolder) -> &str {
    folder
        .wiki_prefix
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(&folder.wiki_prefix)
}

/// The schema folder a slug directory names: its prefix exactly, else — for
/// the habit of writing `research/memo` from before namespacing — the prefix
/// without the namespace.
pub(crate) fn folder_by_dir<'a>(folders: &'a [LibraryFolder], dir: &str) -> Option<&'a LibraryFolder> {
    if dir.is_empty() {
        return None;
    }
    folders
        .iter()
        .find(|f| f.wiki_prefix == dir)
        .or_else(|| folders.iter().find(|f| rel_prefix(f) == dir))
}

// ── Project namespace ───────────────────────────────────────────────────────

/// A project's persisted identity in the library: the namespace its docs'
/// slugs start with.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    /// The project key ([`project_key`]).
    pub workspace_path: String,
    /// First slug segment of every doc filed in the project's folders.
    pub namespace: String,
    #[serde(default)]
    pub assigned_ms: u64,
}

fn project_record_file(roots: &Roots, key: &str) -> PathBuf {
    project_dir(roots, key).join("project.json")
}

fn read_project_record(roots: &Roots, key: &str) -> Option<ProjectRecord> {
    let raw = fs::read_to_string(project_record_file(roots, key)).ok()?;
    serde_json::from_str::<ProjectRecord>(&raw)
        .ok()
        .filter(|r| r.workspace_path == key && !r.namespace.is_empty())
}

/// What a namespace is spelled from: the project's name (the repo's directory
/// name, which is what most projects' docs were already prefixed with by hand
/// — `human-zoo/…`, `suikou/…`), as a lowercase ASCII slug segment; a CJK
/// name becomes pinyin.
fn namespace_base(key: &str) -> String {
    let name = wiki::workspace_name_of(key);
    let ascii = if name.is_ascii() { normalize_key(&name) } else { normalize_key(&auto_key(&name)) };
    ascii.unwrap_or_else(|| format!("p-{}", short_hash(key, 6)))
}

/// Namespaces to try, in order: the name, then the name under its parent
/// directory's name, then the name with a hash of the key.
fn namespace_candidates(key: &str) -> Vec<String> {
    let base = namespace_base(key);
    let mut out = vec![base.clone()];
    let parent = Path::new(key)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| normalize_key(&n.to_string_lossy()));
    if let Some(parent) = parent {
        if let Some(k) = normalize_key(&format!("{parent}-{base}")) {
            out.push(k);
        }
    }
    let head: String = base.chars().take(MAX_KEY_LEN - 7).collect();
    out.push(format!("{}-{}", head.trim_end_matches('-'), short_hash(key, 6)));
    out
}

/// The slug namespace of project `key` — the first segment of every folder's
/// wiki prefix.
///
/// Assigned once and then persisted, so it never changes under a project's
/// docs: the project's name when no other project holds it, else a longer
/// spelling (see [`namespace_candidates`]). Persisting is what keeps it stable
/// when a second project with the same directory name shows up later — a
/// rule derived afresh on every read would rename the first one's docs. One
/// repo and all its worktrees are one key, so they share a namespace.
pub fn project_namespace_in(roots: &Roots, key: &str) -> String {
    if let Some(r) = read_project_record(roots, key) {
        return r.namespace;
    }
    let assign = || -> Result<String, String> {
        if let Some(r) = read_project_record(roots, key) {
            return Ok(r.namespace);
        }
        let taken: HashSet<String> = fs::read_dir(projects_dir(roots))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| fs::read_to_string(e.path().join("project.json")).ok())
            .filter_map(|raw| serde_json::from_str::<ProjectRecord>(&raw).ok())
            .filter(|r| r.workspace_path != key)
            .map(|r| r.namespace)
            .collect();
        let candidates = namespace_candidates(key);
        let namespace = candidates
            .iter()
            .find(|c| !taken.contains(*c))
            .cloned()
            .unwrap_or_else(|| format!("p-{}", short_hash(key, 12)));
        let record = ProjectRecord { workspace_path: key.to_string(), namespace, assigned_ms: now_ms() };
        let body = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
        crate::atomic_json::write_atomic(&project_record_file(roots, key), &body)
            .map_err(|e| format!("write project record: {e}"))?;
        Ok(record.namespace)
    };
    // One lock for the whole store: two projects with the same name assigning
    // at once must not both take it.
    match crate::atomic_json::with_file_lock(&projects_dir(roots).join("namespaces"), assign) {
        Ok(ns) => ns,
        Err(e) => {
            // Unwritable store: fall back to the name, unpersisted.
            crate::log_debug(&format!("[library] namespace for {key}: {e}"));
            namespace_base(key)
        }
    }
}

pub fn project_namespace(workspace: &str) -> Result<String, String> {
    let roots = Roots::home()?;
    Ok(project_namespace_in(&roots, &checked_key(workspace)?))
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..i]).unwrap_or("")
}

/// The directory part of a wiki slug (`""` for a doc at the root).
pub(crate) fn slug_dir(slug: &str) -> &str {
    parent_of(slug)
}

/// Every ancestor of `path` and `path` itself, shallowest first.
fn prefixes(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    for seg in path.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        out.push(acc.clone());
    }
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Storage ──────────────────────────────────────────────────────────────────

fn projects_dir(roots: &Roots) -> PathBuf {
    roots.library.join("projects")
}

fn project_dir(roots: &Roots, key: &str) -> PathBuf {
    projects_dir(roots).join(short_hash(key, 16))
}

fn folder_file(project: &Path, path: &str) -> PathBuf {
    project
        .join("folders")
        .join(format!("{}.json", short_hash(path, 16)))
}

fn with_project_lock<R>(roots: &Roots, key: &str, f: impl FnOnce() -> R) -> R {
    crate::atomic_json::with_file_lock(&project_dir(roots, key).join("schema"), f)
}

/// The folder records in one project directory, prefixes not yet assigned.
fn read_folders_dir(project: &Path) -> Vec<LibraryFolder> {
    let Ok(entries) = fs::read_dir(project.join("folders")) else {
        return Vec::new();
    };
    let mut out: Vec<LibraryFolder> = entries
        .flatten()
        .map(|e| e.path())
        // Only finished files: atomic writes leave `<name>.json.tmp.<pid>.<n>`
        // siblings while in flight.
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| fs::read_to_string(&p).ok())
        .filter_map(|raw| serde_json::from_str::<LibraryFolder>(&raw).ok())
        .filter(|f| !f.path.is_empty())
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// One project's schema, without the legacy import.
fn project_folders(roots: &Roots, key: &str) -> Vec<LibraryFolder> {
    let mut out: Vec<LibraryFolder> = read_folders_dir(&project_dir(roots, key))
        .into_iter()
        // A hash collision between two project keys is astronomically
        // unlikely, but a folder must never show up under the wrong project.
        .filter(|f| f.workspace_path == key)
        .collect();
    if !out.is_empty() {
        assign_prefixes(&project_namespace_in(roots, key), &mut out);
    }
    out
}

fn write_folder(roots: &Roots, folder: &LibraryFolder) -> Result<(), String> {
    let file = folder_file(&project_dir(roots, &folder.workspace_path), &folder.path);
    // `wiki_prefix` is stored as the caller computed it — informational only,
    // `read_folders_dir` derives it again from the keys.
    let body = serde_json::to_vec_pretty(folder).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&file, &body)
        .map_err(|e| format!("write folder '{}': {e}", folder.path))
}

fn remove_folder_file(roots: &Roots, key: &str, path: &str) -> Result<(), String> {
    let file = folder_file(&project_dir(roots, key), path);
    match fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove folder '{path}': {e}")),
    }
}

/// Fold the artifact store's legacy `folders.json` into the schema. See the
/// module docs for why this runs whenever the file exists, not once.
pub fn import_legacy_artifact_folders(roots: &Roots) {
    let legacy = roots.artifacts.join("folders.json");
    if !legacy.exists() {
        return;
    }
    let folders = match crate::atomic_json::load_preserving::<Vec<artifacts::Folder>>(&legacy) {
        crate::atomic_json::JsonLoad::Loaded(list) => list,
        // Unreadable: try again next time. Corrupt: already renamed aside.
        _ => return,
    };
    let mut by_project: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in folders {
        let Ok(path) = artifacts::normalize_dir_path(&f.path) else {
            continue;
        };
        let key = project_key(&f.workspace_path);
        if path.is_empty() || key.is_empty() {
            continue;
        }
        by_project.entry(key).or_default().push(path);
    }
    for (key, paths) in by_project {
        let result = with_project_lock(roots, &key, || -> Result<(), String> {
            let mut have: HashSet<String> =
                project_folders(roots, &key).into_iter().map(|f| f.path).collect();
            let now = now_ms();
            for path in paths {
                for p in prefixes(&path) {
                    if have.insert(p.clone()) {
                        write_folder(
                            roots,
                            &LibraryFolder {
                                workspace_path: key.clone(),
                                path: p,
                                description: String::new(),
                                created_ms: now,
                                updated_ms: now,
                                ..Default::default()
                            },
                        )?;
                    }
                }
            }
            Ok(())
        });
        if let Err(e) = result {
            // Leave the legacy file in place so the next read retries.
            eprintln!("library: importing artifact folders for {key} failed: {e}");
            return;
        }
    }
    let _ = fs::rename(&legacy, roots.artifacts.join("folders.json.imported"));
}

// ── Reads ────────────────────────────────────────────────────────────────────

/// One project's schema folders, sorted by path.
pub fn list_folders(workspace: &str) -> Result<Vec<LibraryFolder>, String> {
    list_folders_in(&Roots::home()?, workspace)
}

pub fn list_folders_in(roots: &Roots, workspace: &str) -> Result<Vec<LibraryFolder>, String> {
    import_legacy_artifact_folders(roots);
    let key = project_key(workspace);
    if key.is_empty() {
        return Err("a workspace is required".to_string());
    }
    Ok(project_folders(roots, &key))
}

/// Every project's schema folders.
pub(crate) fn all_folders(roots: &Roots) -> Vec<LibraryFolder> {
    let Ok(entries) = fs::read_dir(projects_dir(roots)) else {
        return Vec::new();
    };
    let keys: BTreeMap<String, ()> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .flat_map(|e| read_folders_dir(&e.path()))
        .map(|f| (f.workspace_path, ()))
        .collect();
    let mut out: Vec<LibraryFolder> =
        keys.into_keys().flat_map(|k| project_folders(roots, &k)).collect();
    out.sort_by(|a, b| {
        a.workspace_path
            .cmp(&b.workspace_path)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Memoized [`project_key`] — it walks the filesystem, and a store holds many
/// items per workspace.
#[derive(Default)]
struct KeyCache(HashMap<String, String>);

impl KeyCache {
    fn key(&mut self, workspace: &str) -> String {
        if let Some(k) = self.0.get(workspace) {
            return k.clone();
        }
        let k = project_key(workspace);
        self.0.insert(workspace.to_string(), k.clone());
        k
    }
}

fn doc_item(doc: &WikiDoc, folder: Option<String>) -> LibraryItem {
    LibraryItem {
        kind: ITEM_DOC.to_string(),
        id: doc.slug.clone(),
        title: if doc.title.is_empty() {
            wiki::slug_basename(&doc.slug).to_string()
        } else {
            doc.title.clone()
        },
        workspace_path: doc.workspace_path.clone(),
        folder,
        location: slug_dir(&doc.slug).to_string(),
        updated_ms: doc.updated_ms.max(doc.created_ms),
        artifact_kind: String::new(),
        summary: doc.summary.clone(),
        tags: doc.tags.clone(),
        name: String::new(),
        mime: String::new(),
        size_bytes: 0,
    }
}

fn artifact_item(a: &Artifact, folder: Option<String>) -> LibraryItem {
    LibraryItem {
        kind: ITEM_ARTIFACT.to_string(),
        id: a.id.clone(),
        title: if a.title.is_empty() {
            a.name.clone()
        } else {
            a.title.clone()
        },
        workspace_path: a.workspace_path.clone(),
        folder,
        location: artifacts::effective_directory(a),
        updated_ms: a.created_ms,
        artifact_kind: a.kind.clone(),
        summary: a.summary.clone(),
        tags: a.tags.clone(),
        name: a.name.clone(),
        mime: a.mime.clone(),
        size_bytes: a.size_bytes,
    }
}

/// The schema folder a doc is filed in, by its slug directory.
pub(crate) fn doc_folder(folders: &[LibraryFolder], slug: &str) -> Option<String> {
    let dir = slug_dir(slug);
    if dir.is_empty() {
        return None;
    }
    folders
        .iter()
        .find(|f| f.wiki_prefix == dir)
        .map(|f| f.path.clone())
}

/// The schema folder an artifact is filed in, by its explicit `path`.
pub(crate) fn artifact_folder(folders: &[LibraryFolder], artifact: &Artifact) -> Option<String> {
    if artifact.path.is_empty() {
        return None;
    }
    folders
        .iter()
        .find(|f| f.path == artifact.path)
        .map(|f| f.path.clone())
}

/// The merged tree: every project (or just `workspace`'s), with its schema
/// folders and every wiki doc and artifact placed in them or left unfiled.
pub fn tree(workspace: Option<&str>) -> Result<Vec<LibraryProject>, String> {
    Ok(tree_in(&Roots::home()?, workspace))
}

pub fn tree_in(roots: &Roots, workspace: Option<&str>) -> Vec<LibraryProject> {
    import_legacy_artifact_folders(roots);
    let mut keys = KeyCache::default();
    let only = workspace
        .filter(|w| !w.trim().is_empty())
        .map(|w| keys.key(w));

    let mut projects: BTreeMap<String, LibraryProject> = BTreeMap::new();
    let ensure = |key: &str, projects: &mut BTreeMap<String, LibraryProject>| {
        projects
            .entry(key.to_string())
            .or_insert_with(|| LibraryProject {
                workspace_path: key.to_string(),
                workspace_name: wiki::workspace_name_of(key),
                folders: Vec::new(),
                items: Vec::new(),
                counts: BTreeMap::new(),
            });
    };

    for f in all_folders(roots) {
        if only.as_ref().is_some_and(|k| *k != f.workspace_path) {
            continue;
        }
        ensure(&f.workspace_path.clone(), &mut projects);
        if let Some(p) = projects.get_mut(&f.workspace_path) {
            p.folders.push(f);
        }
    }

    for doc in wiki::list_docs_in(&roots.wiki) {
        let key = keys.key(&doc.workspace_path);
        if key.is_empty() || only.as_ref().is_some_and(|k| *k != key) {
            continue;
        }
        ensure(&key, &mut projects);
        let p = projects.get_mut(&key).expect("just ensured");
        let folder = doc_folder(&p.folders, &doc.slug);
        p.items.push(doc_item(&doc, folder));
    }

    for a in artifacts::list_in(&roots.artifacts) {
        let key = keys.key(&a.workspace_path);
        if key.is_empty() || only.as_ref().is_some_and(|k| *k != key) {
            continue;
        }
        ensure(&key, &mut projects);
        let p = projects.get_mut(&key).expect("just ensured");
        let folder = artifact_folder(&p.folders, &a);
        p.items.push(artifact_item(&a, folder));
    }

    let mut out: Vec<LibraryProject> = projects.into_values().collect();
    for p in &mut out {
        p.items.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    out.sort_by(|a, b| {
        a.workspace_name
            .cmp(&b.workspace_name)
            .then_with(|| a.workspace_path.cmp(&b.workspace_path))
    });
    out
}

/// Items an [`outline`] keeps per folder by default — what a project
/// overview card lists under each folder.
pub const OUTLINE_RECENT: usize = 3;

/// The tree without the bulk: every project's (or `workspace`'s) folders,
/// how many items each folder holds directly ([`LibraryProject::counts`]),
/// and only the newest `recent` items of each folder, stripped of summary and
/// tags.
///
/// This is what the phone draws its project list and project overview from.
/// The full tree is ~1.4 MB on a real library (2,000 items, most of it
/// summaries); a folder's items, summaries included, come from
/// [`folder_items`] when it is opened. The newest few per folder are enough
/// for any folder's "newest few beneath it", since those are each among the
/// newest few of their own folder.
pub fn outline(workspace: Option<&str>, recent: usize) -> Result<Vec<LibraryProject>, String> {
    Ok(outline_in(&Roots::home()?, workspace, recent))
}

pub fn outline_in(roots: &Roots, workspace: Option<&str>, recent: usize) -> Vec<LibraryProject> {
    let mut out = tree_in(roots, workspace);
    for p in &mut out {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let items = std::mem::take(&mut p.items);
        for mut item in items {
            let n = counts.entry(item.folder.clone().unwrap_or_default()).or_insert(0);
            *n += 1;
            // Items arrive newest first, so the first `recent` per folder win.
            if *n <= recent {
                item.summary = String::new();
                item.tags = Vec::new();
                p.items.push(item);
            }
        }
        p.counts = counts;
    }
    out
}

/// Every item of `workspace`'s project filed in `folder` or beneath it,
/// newest first, with summaries and tags; `folder` `""` answers the unfiled
/// ones. The listing a phone fetches when a folder is opened (see
/// [`outline`]).
pub fn folder_items(workspace: &str, folder: &str) -> Result<Vec<LibraryItem>, String> {
    folder_items_in(&Roots::home()?, workspace, folder)
}

pub fn folder_items_in(
    roots: &Roots,
    workspace: &str,
    folder: &str,
) -> Result<Vec<LibraryItem>, String> {
    checked_key(workspace)?;
    let folder = folder.trim().trim_matches('/');
    let items = tree_in(roots, Some(workspace))
        .into_iter()
        .next()
        .map(|p| p.items)
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .filter(|i| match i.folder.as_deref() {
            None => folder.is_empty(),
            Some(f) => !folder.is_empty() && is_at_or_under(f, folder),
        })
        .collect())
}

// ── Folder edits ─────────────────────────────────────────────────────────────

fn checked_path(raw: &str) -> Result<String, String> {
    let path = artifacts::normalize_dir_path(raw)?;
    if path.is_empty() {
        return Err("a folder needs a name".to_string());
    }
    let depth = path.split('/').count();
    if depth > MAX_FOLDER_DEPTH {
        return Err(format!(
            "folder path is more than {MAX_FOLDER_DEPTH} levels deep"
        ));
    }
    if wiki_prefix(&path).len() > MAX_PREFIX_LEN {
        return Err(format!("folder path '{path}' is too long"));
    }
    Ok(path)
}

fn checked_key(workspace: &str) -> Result<String, String> {
    let key = project_key(workspace);
    if key.is_empty() {
        Err("a workspace is required".to_string())
    } else {
        Ok(key)
    }
}

/// Refuse a folder whose wiki prefix (`prefix`) another folder already maps
/// to — the two would silently share their documents.
fn ensure_prefix_free(
    folders: &[LibraryFolder],
    path: &str,
    prefix: &str,
    ignore_under: Option<&str>,
) -> Result<(), String> {
    if prefix.len() > MAX_PREFIX_LEN {
        return Err(format!("folder path '{path}' is too long"));
    }
    if let Some(other) = folders.iter().find(|f| {
        f.path != path
            && f.wiki_prefix == prefix
            && !ignore_under.is_some_and(|from| is_at_or_under(&f.path, from))
    }) {
        return Err(format!(
            "'{path}' would share its document folder '{prefix}' with '{}' — pick a different name",
            other.path
        ));
    }
    Ok(())
}

/// Add a folder (and any missing ancestor) to `workspace`'s schema.
///
/// Creating a folder that already exists succeeds; a `description` passed
/// then replaces the old one, `None` leaves it alone.
pub fn create_folder(
    workspace: &str,
    path: &str,
    description: Option<&str>,
) -> Result<LibraryFolder, String> {
    create_folder_in(&Roots::home()?, workspace, path, description)
}

pub fn create_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: Option<&str>,
) -> Result<LibraryFolder, String> {
    create_keyed_folder_in(roots, workspace, path, description, None)
}

/// [`create_folder_in`] with a readable [`LibraryFolder::key`] for the leaf
/// (normalized with [`normalize_key`]; ancestors it has to create get derived
/// segments). The key only applies when the folder is new — an existing
/// folder's key is never changed here, since that would move its documents.
pub fn create_keyed_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: Option<&str>,
    folder_key: Option<&str>,
) -> Result<LibraryFolder, String> {
    create_folder_impl(roots, workspace, path, description, folder_key, None).map(|(f, _)| f)
}

/// [`create_keyed_folder_in`] for a folder an AI (an agent filing something,
/// an upload's classifier) decided to add. Every folder the call creates —
/// the leaf and any missing ancestor — carries `origin`, so the user sees all
/// of it marked. Returns the folder and the paths the call created, parents
/// first (empty when the folder already existed — it is returned untouched:
/// its origin, key and description stay as they are).
pub fn create_ai_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: Option<&str>,
    folder_key: Option<&str>,
    origin: FolderOrigin,
) -> Result<(LibraryFolder, Vec<String>), String> {
    create_folder_impl(roots, workspace, path, description, folder_key, Some(origin))
}

/// The one place folders are created. Returns the leaf and the paths it had
/// to create.
fn create_folder_impl(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: Option<&str>,
    folder_key: Option<&str>,
    origin: Option<FolderOrigin>,
) -> Result<(LibraryFolder, Vec<String>), String> {
    import_legacy_artifact_folders(roots);
    let path = checked_path(path)?;
    let key = checked_key(workspace)?;
    let leaf_key = folder_key.and_then(normalize_key).unwrap_or_default();
    with_project_lock(roots, &key, || {
        let mut folders = project_folders(roots, &key);
        let ns = project_namespace_in(roots, &key);
        let now = now_ms();
        let mut created = Vec::new();
        for p in prefixes(&path) {
            if folders.iter().any(|f| f.path == p) {
                continue;
            }
            let explicit = if p == path { leaf_key.clone() } else { String::new() };
            let auto = auto_key(leaf_of(&p));
            let mut f = LibraryFolder {
                workspace_path: key.clone(),
                path: p.clone(),
                key: if explicit.is_empty() { auto.clone() } else { explicit.clone() },
                created_ms: now,
                updated_ms: now,
                origin: origin.clone(),
                ..Default::default()
            };
            folders.push(f.clone());
            f.wiki_prefix = prefix_in(&ns, &folders, &p);
            folders.pop();
            if explicit.is_empty() && !auto.is_empty() {
                // A pinyin key is a nicety, not a promise: two names can
                // share a spelling (`公式` / `公示`). Fall back to the derived
                // segment rather than refuse the folder.
                if ensure_prefix_free(&folders, &p, &f.wiki_prefix, None).is_err() {
                    f.key = String::new();
                    folders.push(f.clone());
                    f.wiki_prefix = prefix_in(&ns, &folders, &p);
                    folders.pop();
                }
            }
            ensure_prefix_free(&folders, &p, &f.wiki_prefix, None)?;
            write_folder(roots, &f)?;
            created.push(p.clone());
            folders.push(f);
        }
        let leaf = folders
            .iter_mut()
            .find(|f| f.path == path)
            .expect("created above");
        // An AI naming a folder that already exists files into it; it does
        // not get to rewrite the user's description of it.
        let may_describe = origin.is_none() || created.contains(&path);
        if let Some(d) = description.filter(|_| may_describe) {
            let d = d.trim().to_string();
            if d != leaf.description {
                leaf.description = d;
                leaf.updated_ms = now;
                write_folder(roots, leaf)?;
            }
        }
        Ok((leaf.clone(), created))
    })
}

/// Clear a folder's AI-created marker: the user looked at it and keeps it.
/// The origin record itself stays, as history.
pub fn accept_folder(workspace: &str, path: &str) -> Result<LibraryFolder, String> {
    accept_folder_in(&Roots::home()?, workspace, path)
}

pub fn accept_folder_in(roots: &Roots, workspace: &str, path: &str) -> Result<LibraryFolder, String> {
    import_legacy_artifact_folders(roots);
    let path = artifacts::normalize_dir_path(path)?;
    let key = checked_key(workspace)?;
    with_project_lock(roots, &key, || {
        let mut folder = project_folders(roots, &key)
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| format!("folder '{path}' not found"))?;
        if let Some(o) = folder.origin.as_mut().filter(|o| o.accepted_ms.is_none()) {
            o.accepted_ms = Some(now_ms());
            write_folder(roots, &folder)?;
        }
        Ok(folder)
    })
}

/// The schema folder an agent (or a CLI user) means by `raw`: its display
/// path (`发票/2026`), its document prefix (`invoices/y2026`), either one
/// case-insensitively, or — when exactly one folder matches — a bare leaf name
/// or key (`2026`, `y2026`). `None` when nothing or more than one matches.
pub fn resolve_folder<'a>(folders: &'a [LibraryFolder], raw: &str) -> Option<&'a LibraryFolder> {
    let want = artifacts::normalize_dir_path(raw).ok()?;
    if want.is_empty() {
        return None;
    }
    if let Some(f) = folders.iter().find(|f| f.path == want || f.wiki_prefix == want) {
        return Some(f);
    }
    let lower = want.to_lowercase();
    if let Some(f) = folders
        .iter()
        .find(|f| f.path.to_lowercase() == lower || f.wiki_prefix.to_lowercase() == lower)
    {
        return Some(f);
    }
    // The prefix without the project namespace, as agents wrote it before.
    if let Some(f) = folders.iter().find(|f| rel_prefix(f) == lower) {
        return Some(f);
    }
    if want.contains('/') {
        return None;
    }
    let hits: Vec<&LibraryFolder> = folders
        .iter()
        .filter(|f| {
            leaf_of(&f.path).to_lowercase() == lower || leaf_of(&f.wiki_prefix) == lower
        })
        .collect();
    (hits.len() == 1).then(|| hits[0])
}

/// Replace a folder's one-line description.
pub fn describe_folder(
    workspace: &str,
    path: &str,
    description: &str,
) -> Result<LibraryFolder, String> {
    describe_folder_in(&Roots::home()?, workspace, path, description)
}

pub fn describe_folder_in(
    roots: &Roots,
    workspace: &str,
    path: &str,
    description: &str,
) -> Result<LibraryFolder, String> {
    import_legacy_artifact_folders(roots);
    let path = artifacts::normalize_dir_path(path)?;
    let key = checked_key(workspace)?;
    with_project_lock(roots, &key, || {
        let mut folder = project_folders(roots, &key)
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| format!("folder '{path}' not found"))?;
        folder.description = description.trim().to_string();
        folder.updated_ms = now_ms();
        write_folder(roots, &folder)?;
        Ok(folder)
    })
}

/// Remove a folder and its subfolders from the schema. Refused while any doc or
/// artifact is filed in it, so the button can never orphan anything silently.
pub fn delete_folder(workspace: &str, path: &str) -> Result<(), String> {
    delete_folder_in(&Roots::home()?, workspace, path)
}

pub fn delete_folder_in(roots: &Roots, workspace: &str, path: &str) -> Result<(), String> {
    import_legacy_artifact_folders(roots);
    let path = artifacts::normalize_dir_path(path)?;
    if path.is_empty() {
        return Err("the project root is not a folder you can delete".to_string());
    }
    let key = checked_key(workspace)?;
    let filed = tree_in(roots, Some(&key))
        .into_iter()
        .flat_map(|p| p.items)
        .filter(|i| i.folder.as_deref().is_some_and(|f| is_at_or_under(f, &path)))
        .count();
    if filed > 0 {
        return Err(format!(
            "'{path}' still holds {filed} item(s) — move them out first"
        ));
    }
    with_project_lock(roots, &key, || {
        let doomed: Vec<String> = project_folders(roots, &key)
            .into_iter()
            .filter(|f| is_at_or_under(&f.path, &path))
            .map(|f| f.path)
            .collect();
        if doomed.is_empty() {
            return Err(format!("folder '{path}' not found"));
        }
        for p in doomed {
            remove_folder_file(roots, &key, &p)?;
        }
        Ok(())
    })
}

/// Rename or re-nest a folder, carrying its subfolders and everything filed
/// under it: artifacts get their `path` rewritten, wiki docs are re-keyed onto
/// the new prefix.
pub fn rename_folder(workspace: &str, from: &str, to: &str) -> Result<FolderRename, String> {
    rename_folder_in(&Roots::home()?, workspace, from, to)
}

pub fn rename_folder_in(
    roots: &Roots,
    workspace: &str,
    from: &str,
    to: &str,
) -> Result<FolderRename, String> {
    import_legacy_artifact_folders(roots);
    let from = artifacts::normalize_dir_path(from)?;
    if from.is_empty() {
        return Err("the project root cannot be renamed".to_string());
    }
    let to = checked_path(to)?;
    if from == to {
        return Ok(FolderRename::default());
    }
    if is_at_or_under(&to, &from) {
        return Err(format!("cannot move '{from}' into itself"));
    }
    let key = checked_key(workspace)?;
    let mut keys = KeyCache::default();

    with_project_lock(roots, &key, || {
        let folders = project_folders(roots, &key);
        let ns = project_namespace_in(roots, &key);
        if !folders.iter().any(|f| f.path == from) {
            return Err(format!("folder '{from}' not found"));
        }
        if folders.iter().any(|f| f.path == to) {
            return Err(format!("folder '{to}' already exists"));
        }
        let moving: Vec<&LibraryFolder> = folders
            .iter()
            .filter(|f| is_at_or_under(&f.path, &from))
            .collect();
        for f in &moving {
            let next = repath(&f.path, &from, &to).expect("filtered under from");
            checked_path(&next)?;
        }
        // The schema as it will be: moved folders keep their keys, so only
        // segments without one (and new ancestors of `to`) change spelling.
        let after: Vec<LibraryFolder> = folders
            .iter()
            .map(|f| match repath(&f.path, &from, &to) {
                Some(next) => LibraryFolder {
                    path: next,
                    ..f.clone()
                },
                None => f.clone(),
            })
            .collect();
        let old_prefix = prefix_in(&ns, &folders, &from);
        let new_prefix = prefix_in(&ns, &after, &to);
        ensure_prefix_free(&folders, &to, &new_prefix, Some(&from))?;

        // Plan the wiki re-keys before touching anything: they are the part
        // that can collide (slugs are global), and they can be rolled back.
        let mut doc_plan: Vec<(String, String)> = Vec::new();
        if old_prefix != new_prefix {
            let needle = format!("{old_prefix}/");
            for doc in wiki::list_docs_in(&roots.wiki) {
                if keys.key(&doc.workspace_path) != key {
                    continue;
                }
                let Some(rest) = doc.slug.strip_prefix(&needle) else {
                    continue;
                };
                let target = wiki::normalize_slug(&format!("{new_prefix}/{rest}"))?;
                if roots
                    .wiki
                    .join(wiki::slug_to_dirname(&target))
                    .join("doc.json")
                    .exists()
                {
                    return Err(format!("wiki doc '{target}' already exists"));
                }
                doc_plan.push((doc.slug.clone(), target));
            }
        }

        let mut done: Vec<(String, String)> = Vec::new();
        for (old, new) in &doc_plan {
            if let Err(e) = wiki::move_doc_in(&roots.wiki, old, new) {
                for (o, n) in done.iter().rev() {
                    let _ = wiki::unmove_doc_in(&roots.wiki, n, o);
                }
                return Err(e);
            }
            done.push((old.clone(), new.clone()));
        }

        // Schema: write the new records first, then drop the old ones, so a
        // crash in between duplicates a folder rather than losing one.
        let now = now_ms();
        let mut have: HashSet<String> = folders.iter().map(|f| f.path.clone()).collect();
        for f in &moving {
            let next = repath(&f.path, &from, &to).expect("filtered under from");
            let mut moved = (*f).clone();
            moved.path = next.clone();
            moved.wiki_prefix = prefix_in(&ns, &after, &next);
            moved.updated_ms = now;
            // Renaming an AI-created folder is the user deciding what it is:
            // that clears its marker, as accepting would.
            if f.path == from {
                if let Some(o) = moved.origin.as_mut().filter(|o| o.accepted_ms.is_none()) {
                    o.accepted_ms = Some(now);
                }
            }
            write_folder(roots, &moved)?;
            have.insert(next);
        }
        for p in prefixes(&to) {
            if have.insert(p.clone()) {
                write_folder(
                    roots,
                    &LibraryFolder {
                        workspace_path: key.clone(),
                        path: p.clone(),
                        wiki_prefix: prefix_in(&ns, &after, &p),
                        created_ms: now,
                        updated_ms: now,
                        ..Default::default()
                    },
                )?;
            }
        }
        for f in &moving {
            remove_folder_file(roots, &key, &f.path)?;
        }

        let mut moved_artifacts = 0usize;
        for a in artifacts::list_in(&roots.artifacts) {
            if keys.key(&a.workspace_path) != key {
                continue;
            }
            let Some(next) = repath(&a.path, &from, &to) else {
                continue;
            };
            artifacts::update_in(&roots.artifacts, &a.id, None, None, None, Some(&next))?;
            moved_artifacts += 1;
        }
        Ok(FolderRename {
            moved_docs: done.len(),
            moved_artifacts,
        })
    })
}

// ── Filing ───────────────────────────────────────────────────────────────────

/// File one item into `folder` of its own project's schema; `""` unfiles it
/// (a doc goes to the wiki root, an artifact's `path` is cleared).
pub fn move_item(kind: &str, id: &str, folder: &str) -> Result<LibraryItem, String> {
    move_item_in(&Roots::home()?, kind, id, folder)
}

pub fn move_item_in(
    roots: &Roots,
    kind: &str,
    id: &str,
    folder: &str,
) -> Result<LibraryItem, String> {
    import_legacy_artifact_folders(roots);
    let folder = artifacts::normalize_dir_path(folder)?;
    let target = |workspace: &str| -> Result<Option<LibraryFolder>, String> {
        if folder.is_empty() {
            return Ok(None);
        }
        let key = checked_key(workspace)?;
        project_folders(roots, &key)
            .into_iter()
            .find(|f| f.path == folder)
            .map(Some)
            .ok_or_else(|| format!("folder '{folder}' is not in this project's schema"))
    };
    match kind {
        ITEM_DOC => {
            let doc = wiki::get_doc_in(&roots.wiki, id)?;
            let dest = target(&doc.workspace_path)?;
            let base = wiki::slug_basename(&doc.slug);
            let slug = match &dest {
                Some(f) => format!("{}/{base}", f.wiki_prefix),
                // Unfiled, but still in the project's namespace: a bare slug
                // would compete with every other project's.
                None => format!("{}/{base}", project_namespace_in(roots, &checked_key(&doc.workspace_path)?)),
            };
            let moved = wiki::move_doc_in(&roots.wiki, &doc.slug, &slug)?;
            Ok(doc_item(&moved, dest.map(|f| f.path)))
        }
        ITEM_ARTIFACT => {
            let a = artifacts::get_in(&roots.artifacts, id)?;
            let dest = target(&a.workspace_path)?;
            let path = dest.as_ref().map(|f| f.path.as_str()).unwrap_or("");
            let updated =
                artifacts::update_in(&roots.artifacts, id, None, None, None, Some(path))?;
            Ok(artifact_item(&updated, dest.map(|f| f.path)))
        }
        other => Err(format!("unknown library item kind '{other}'")),
    }
}

/// A slug under `prefix` for the doc at `doc` (named `base` there) that
/// nothing else occupies — no other doc and no redirect another doc left
/// (taking that over would cut the other doc's old link). A redirect to `doc`
/// itself, left by an earlier move of it, is free: the doc takes it back.
pub(crate) fn free_slug(wiki_root: &Path, prefix: &str, doc: &str) -> Result<String, String> {
    let base = wiki::slug_basename(doc);
    let current = wiki::resolve_slug_in(wiki_root, doc);
    for n in 1..100 {
        let name = if n == 1 { base.to_string() } else { format!("{base}-{n}") };
        let slug = wiki::normalize_slug(&format!("{prefix}/{name}"))?;
        let dir = wiki_root.join(wiki::slug_to_dirname(&slug));
        let own_redirect = !dir.join("doc.json").exists()
            && current.is_some()
            && wiki::resolve_slug_in(wiki_root, &slug) == current;
        if !dir.exists() || own_redirect {
            return Ok(slug);
        }
    }
    Err(format!("no free slug for '{base}' under '{prefix}'"))
}

/// One doc [`migrate_namespaces_in`] moves (or would move).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceMove {
    pub workspace_path: String,
    pub folder: String,
    pub from: String,
    pub to: String,
    /// Why it was not moved; `None` = moved (or, in a dry run, would be).
    #[serde(default)]
    pub error: Option<String>,
}

/// Move docs filed before namespacing onto their folder's namespaced prefix:
/// a doc of a project whose slug directory is a folder's prefix without the
/// namespace (`research/memo`) goes to `<namespace>/research/memo`, leaving a
/// redirect at the old slug so links and quoted slugs keep working. Every
/// project, or only `workspace`'s. Idempotent: a moved doc no longer matches.
/// Safe to run again whenever an older build may have filed more docs the old
/// way.
pub fn migrate_namespaces_in(
    roots: &Roots,
    workspace: Option<&str>,
    dry_run: bool,
) -> Result<Vec<NamespaceMove>, String> {
    import_legacy_artifact_folders(roots);
    let only = workspace.map(checked_key).transpose()?;
    let mut by_project: BTreeMap<String, Vec<LibraryFolder>> = BTreeMap::new();
    for f in all_folders(roots) {
        if only.as_ref().is_none_or(|k| *k == f.workspace_path) {
            by_project.entry(f.workspace_path.clone()).or_default().push(f);
        }
    }
    let mut keys = KeyCache::default();
    let docs = wiki::list_docs_in(&roots.wiki);
    let mut out = Vec::new();
    for (key, folders) in by_project {
        for doc in docs.iter().filter(|d| keys.key(&d.workspace_path) == key) {
            let dir = slug_dir(&doc.slug);
            // Already on a namespaced prefix (or unfiled): nothing to do.
            if dir.is_empty() || folders.iter().any(|f| f.wiki_prefix == dir) {
                continue;
            }
            let Some(folder) = folders.iter().find(|f| rel_prefix(f) == dir) else {
                continue;
            };
            let mut m = NamespaceMove {
                workspace_path: key.clone(),
                folder: folder.path.clone(),
                from: doc.slug.clone(),
                to: String::new(),
                error: None,
            };
            match free_slug(&roots.wiki, &folder.wiki_prefix, &doc.slug) {
                Ok(to) => {
                    m.to = to;
                    if !dry_run {
                        if let Err(e) = wiki::move_doc_in(&roots.wiki, &doc.slug, &m.to) {
                            m.error = Some(e);
                        }
                    }
                }
                Err(e) => m.error = Some(e),
            }
            out.push(m);
        }
    }
    Ok(out)
}

pub fn migrate_namespaces(workspace: Option<&str>, dry_run: bool) -> Result<Vec<NamespaceMove>, String> {
    migrate_namespaces_in(&Roots::home()?, workspace, dry_run)
}

// ── Artifacts page compatibility ─────────────────────────────────────────────

/// The schema as the Artifacts page's folder list: one record per folder for
/// the project key, plus one for every worktree path the project's artifacts
/// were recorded under — that page groups by the exact recorded path, and a
/// folder that only appeared under the repo root would be missing from every
/// worktree's branch.
pub fn artifact_folders_in(roots: &Roots) -> Vec<artifacts::Folder> {
    import_legacy_artifact_folders(roots);
    let mut keys = KeyCache::default();
    let mut variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in artifacts::list_in(&roots.artifacts) {
        let key = keys.key(&a.workspace_path);
        let list = variants.entry(key).or_default();
        if !list.contains(&a.workspace_path) {
            list.push(a.workspace_path.clone());
        }
    }
    let mut out: Vec<artifacts::Folder> = Vec::new();
    for f in all_folders(roots) {
        out.push(artifacts::Folder {
            workspace_path: f.workspace_path.clone(),
            path: f.path.clone(),
        });
        for ws in variants.get(&f.workspace_path).into_iter().flatten() {
            if *ws != f.workspace_path {
                out.push(artifacts::Folder {
                    workspace_path: ws.clone(),
                    path: f.path.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.workspace_path
            .cmp(&b.workspace_path)
            .then_with(|| a.path.cmp(&b.path))
    });
    out.dedup();
    out
}

pub fn artifact_folders() -> Vec<artifacts::Folder> {
    match Roots::home() {
        Ok(roots) => artifact_folders_in(&roots),
        Err(_) => Vec::new(),
    }
}

// ── Workspace hygiene ───────────────────────────────────────────────────────

/// Fewest unfiled items a project needs before re-filing drafts a schema for
/// it; below this a schema would be guessed from almost nothing.
pub const MIN_REFILE_ITEMS: usize = 5;

/// Whether a project key names a real project worth a folder schema of its
/// own. Items get recorded under whatever directory the agent ran in, so the
/// store also holds session scratchpads, `/tmp` probes, the home directory and
/// projects deleted since; drafting a schema for each of those would bury the
/// real projects' folders under one-off ones.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectHealth {
    Ok,
    /// The directory no longer exists.
    Missing,
    /// Under a temp directory (`/tmp`, `/var/folders`, the session scratchpads).
    Temp,
    /// The home directory itself: not a project.
    Home,
    /// Fewer than [`MIN_REFILE_ITEMS`] unfiled items.
    Tiny,
}

impl ProjectHealth {
    /// Why re-filing skips the project; empty for [`Self::Ok`].
    pub fn reason(self) -> &'static str {
        match self {
            Self::Ok => "",
            Self::Missing => "the project directory no longer exists",
            Self::Temp => "it is a temporary directory",
            Self::Home => "it is the home directory, not a project",
            Self::Tiny => "it has fewer than 5 unfiled items",
        }
    }
}

fn is_temp_path(path: &Path) -> bool {
    let mut roots: Vec<PathBuf> = ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders", "/var/tmp"]
        .iter()
        .map(PathBuf::from)
        .collect();
    let tmp = std::env::temp_dir();
    if let Ok(canon) = tmp.canonicalize() {
        roots.push(canon);
    }
    roots.push(tmp);
    roots.iter().any(|r| path.starts_with(r))
}

/// Classify project `key` holding `unfiled` unfiled items.
pub fn project_health(key: &str, unfiled: usize) -> ProjectHealth {
    let path = Path::new(key);
    if is_temp_path(path) {
        return ProjectHealth::Temp;
    }
    if dirs::home_dir().is_some_and(|h| h == path) {
        return ProjectHealth::Home;
    }
    if !path.is_dir() {
        return ProjectHealth::Missing;
    }
    if unfiled < MIN_REFILE_ITEMS {
        return ProjectHealth::Tiny;
    }
    ProjectHealth::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Env {
        _tmp: TempDir,
        roots: Roots,
        /// A workspace directory outside any checkout.
        ws: PathBuf,
    }

    fn env() -> Env {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let roots = Roots {
            library: base.join("library"),
            wiki: base.join("wiki"),
            artifacts: base.join("artifacts"),
        };
        let ws = base.join("project");
        fs::create_dir_all(&ws).unwrap();
        Env {
            _tmp: tmp,
            roots,
            ws,
        }
    }

    impl Env {
        fn ws(&self) -> String {
            self.ws.display().to_string()
        }

        fn doc(&self, slug: &str, ws: &Path) -> WikiDoc {
            wiki::publish_text_in(
                &self.roots.wiki,
                slug,
                Some(slug),
                "# body",
                ws,
                wiki::TextPublishMode::Replace,
            )
            .unwrap()
        }

        fn artifact(&self, name: &str, ws: &Path) -> Artifact {
            let src = ws.join(name);
            fs::write(&src, b"bytes").unwrap();
            artifacts::add_in(&self.roots.artifacts, &src, None, None, ws, None).unwrap()
        }

        fn project(&self) -> LibraryProject {
            let mut all = tree_in(&self.roots, Some(&self.ws()));
            assert_eq!(all.len(), 1, "expected exactly one project");
            all.remove(0)
        }
    }

    fn item<'a>(p: &'a LibraryProject, id: &str) -> &'a LibraryItem {
        p.items.iter().find(|i| i.id == id).expect("item present")
    }

    #[test]
    fn tree_items_carry_summary_tags_and_file_facts() {
        let e = env();
        let doc = e.doc("notes", &e.ws);
        wiki::write_summary_in(
            &e.roots.wiki,
            &doc.slug,
            &wiki::DocSummary {
                summary: "What the notes say.".into(),
                tags: vec!["notes".into()],
                version: doc.current_version.clone(),
                generated_ms: 1,
            },
        )
        .unwrap();
        let art = e.artifact("shot.png", &e.ws);
        let p = e.project();
        let d = item(&p, &doc.slug);
        assert_eq!(d.summary, "What the notes say.");
        assert_eq!(d.tags, vec!["notes".to_string()]);
        assert!(d.name.is_empty() && d.size_bytes == 0);
        let a = item(&p, &art.id);
        assert_eq!(a.name, "shot.png");
        assert_eq!(a.mime, art.mime);
        assert_eq!(a.size_bytes, 5);
        // Empty facts stay off the wire.
        let json = serde_json::to_value(a).unwrap();
        assert!(json.get("summary").is_none() && json.get("tags").is_none());
    }

    #[test]
    fn outline_counts_every_folder_but_keeps_only_the_newest_few_without_summaries() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "发票", None).unwrap();
        create_folder_in(&e.roots, &e.ws(), "发票/2026", None).unwrap();
        for n in 0..5 {
            let doc = e.doc(&format!("bill-{n}"), &e.ws);
            wiki::write_summary_in(
                &e.roots.wiki,
                &doc.slug,
                &wiki::DocSummary {
                    summary: format!("Bill {n}."),
                    tags: vec!["bill".into()],
                    version: doc.current_version.clone(),
                    generated_ms: 1,
                },
            )
            .unwrap();
            let folder = if n < 3 { "发票" } else { "发票/2026" };
            move_item_in(&e.roots, ITEM_DOC, &doc.slug, folder).unwrap();
        }
        e.artifact("loose.pdf", &e.ws);

        let p = outline_in(&e.roots, Some(&e.ws()), 2).remove(0);
        assert_eq!(p.counts.get("发票"), Some(&3));
        assert_eq!(p.counts.get("发票/2026"), Some(&2));
        assert_eq!(p.counts.get(""), Some(&1), "unfiled items count under \"\"");
        // Two per folder survive, plus the one unfiled item.
        assert_eq!(p.items.len(), 5);
        assert_eq!(p.items.iter().filter(|i| i.folder.as_deref() == Some("发票")).count(), 2);
        assert!(p.items.iter().all(|i| i.summary.is_empty() && i.tags.is_empty()));
        assert_eq!(p.folders.len(), 2, "folders are carried whole");
        // A full tree never carries counts.
        assert!(serde_json::to_value(e.project()).unwrap().get("counts").is_none());

        // Opening a folder answers everything beneath it, summaries included.
        let under = folder_items_in(&e.roots, &e.ws(), "发票").unwrap();
        assert_eq!(under.len(), 5);
        assert!(under.iter().all(|i| i.summary.starts_with("Bill ")));
        assert_eq!(folder_items_in(&e.roots, &e.ws(), "/发票/2026/").unwrap().len(), 2);
        let unfiled = folder_items_in(&e.roots, &e.ws(), "").unwrap();
        assert_eq!(unfiled.len(), 1);
        assert_eq!(unfiled[0].name, "loose.pdf");
        // A folder name that merely shares a prefix is not "beneath".
        assert!(folder_items_in(&e.roots, &e.ws(), "发").unwrap().is_empty());
        assert!(folder_items_in(&e.roots, "", "发票").is_err());
    }

    #[test]
    fn segment_keys_keep_ascii_readable_and_hash_everything_else() {
        assert_eq!(segment_key("Design Notes"), "design-notes");
        assert_eq!(segment_key("arch"), "arch");
        let cjk = segment_key("发票");
        assert!(cjk.starts_with("f-") && cjk.len() == 8, "{cjk}");
        assert_eq!(cjk, segment_key("发票"), "must be stable");
        let a = segment_key("2026 发票");
        let b = segment_key("2026 收据");
        assert!(a.starts_with("2026-") && b.starts_with("2026-"));
        assert_ne!(a, b, "mixed names must not collide on their ASCII part");
        assert_eq!(wiki_prefix("Design Notes/发票"), format!("design-notes/{cjk}"));
        // Every key is a valid slug segment.
        for k in [cjk, a, b] {
            assert_eq!(wiki::normalize_slug(&k).unwrap(), k);
        }
    }

    #[test]
    fn auto_keys_spell_hanzi_in_pinyin() {
        assert_eq!(auto_key("合同"), "hetong");
        assert_eq!(auto_key("2026 发票"), "2026-fapiao");
        assert_eq!(auto_key("发票2026"), "fapiao-2026");
        assert_eq!(auto_key("design"), "", "ascii names keep segment_key");
        assert_eq!(auto_key("✨"), "");
    }

    #[test]
    fn keyed_folders_give_cjk_names_readable_prefixes() {
        let e = env();
        assert_eq!(normalize_key(" Invoices 2026 ").as_deref(), Some("invoices-2026"));
        assert_eq!(normalize_key("发票"), None);
        create_keyed_folder_in(&e.roots, &e.ws(), "发票", Some("invoices"), Some("Invoices")).unwrap();
        create_keyed_folder_in(&e.roots, &e.ws(), "发票/2026", None, Some("y2026")).unwrap();
        // A child without a key still sits under its parent's key.
        create_folder_in(&e.roots, &e.ws(), "发票/餐饮", None).unwrap();
        let folders = list_folders_in(&e.roots, &e.ws()).unwrap();
        let prefix = |p: &str| folders.iter().find(|f| f.path == p).unwrap().wiki_prefix.clone();
        assert_eq!(prefix("发票"), "project/invoices");
        assert_eq!(prefix("发票/2026"), "project/invoices/y2026");
        assert_eq!(prefix("发票/餐饮"), "project/invoices/canyin");

        // Docs file under the keyed prefix and are found there.
        let moved = move_item_in(&e.roots, ITEM_DOC, &e.doc("scan", &e.ws).slug, "发票/2026").unwrap();
        assert_eq!(moved.id, "project/invoices/y2026/scan");
        assert_eq!(item(&e.project(), "project/invoices/y2026/scan").folder.as_deref(), Some("发票/2026"));

        // Creating an existing folder again never changes its key.
        create_keyed_folder_in(&e.roots, &e.ws(), "发票", None, Some("bills")).unwrap();
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap()[0].key, "invoices");

        // A second folder asking for a key already taken is refused.
        let err = create_keyed_folder_in(&e.roots, &e.ws(), "收据", None, Some("invoices")).unwrap_err();
        assert!(err.contains("发票"), "{err}");
    }

    #[test]
    fn rename_keeps_keys_so_docs_keep_their_slugs() {
        let e = env();
        create_keyed_folder_in(&e.roots, &e.ws(), "发票", None, Some("invoices")).unwrap();
        e.doc("project/invoices/jd-0920", &e.ws);
        let r = rename_folder_in(&e.roots, &e.ws(), "发票", "报销发票").unwrap();
        // Same key, same prefix: nothing to re-key.
        assert_eq!(r.moved_docs, 0);
        let p = e.project();
        assert_eq!(p.folders[0].path, "报销发票");
        assert_eq!(p.folders[0].key, "invoices");
        assert_eq!(item(&p, "project/invoices/jd-0920").folder.as_deref(), Some("报销发票"));

        // Re-nesting under a new parent moves the docs under the parent's prefix.
        create_keyed_folder_in(&e.roots, &e.ws(), "财务", None, Some("finance")).unwrap();
        let r = rename_folder_in(&e.roots, &e.ws(), "报销发票", "财务/报销发票").unwrap();
        assert_eq!(r.moved_docs, 1);
        assert_eq!(
            item(&e.project(), "project/finance/invoices/jd-0920").folder.as_deref(),
            Some("财务/报销发票")
        );
    }

    #[test]
    fn a_folder_holds_both_a_doc_and_an_artifact_and_the_rest_is_unfiled() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "reports", Some("Quarterly reports")).unwrap();
        e.doc("project/reports/q3", &e.ws);
        e.doc("loose-note", &e.ws);
        e.doc("elsewhere/deep", &e.ws);
        let filed = e.artifact("q3.pdf", &e.ws);
        let loose = e.artifact("loose.pdf", &e.ws);
        artifacts::update_in(&e.roots.artifacts, &filed.id, None, None, None, Some("reports"))
            .unwrap();

        let p = e.project();
        assert_eq!(p.folders.len(), 1);
        assert_eq!(p.folders[0].description, "Quarterly reports");
        assert_eq!(p.folders[0].wiki_prefix, "project/reports");
        assert_eq!(item(&p, "project/reports/q3").folder.as_deref(), Some("reports"));
        assert_eq!(item(&p, &filed.id).folder.as_deref(), Some("reports"));
        assert_eq!(item(&p, "loose-note").folder, None);
        assert_eq!(item(&p, &loose.id).folder, None);
        // A doc in a directory the schema does not know is unfiled, not a new
        // folder — and its location says where it sits.
        let deep = item(&p, "elsewhere/deep");
        assert_eq!(deep.folder, None);
        assert_eq!(deep.location, "elsewhere");
        assert_eq!(p.items.len(), 5);
    }

    #[test]
    fn an_artifact_is_filed_only_by_its_explicit_path() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "out", None).unwrap();
        fs::create_dir_all(e.ws.join("out")).unwrap();
        // Written into <ws>/out but never filed: the derived directory is a
        // hint, not a filing — otherwise it could never be unfiled.
        let a = e.artifact("out/render.mp4", &e.ws);
        let p = e.project();
        let it = item(&p, &a.id);
        assert_eq!(it.folder, None);
        assert_eq!(it.location, "out");
    }

    #[test]
    fn worktrees_of_one_repo_share_one_tree() {
        let e = env();
        let repo = e.ws.join("repo");
        fs::create_dir_all(repo.join(".git/worktrees/feat")).unwrap();
        let wt = repo.join(".worktrees/feat");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/feat").display()),
        )
        .unwrap();

        create_folder_in(&e.roots, &wt.display().to_string(), "notes", None).unwrap();
        e.doc("repo/notes/from-worktree", &wt);
        e.doc("repo/notes/from-main", &repo);

        let all = tree_in(&e.roots, Some(&repo.display().to_string()));
        assert_eq!(all.len(), 1);
        let p = &all[0];
        assert_eq!(p.workspace_path, repo.display().to_string());
        assert_eq!(p.folders.len(), 1);
        assert!(p.items.iter().all(|i| i.folder.as_deref() == Some("notes")));
        assert_eq!(p.items.len(), 2);
    }

    #[test]
    fn projects_do_not_see_each_others_folders_or_items() {
        let e = env();
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        create_folder_in(&e.roots, &e.ws(), "a", None).unwrap();
        create_folder_in(&e.roots, &other.display().to_string(), "b", None).unwrap();
        e.doc("project/a/one", &e.ws);
        e.doc("other/b/two", &other);
        // `project/a/` exists only in the first project's schema.
        e.doc("project/a/three", &other);

        let all = tree_in(&e.roots, None);
        assert_eq!(all.len(), 2);
        let theirs = all
            .iter()
            .find(|p| p.workspace_path == other.display().to_string())
            .unwrap();
        assert_eq!(theirs.folders.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(item(theirs, "project/a/three").folder, None);
        assert_eq!(item(theirs, "other/b/two").folder.as_deref(), Some("b"));
    }

    #[test]
    fn create_registers_ancestors_and_is_idempotent() {
        let e = env();
        let f = create_folder_in(&e.roots, &e.ws(), "/交付/2026Q3/", None).unwrap();
        assert_eq!(f.path, "交付/2026Q3");
        let paths: Vec<String> = list_folders_in(&e.roots, &e.ws())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["交付", "交付/2026Q3"]);
        // Again, now with a description: no duplicate, description set.
        let again = create_folder_in(&e.roots, &e.ws(), "交付/2026Q3", Some("Q3 deliverables")).unwrap();
        assert_eq!(again.description, "Q3 deliverables");
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap().len(), 2);
        assert!(create_folder_in(&e.roots, &e.ws(), " / ", None).is_err());
        assert!(create_folder_in(&e.roots, &e.ws(), "a/b/c/d/e/f/g/h", None).is_err());
    }

    #[test]
    fn two_folders_that_would_share_a_wiki_prefix_are_refused() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "Design Notes", None).unwrap();
        let err = create_folder_in(&e.roots, &e.ws(), "design-notes", None).unwrap_err();
        assert!(err.contains("Design Notes"), "{err}");
    }

    #[test]
    fn describe_replaces_the_description() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "发票", None).unwrap();
        let f = describe_folder_in(&e.roots, &e.ws(), "发票", "  报销用的发票  ").unwrap();
        assert_eq!(f.description, "报销用的发票");
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap()[0].description, "报销用的发票");
        assert!(describe_folder_in(&e.roots, &e.ws(), "nope", "x").is_err());
    }

    #[test]
    fn moving_items_in_and_out_of_a_folder() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "发票", None).unwrap();
        e.doc("scan", &e.ws);
        let a = e.artifact("receipt.pdf", &e.ws);

        let moved = move_item_in(&e.roots, ITEM_DOC, "scan", "发票").unwrap();
        assert_eq!(moved.folder.as_deref(), Some("发票"));
        assert_eq!(moved.id, "project/fapiao/scan");
        let moved_a = move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "发票").unwrap();
        assert_eq!(moved_a.folder.as_deref(), Some("发票"));
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &a.id).unwrap().path, "发票");

        let p = e.project();
        assert!(p.items.iter().all(|i| i.folder.as_deref() == Some("发票")));

        // Back out: the doc stays in the project's namespace but in no folder,
        // the artifact's path clears.
        let out = move_item_in(&e.roots, ITEM_DOC, &moved.id, "").unwrap();
        assert_eq!((out.id.as_str(), out.folder), ("project/scan", None));
        move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "").unwrap();
        assert_eq!(artifacts::get_in(&e.roots.artifacts, &a.id).unwrap().path, "");

        // Filing into a folder the schema lacks is refused rather than inventing one.
        assert!(move_item_in(&e.roots, ITEM_DOC, "scan", "made-up").is_err());
    }

    #[test]
    fn delete_is_refused_while_anything_is_filed_and_takes_subfolders() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "docs/2026", None).unwrap();
        e.doc("project/docs/2026/plan", &e.ws);
        let err = delete_folder_in(&e.roots, &e.ws(), "docs").unwrap_err();
        assert!(err.contains("1 item"), "{err}");
        move_item_in(&e.roots, ITEM_DOC, "project/docs/2026/plan", "").unwrap();
        delete_folder_in(&e.roots, &e.ws(), "docs").unwrap();
        assert!(list_folders_in(&e.roots, &e.ws()).unwrap().is_empty());
        assert!(delete_folder_in(&e.roots, &e.ws(), "docs").is_err());
        assert!(delete_folder_in(&e.roots, &e.ws(), "").is_err());
    }

    #[test]
    fn rename_carries_subfolders_docs_and_artifacts() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "docs/2026", Some("this year")).unwrap();
        create_folder_in(&e.roots, &e.ws(), "docs-old", None).unwrap();
        e.doc("project/docs/2026/plan", &e.ws);
        e.doc("project/docs/top", &e.ws);
        e.doc("project/docs-old/keep", &e.ws);
        let a = e.artifact("deck.pptx", &e.ws);
        move_item_in(&e.roots, ITEM_ARTIFACT, &a.id, "docs/2026").unwrap();

        let r = rename_folder_in(&e.roots, &e.ws(), "docs", "交付").unwrap();
        assert_eq!(r, FolderRename { moved_docs: 2, moved_artifacts: 1 });

        let p = e.project();
        let paths: Vec<&str> = p.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["docs-old", "交付", "交付/2026"]);
        let kept = p.folders.iter().find(|f| f.path == "交付/2026").unwrap();
        assert_eq!(kept.description, "this year");
        let k = segment_key("交付");
        assert_eq!(item(&p, &format!("project/{k}/2026/plan")).folder.as_deref(), Some("交付/2026"));
        assert_eq!(item(&p, &format!("project/{k}/top")).folder.as_deref(), Some("交付"));
        assert_eq!(item(&p, "project/docs-old/keep").folder.as_deref(), Some("docs-old"));
        assert_eq!(item(&p, &a.id).folder.as_deref(), Some("交付/2026"));

        assert!(rename_folder_in(&e.roots, &e.ws(), "交付", "交付/内层").is_err());
        assert!(rename_folder_in(&e.roots, &e.ws(), "交付", "docs-old").is_err());
        assert!(rename_folder_in(&e.roots, &e.ws(), "没有", "x").is_err());
        assert_eq!(
            rename_folder_in(&e.roots, &e.ws(), "交付", "交付").unwrap(),
            FolderRename::default()
        );
    }

    #[test]
    fn rename_leaves_another_projects_docs_under_the_same_prefix_alone() {
        let e = env();
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        create_folder_in(&e.roots, &e.ws(), "arch", None).unwrap();
        e.doc("project/arch/mine", &e.ws);
        // Another project's doc under this project's prefix is not this rename's to move.
        e.doc("project/arch/theirs", &other);
        rename_folder_in(&e.roots, &e.ws(), "arch", "design").unwrap();
        assert!(wiki::get_doc_in(&e.roots.wiki, "project/design/mine").is_ok());
        assert!(wiki::get_doc_in(&e.roots.wiki, "project/arch/theirs").is_ok());
    }

    #[test]
    fn rename_that_would_collide_in_the_wiki_changes_nothing() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "a", None).unwrap();
        e.doc("project/a/x", &e.ws);
        // Some other project already owns `project/b/x`.
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        e.doc("project/b/x", &other);
        assert!(rename_folder_in(&e.roots, &e.ws(), "a", "b").is_err());
        assert!(wiki::get_doc_in(&e.roots.wiki, "project/a/x").is_ok());
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap()[0].path, "a");
    }

    #[test]
    fn legacy_artifact_folders_json_is_imported_and_retired() {
        let e = env();
        fs::create_dir_all(&e.roots.artifacts).unwrap();
        let legacy = vec![artifacts::Folder {
            workspace_path: e.ws(),
            path: "交付/2026Q3".to_string(),
        }];
        fs::write(
            e.roots.artifacts.join("folders.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        let paths: Vec<String> = list_folders_in(&e.roots, &e.ws())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["交付", "交付/2026Q3"]);
        assert!(!e.roots.artifacts.join("folders.json").exists());
        assert!(e.roots.artifacts.join("folders.json.imported").exists());

        // An old build writing the file again gets merged in, not lost — and
        // a folder deleted since is not resurrected by the old list.
        delete_folder_in(&e.roots, &e.ws(), "交付").unwrap();
        let again = vec![artifacts::Folder {
            workspace_path: e.ws(),
            path: "新的".to_string(),
        }];
        fs::write(
            e.roots.artifacts.join("folders.json"),
            serde_json::to_vec(&again).unwrap(),
        )
        .unwrap();
        let paths: Vec<String> = list_folders_in(&e.roots, &e.ws())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["新的"]);
    }

    #[test]
    fn the_artifacts_page_sees_schema_folders_under_every_worktree_path() {
        let e = env();
        let repo = e.ws.join("repo");
        fs::create_dir_all(repo.join(".git/worktrees/feat")).unwrap();
        let wt = repo.join(".worktrees/feat");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/feat").display()),
        )
        .unwrap();
        e.artifact("x.pdf", &wt);
        create_folder_in(&e.roots, &repo.display().to_string(), "out", None).unwrap();

        let folders = artifact_folders_in(&e.roots);
        let owners: Vec<&str> = folders.iter().map(|f| f.workspace_path.as_str()).collect();
        assert!(owners.contains(&repo.display().to_string().as_str()));
        assert!(owners.contains(&wt.display().to_string().as_str()));
    }

    #[test]
    fn half_written_and_foreign_files_in_the_folder_dir_are_ignored() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "a", None).unwrap();
        let dir = project_dir(&e.roots, &project_key(&e.ws())).join("folders");
        fs::write(dir.join("x.json.tmp.1.2"), b"{").unwrap();
        fs::write(dir.join("broken.json"), b"{").unwrap();
        assert_eq!(list_folders_in(&e.roots, &e.ws()).unwrap().len(), 1);
    }

    #[test]
    fn project_health_skips_temp_home_missing_and_tiny_projects() {
        let real = std::env::current_dir().unwrap();
        let real = real.display().to_string();
        assert_eq!(project_health(&real, MIN_REFILE_ITEMS), ProjectHealth::Ok);
        assert_eq!(project_health(&real, MIN_REFILE_ITEMS - 1), ProjectHealth::Tiny);
        assert_eq!(project_health("/private/tmp/claude-501/x/scratchpad", 50), ProjectHealth::Temp);
        assert_eq!(project_health("/tmp/wk", 50), ProjectHealth::Temp);
        let tmp = TempDir::new().unwrap();
        assert_eq!(project_health(&tmp.path().display().to_string(), 50), ProjectHealth::Temp);
        assert_eq!(project_health("/definitely/not/here/project", 50), ProjectHealth::Missing);
        let home = dirs::home_dir().unwrap().display().to_string();
        assert_eq!(project_health(&home, 50), ProjectHealth::Home);
    }

    #[test]
    fn folder_prefixes_start_with_the_projects_namespace() {
        let e = env();
        let f = create_folder_in(&e.roots, &e.ws(), "research/notes", None).unwrap();
        assert_eq!(f.wiki_prefix, "project/research/notes");
        assert_eq!(rel_prefix(&f), "research/notes");
        e.doc("project/research/notes/memo", &e.ws);
        assert_eq!(item(&e.project(), "project/research/notes/memo").folder.as_deref(), Some("research/notes"));
        // A CJK project name is spelled in pinyin.
        let cjk = e.ws.parent().unwrap().join("装修");
        fs::create_dir_all(&cjk).unwrap();
        let f = create_folder_in(&e.roots, &cjk.display().to_string(), "合同", None).unwrap();
        assert_eq!(f.wiki_prefix, "zhuangxiu/hetong");
    }

    #[test]
    fn same_named_projects_get_distinct_namespaces_that_never_change() {
        let e = env();
        let base = e.ws.parent().unwrap();
        let a = base.join("work/app");
        let b = base.join("side/app");
        let c = base.join("side2/side/app");
        for p in [&a, &b, &c] {
            fs::create_dir_all(p).unwrap();
        }
        let key = |p: &Path| p.display().to_string();
        assert_eq!(project_namespace_in(&e.roots, &key(&a)), "app");
        assert_eq!(project_namespace_in(&e.roots, &key(&b)), "side-app");
        // Name and parent-name both taken: the hashed spelling.
        let third = project_namespace_in(&e.roots, &key(&c));
        assert!(third.starts_with("app-") && third.len() == "app-".len() + 6, "{third}");
        // Persisted: asking again (in any order) gives the same answers, so
        // a project's docs never move because another project appeared.
        assert_eq!(project_namespace_in(&e.roots, &key(&b)), "side-app");
        assert_eq!(project_namespace_in(&e.roots, &key(&a)), "app");
        assert_eq!(project_namespace_in(&e.roots, &key(&c)), third);
    }

    #[test]
    fn worktrees_share_their_repos_namespace() {
        let e = env();
        let repo = e.ws.join("repo");
        fs::create_dir_all(repo.join(".git/worktrees/feat")).unwrap();
        let wt = repo.join(".worktrees/feat");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", repo.join(".git/worktrees/feat").display())).unwrap();
        let f = create_folder_in(&e.roots, &wt.display().to_string(), "notes", None).unwrap();
        assert_eq!(f.wiki_prefix, "repo/notes");
        assert_eq!(
            project_namespace_in(&e.roots, &project_key(&wt.display().to_string())),
            project_namespace_in(&e.roots, &repo.display().to_string())
        );
    }

    #[test]
    fn migration_moves_un_namespaced_docs_and_leaves_redirects() {
        let e = env();
        create_folder_in(&e.roots, &e.ws(), "research", None).unwrap();
        e.doc("research/memo", &e.ws);
        e.doc("project/research/already", &e.ws);
        e.doc("loose", &e.ws);
        // Another project's `research/x` is not this project's to move.
        let other = e.ws.parent().unwrap().join("other");
        fs::create_dir_all(&other).unwrap();
        e.doc("research/x", &other);

        let plan = migrate_namespaces_in(&e.roots, None, true).unwrap();
        assert_eq!(plan.len(), 1, "{plan:?}");
        assert_eq!((plan[0].from.as_str(), plan[0].to.as_str()), ("research/memo", "project/research/memo"));
        assert!(wiki::get_doc_in(&e.roots.wiki, "research/memo").unwrap().slug == "research/memo", "dry run moves nothing");

        let done = migrate_namespaces_in(&e.roots, Some(&e.ws()), false).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].error, None);
        assert_eq!(wiki::resolve_slug_in(&e.roots.wiki, "research/memo").as_deref(), Some("project/research/memo"));
        assert_eq!(item(&e.project(), "project/research/memo").folder.as_deref(), Some("research"));
        assert_eq!(wiki::get_doc_in(&e.roots.wiki, "research/x").unwrap().slug, "research/x");
        // Idempotent.
        assert!(migrate_namespaces_in(&e.roots, None, false).unwrap().is_empty());
    }
}
