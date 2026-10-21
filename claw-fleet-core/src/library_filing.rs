//! Filing rules for what agents put into the library.
//!
//! Before this, an agent publishing a doc or adding an artifact picked any slug
//! or no folder at all, and the library filled with things "just put
//! anywhere". Now every `fleet__wiki publish` / `fleet__artifact add` (and the
//! matching CLI) must name one of the project's schema folders
//! ([`crate::library`]). Without one, or with a name the schema does not have,
//! the call is refused with the project's folder list and their descriptions,
//! so the agent can retry in one step.
//!
//! An agent may add a folder, but only explicitly — `new_folder` plus a
//! `reason` (why nothing existing fits) and a `description` (what belongs
//! there). The folder is recorded with a [`FolderOrigin`] naming the session
//! and the reason, and stays marked in the tree until the user accepts,
//! renames or deletes it.
//!
//! ## A project with no schema yet
//!
//! Its first publish is refused like any other, but the message says the
//! project has no folders and asks for the first one the new-folder way. That
//! one call seeds the schema, marked for review like every AI-created folder.
//! The alternative — running the upload organizer's LLM schema draft from
//! inside the tool call — would block an agent for tens of seconds on a model
//! call, and the agent already knows what it is filing better than a drafter
//! reading its README would.
//!
//! ## What is not constrained
//!
//! - Re-publishing an existing slug, or adding a file that becomes a new
//!   version of an existing artifact: it stays where it is, no folder needed.
//! - The user's own paths (the desktop's "save to wiki" dialog, the
//!   `/artifact_add` route) and the upload pipeline, which files through its
//!   own classifier: they call [`crate::wiki`] / [`crate::artifacts`]
//!   directly. Only the agent channels go through here.
//! - The user moving a doc in the library tree (`/wiki_move`, the desktop's
//!   drag): only `fleet wiki mv` / `fleet__wiki mv` go through
//!   [`move_wiki_in`].

use std::path::Path;

use crate::artifacts::{self, Artifact};
use crate::library::{self, FolderOrigin, LibraryFolder, Roots};
use crate::wiki::{self, WikiDoc};

/// What an agent said about where an item goes.
#[derive(Clone, Debug, Default)]
pub struct FilingArgs {
    /// A schema folder: display path, document prefix, or a unique leaf / key.
    pub folder: Option<String>,
    /// Create `folder` because no existing one fits.
    pub new_folder: bool,
    /// Why no existing folder fits (required with `new_folder`).
    pub reason: Option<String>,
    /// What belongs in the new folder (required with `new_folder`).
    pub description: Option<String>,
}

impl FilingArgs {
    /// Read `folder` / `new_folder` / `reason` / `description` from tool args.
    pub fn from_json(args: &serde_json::Value) -> Self {
        let s = |k: &str| {
            args.get(k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Self {
            folder: s("folder"),
            new_folder: args.get("new_folder").and_then(|v| v.as_bool()).unwrap_or(false),
            reason: s("reason"),
            description: s("description"),
        }
    }
}

/// Where an item ended up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filed {
    /// The schema folder (display path); `None` = unfiled (only possible for
    /// an item that was already unfiled and was re-published in place).
    pub folder: Option<String>,
    /// This call created the folder (it is marked for the user's review).
    pub created: bool,
    /// The item already existed and kept its folder.
    pub kept: bool,
}

impl Filed {
    /// One line for the agent on where the item went.
    pub fn describe(&self, reason: Option<&str>) -> String {
        match (&self.folder, self.kept, self.created) {
            (Some(f), true, _) => format!("It stays in folder '{f}'."),
            (None, true, _) => "It stays where it was (unfiled).".to_string(),
            (Some(f), false, true) => format!(
                "Filed in NEW folder '{f}' (reason: {}). The user sees it marked as AI-created in the library and may rename or delete it.",
                reason.unwrap_or("")
            ),
            (Some(f), false, false) => format!("Filed in folder '{f}'."),
            (None, false, _) => String::new(),
        }
    }
}

fn project_of(workspace: &Path) -> Result<String, String> {
    let key = library::project_key(&workspace.display().to_string());
    if key.is_empty() {
        Err("cannot tell which project this workspace belongs to".to_string())
    } else {
        Ok(key)
    }
}

/// The folder list an agent picks from, one per line:
/// `- <path>  (<doc prefix>) — <description>`.
pub fn folder_menu(folders: &[LibraryFolder]) -> String {
    folders
        .iter()
        .map(|f| {
            let mut line = format!("- {}", f.path);
            if f.wiki_prefix != f.path {
                line.push_str(&format!("  ({})", f.wiki_prefix));
            }
            if !f.description.is_empty() {
                line.push_str(&format!(" — {}", f.description));
            }
            if f.awaiting_review() {
                line.push_str("  [new, awaiting the user's review]");
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const NEW_FOLDER_HOW: &str = "pass new_folder=true with folder=\"<path>\", \
description=\"<one line on what belongs there>\" and reason=\"<why no existing folder fits>\" \
(CLI: --new-folder --folder <path> --description <text> --reason <text>). The user is shown \
every folder an agent creates, with the reason.";

/// The refusal for a missing or unknown folder.
fn refusal(project: &str, folders: &[LibraryFolder], asked: Option<&str>) -> String {
    let name = wiki::workspace_name_of(project);
    if folders.is_empty() {
        return format!(
            "Project '{name}' has no library folders yet, so this needs the first one. Pick a \
broad folder this kind of item belongs in and create it: {NEW_FOLDER_HOW}"
        );
    }
    let head = match asked {
        Some(a) => format!("'{a}' is not one of project '{name}''s library folders."),
        None => format!("A folder is required: file this into one of project '{name}''s library folders."),
    };
    format!(
        "{head} Retry with folder=\"<path>\" (CLI: --folder <path>), picking from:\n{}\n\n\
Only if none of these fits, create one: {NEW_FOLDER_HOW}",
        folder_menu(folders)
    )
}

/// The folder an agent's args name, creating it when they ask to. Returns the
/// folder and the paths this call created (parents first; empty = existed).
pub fn resolve_in(
    roots: &Roots,
    workspace: &Path,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(LibraryFolder, Vec<String>), String> {
    let project = project_of(workspace)?;
    let folders = library::list_folders_in(roots, &project)?;
    let asked = args.folder.as_deref();
    if let Some(f) = asked.and_then(|a| library::resolve_folder(&folders, a)) {
        return Ok((f.clone(), Vec::new()));
    }
    if !args.new_folder {
        return Err(refusal(&project, &folders, asked));
    }
    let path = asked.ok_or_else(|| {
        format!("new_folder needs folder=\"<path>\" — the folder to create. {NEW_FOLDER_HOW}")
    })?;
    let reason = args.reason.as_deref().ok_or_else(|| {
        "new_folder needs reason=\"<why no existing folder fits>\" — the user is shown it.".to_string()
    })?;
    let description = args.description.as_deref().ok_or_else(|| {
        "new_folder needs description=\"<one line on what belongs there>\" — later agents file by it.".to_string()
    })?;
    library::create_ai_folder_in(
        roots,
        &project,
        path,
        Some(description),
        None,
        FolderOrigin::agent(session_id, reason),
    )
}

/// Take back the folders this call created when the filing itself failed, so
/// a refused publish leaves no empty marked folder behind.
fn roll_back(roots: &Roots, folder: &LibraryFolder, created: &[String]) {
    for path in created.iter().rev() {
        let _ = library::delete_folder_in(roots, &folder.workspace_path, path);
    }
}

/// `fleet__wiki publish` under the filing rules. The doc's slug becomes
/// `<folder prefix>/<name>`, where the name is the basename of `slug` or, with
/// none, of the source's file name.
pub fn publish_wiki_in(
    roots: &Roots,
    source: &Path,
    slug: Option<&str>,
    title: Option<&str>,
    workspace: &Path,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(WikiDoc, Filed), String> {
    let slug = slug.map(wiki::normalize_slug).transpose()?;

    // Re-publishing a doc that exists: a new version, wherever it is filed.
    if let Some(s) = &slug {
        if let Ok(existing) = wiki::get_doc_in(&roots.wiki, s) {
            let doc = wiki::publish_in(&roots.wiki, source, Some(s), title, workspace)?;
            let project = library::project_key(&existing.workspace_path);
            let folders = library::list_folders_in(roots, &project).unwrap_or_default();
            let folder = library::doc_folder(&folders, &doc.slug);
            return Ok((doc, Filed { folder, kept: true, created: false }));
        }
    }

    // A slug whose directory already is a folder's prefix names that folder
    // (`--slug invoices/jd-0920`), so the old habit keeps working.
    let mut args = args.clone();
    if args.folder.is_none() {
        if let Some(dir) = slug.as_deref().map(library::slug_dir).filter(|d| !d.is_empty()) {
            let project = project_of(workspace)?;
            let folders = library::list_folders_in(roots, &project)?;
            if folders.iter().any(|f| f.wiki_prefix == dir) {
                args.folder = Some(dir.to_string());
                args.new_folder = false;
            }
        }
    }

    let (folder, created) = resolve_in(roots, workspace, &args, session_id)?;
    let name = match &slug {
        Some(s) => wiki::slug_basename(s).to_string(),
        None => wiki::derived_slug(source)?,
    };
    let target = wiki::normalize_slug(&format!("{}/{name}", folder.wiki_prefix))?;
    match wiki::publish_in(&roots.wiki, source, Some(&target), title, workspace) {
        Ok(doc) => Ok((
            doc,
            Filed { folder: Some(folder.path.clone()), created: !created.is_empty(), kept: false },
        )),
        Err(e) => {
            roll_back(roots, &folder, &created);
            Err(e)
        }
    }
}

/// `fleet__wiki mv` under the filing rules: an agent may only move a doc into
/// one of its project's schema folders. The target is `<folder prefix>/<name>`
/// when a folder is named (`name` = basename of `to`, else of the current
/// slug); without one, `to`'s directory must be exactly a folder's prefix.
/// Moving a doc out of every folder (back to unfiled) is refused. The old slug
/// keeps resolving through the redirect [`wiki::move_doc_in`] leaves.
pub fn move_wiki_in(
    roots: &Roots,
    from: &str,
    to: Option<&str>,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(WikiDoc, Filed), String> {
    let doc = wiki::get_doc_in(&roots.wiki, &wiki::normalize_slug(from)?)?;
    let to = to.map(wiki::normalize_slug).transpose()?;
    let workspace = Path::new(&doc.workspace_path);
    let project = project_of(workspace)?;

    let mut args = args.clone();
    if args.folder.is_none() && !args.new_folder {
        let folders = library::list_folders_in(roots, &project)?;
        let dir = to.as_deref().map(library::slug_dir).unwrap_or("");
        if dir.is_empty() {
            return Err(format!(
                "Moving '{}' needs a destination folder; agents may not move docs out of the library folders. {}",
                doc.slug,
                refusal(&project, &folders, None)
            ));
        }
        if !folders.iter().any(|f| f.wiki_prefix == dir) {
            return Err(refusal(&project, &folders, Some(dir)));
        }
        args.folder = Some(dir.to_string());
    }

    let (folder, created) = resolve_in(roots, workspace, &args, session_id)?;
    let name = wiki::slug_basename(to.as_deref().unwrap_or(&doc.slug)).to_string();
    let target = wiki::normalize_slug(&format!("{}/{name}", folder.wiki_prefix))?;
    if target == doc.slug {
        return Ok((doc, Filed { folder: Some(folder.path.clone()), created: false, kept: true }));
    }
    match wiki::move_doc_in(&roots.wiki, &doc.slug, &target) {
        Ok(moved) => Ok((
            moved,
            Filed { folder: Some(folder.path.clone()), created: !created.is_empty(), kept: false },
        )),
        Err(e) => {
            roll_back(roots, &folder, &created);
            Err(e)
        }
    }
}

/// `fleet__artifact add` under the filing rules.
pub fn add_artifact_in(
    roots: &Roots,
    source: &Path,
    title: Option<&str>,
    note: Option<&str>,
    workspace: &Path,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(Artifact, Filed), String> {
    // The same deliverable again: a new version of the card it has, which
    // keeps its folder.
    if artifacts::existing_deliverable_in(&roots.artifacts, source, workspace).is_some() {
        let a = artifacts::add_in(&roots.artifacts, source, title, note, workspace, session_id)?;
        let project = library::project_key(&a.workspace_path);
        let folders = library::list_folders_in(roots, &project).unwrap_or_default();
        let folder = library::artifact_folder(&folders, &a);
        return Ok((a, Filed { folder, kept: true, created: false }));
    }

    let (folder, created) = resolve_in(roots, workspace, args, session_id)?;
    let added = match artifacts::add_in(&roots.artifacts, source, title, note, workspace, session_id) {
        Ok(a) => a,
        Err(e) => {
            roll_back(roots, &folder, &created);
            return Err(e);
        }
    };
    match artifacts::update_in(&roots.artifacts, &added.id, None, None, None, Some(&folder.path)) {
        Ok(a) => Ok((a, Filed { folder: Some(folder.path.clone()), created: !created.is_empty(), kept: false })),
        Err(e) => {
            let _ = artifacts::delete_in(&roots.artifacts, &added.id);
            roll_back(roots, &folder, &created);
            Err(e)
        }
    }
}

/// The `folders` action: the project's schema as the agent files by it.
pub fn folders_text_in(roots: &Roots, workspace: &Path) -> Result<String, String> {
    let project = project_of(workspace)?;
    let folders = library::list_folders_in(roots, &project)?;
    let name = wiki::workspace_name_of(&project);
    if folders.is_empty() {
        return Ok(format!(
            "Project '{name}' has no library folders yet. The first publish creates one: {NEW_FOLDER_HOW}"
        ));
    }
    Ok(format!(
        "Library folders of project '{name}' (path  (doc prefix) — what belongs there). Pass the path as folder=\"…\":\n{}",
        folder_menu(&folders)
    ))
}

// Home-store wrappers.

pub fn publish_wiki(
    source: &Path,
    slug: Option<&str>,
    title: Option<&str>,
    workspace: &Path,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(WikiDoc, Filed), String> {
    publish_wiki_in(&Roots::home()?, source, slug, title, workspace, args, session_id)
}

pub fn add_artifact(
    source: &Path,
    title: Option<&str>,
    note: Option<&str>,
    workspace: &Path,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(Artifact, Filed), String> {
    add_artifact_in(&Roots::home()?, source, title, note, workspace, args, session_id)
}

pub fn move_wiki(
    from: &str,
    to: Option<&str>,
    args: &FilingArgs,
    session_id: Option<&str>,
) -> Result<(WikiDoc, Filed), String> {
    move_wiki_in(&Roots::home()?, from, to, args, session_id)
}

pub fn folders_text(workspace: &Path) -> Result<String, String> {
    folders_text_in(&Roots::home()?, workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Env {
        _tmp: TempDir,
        roots: Roots,
        ws: std::path::PathBuf,
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
        Env { _tmp: tmp, roots, ws }
    }

    impl Env {
        fn ws(&self) -> String {
            self.ws.display().to_string()
        }
        fn file(&self, name: &str, body: &str) -> std::path::PathBuf {
            let p = self.ws.join(name);
            fs::write(&p, body).unwrap();
            p
        }
        fn folders(&self) -> Vec<LibraryFolder> {
            library::list_folders_in(&self.roots, &self.ws()).unwrap()
        }
        fn schema(&self) {
            library::create_keyed_folder_in(&self.roots, &self.ws(), "调研", Some("调研报告与分析"), Some("research")).unwrap();
            library::create_keyed_folder_in(&self.roots, &self.ws(), "发票", Some("报销发票"), Some("invoices")).unwrap();
        }
    }

    fn folder(f: &str) -> FilingArgs {
        FilingArgs { folder: Some(f.into()), ..Default::default() }
    }

    #[test]
    fn publish_without_a_folder_is_refused_with_the_folder_list() {
        let e = env();
        e.schema();
        let src = e.file("report.md", "# Report");
        let err = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &FilingArgs::default(), None).unwrap_err();
        assert!(err.contains("A folder is required"), "{err}");
        assert!(err.contains("- 调研  (research) — 调研报告与分析"), "{err}");
        assert!(err.contains("new_folder=true"), "{err}");
        assert!(wiki::list_docs_in(&e.roots.wiki).is_empty(), "nothing published");

        let err = add_artifact_in(&e.roots, &src, None, None, &e.ws, &folder("made-up"), None).unwrap_err();
        assert!(err.contains("'made-up' is not one of"), "{err}");
        assert!(err.contains("- 发票  (invoices) — 报销发票"), "{err}");
        assert!(artifacts::list_in(&e.roots.artifacts).is_empty(), "nothing stored");
    }

    #[test]
    fn a_listed_folder_by_path_prefix_or_leaf_is_accepted() {
        let e = env();
        e.schema();
        let src = e.file("q3.md", "# Q3");
        let (doc, filed) = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &folder("调研"), None).unwrap();
        assert_eq!(doc.slug, "research/q3");
        assert_eq!(filed, Filed { folder: Some("调研".into()), created: false, kept: false });

        // By prefix, with an explicit slug whose directory is dropped.
        let src2 = e.file("x.md", "# X");
        let (doc, _) = publish_wiki_in(&e.roots, &src2, Some("old/place/memo"), None, &e.ws, &folder("research"), None).unwrap();
        assert_eq!(doc.slug, "research/memo");

        // A slug already under a folder's prefix names that folder.
        let src3 = e.file("y.md", "# Y");
        let (doc, filed) = publish_wiki_in(&e.roots, &src3, Some("invoices/jd-0920"), None, &e.ws, &FilingArgs::default(), None).unwrap();
        assert_eq!((doc.slug.as_str(), filed.folder.as_deref()), ("invoices/jd-0920", Some("发票")));

        let pdf = e.file("receipt.pdf", "%PDF");
        let (a, filed) = add_artifact_in(&e.roots, &pdf, Some("Receipt"), None, &e.ws, &folder("invoices"), Some("s1")).unwrap();
        assert_eq!(a.path, "发票");
        assert!(!filed.created);
        let p = library::tree_in(&e.roots, Some(&e.ws())).remove(0);
        assert!(p.items.iter().all(|i| i.folder.is_some()), "everything filed: {:?}", p.items);
    }

    #[test]
    fn a_new_folder_needs_reason_and_description_and_is_marked() {
        let e = env();
        e.schema();
        let src = e.file("deck.md", "# Deck");
        let mut args = FilingArgs { folder: Some("合同".into()), new_folder: true, ..Default::default() };
        let err = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &args, None).unwrap_err();
        assert!(err.contains("reason"), "{err}");
        args.reason = Some("no folder for contracts".into());
        let err = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &args, None).unwrap_err();
        assert!(err.contains("description"), "{err}");
        assert_eq!(e.folders().len(), 2, "a refused call creates nothing");

        args.description = Some("签过的合同".into());
        let (doc, filed) = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &args, Some("sess-1")).unwrap();
        assert!(filed.created);
        assert_eq!(doc.slug, "hetong/deck", "a CJK folder gets a pinyin key");
        let f = e.folders().into_iter().find(|f| f.path == "合同").unwrap();
        assert_eq!(f.description, "签过的合同");
        let o = f.origin.as_ref().unwrap();
        assert_eq!((o.source.as_str(), o.session_id.as_deref(), o.reason.as_str()), ("agent", Some("sess-1"), "no folder for contracts"));
        assert!(f.awaiting_review());
        assert!(filed.describe(Some("no folder for contracts")).contains("NEW folder '合同'"));

        // Accepting clears the marker but keeps the record.
        let f = library::accept_folder_in(&e.roots, &e.ws(), "合同").unwrap();
        assert!(!f.awaiting_review() && f.origin.is_some());

        // Asking for a new folder that already exists just files into it.
        let src2 = e.file("deck2.md", "# Deck 2");
        let args = FilingArgs { folder: Some("发票".into()), new_folder: true, reason: Some("r".into()), description: Some("other".into()) };
        let (_, filed) = publish_wiki_in(&e.roots, &src2, None, None, &e.ws, &args, None).unwrap();
        assert!(!filed.created);
        let inv = e.folders().into_iter().find(|f| f.path == "发票").unwrap();
        assert_eq!(inv.description, "报销发票", "the user's description is not overwritten");
        assert!(inv.origin.is_none());
    }

    #[test]
    fn a_project_without_a_schema_is_seeded_by_the_first_publish() {
        let e = env();
        let src = e.file("notes.md", "# Notes");
        let err = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &FilingArgs::default(), None).unwrap_err();
        assert!(err.contains("has no library folders yet"), "{err}");
        assert!(err.contains("new_folder=true"), "{err}");
        let text = folders_text_in(&e.roots, &e.ws).unwrap();
        assert!(text.contains("no library folders yet"), "{text}");

        let args = FilingArgs {
            folder: Some("architecture".into()),
            new_folder: true,
            reason: Some("first doc of this project".into()),
            description: Some("Architecture notes".into()),
        };
        let (doc, filed) = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &args, None).unwrap();
        assert_eq!(doc.slug, "architecture/notes");
        assert!(filed.created);
        let text = folders_text_in(&e.roots, &e.ws).unwrap();
        assert!(text.contains("- architecture — Architecture notes  [new, awaiting the user's review]"), "{text}");
    }

    #[test]
    fn republishing_an_existing_slug_or_artifact_keeps_its_place() {
        let e = env();
        e.schema();
        let src = e.file("plan.md", "# Plan v1");
        let (doc, _) = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &folder("调研"), None).unwrap();
        fs::write(&src, "# Plan v2").unwrap();
        let (again, filed) = publish_wiki_in(&e.roots, &src, Some(&doc.slug), None, &e.ws, &FilingArgs::default(), None).unwrap();
        assert_eq!(again.slug, doc.slug);
        assert_eq!(again.versions.len(), 2);
        assert_eq!(filed, Filed { folder: Some("调研".into()), created: false, kept: true });

        let pdf = e.file("out.pdf", "%PDF-1");
        let (a, _) = add_artifact_in(&e.roots, &pdf, None, None, &e.ws, &folder("发票"), None).unwrap();
        fs::write(&pdf, "%PDF-2").unwrap();
        let (b, filed) = add_artifact_in(&e.roots, &pdf, None, None, &e.ws, &FilingArgs::default(), None).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.path, "发票");
        assert!(filed.kept);
    }

    #[test]
    fn a_failed_publish_takes_back_the_folder_it_created() {
        let e = env();
        e.schema();
        let bad = e.file("data.xlsx", "PK");
        let args = FilingArgs {
            folder: Some("表格/2026".into()),
            new_folder: true,
            reason: Some("r".into()),
            description: Some("d".into()),
        };
        assert!(publish_wiki_in(&e.roots, &bad, None, None, &e.ws, &args, None).is_err());
        assert!(!e.folders().iter().any(|f| f.path.starts_with("表格")), "parent taken back too");
    }

    #[test]
    fn agent_mv_is_held_to_the_schema_and_leaves_the_old_slug_resolving() {
        let e = env();
        e.schema();
        let src = e.file("memo.md", "# Memo");
        let (doc, _) = publish_wiki_in(&e.roots, &src, None, None, &e.ws, &folder("调研"), None).unwrap();
        assert_eq!(doc.slug, "research/memo");

        // Out of every folder, or into a directory the schema lacks: refused.
        let err = move_wiki_in(&e.roots, "research/memo", Some("memo"), &FilingArgs::default(), None).unwrap_err();
        assert!(err.contains("may not move docs out"), "{err}");
        let err = move_wiki_in(&e.roots, "research/memo", Some("misc/memo"), &FilingArgs::default(), None).unwrap_err();
        assert!(err.contains("'misc' is not one of"), "{err}");
        assert!(wiki::get_doc_in(&e.roots.wiki, "research/memo").unwrap().slug == "research/memo", "not moved");

        // Into a folder's prefix, or by naming the folder.
        let (moved, filed) = move_wiki_in(&e.roots, "research/memo", Some("invoices/memo-0920"), &FilingArgs::default(), None).unwrap();
        assert_eq!((moved.slug.as_str(), filed.folder.as_deref()), ("invoices/memo-0920", Some("发票")));
        let (moved, _) = move_wiki_in(&e.roots, "invoices/memo-0920", None, &folder("调研"), None).unwrap();
        assert_eq!(moved.slug, "research/memo-0920");
        assert_eq!(wiki::resolve_slug_in(&e.roots.wiki, "research/memo").as_deref(), Some("research/memo-0920"));
        assert_eq!(wiki::get_doc_in(&e.roots.wiki, "invoices/memo-0920").unwrap().slug, "research/memo-0920");

        // A new folder needs the same reason + description, and is marked.
        let args = FilingArgs { folder: Some("合同".into()), new_folder: true, ..Default::default() };
        assert!(move_wiki_in(&e.roots, "research/memo-0920", None, &args, None).is_err());
        let args = FilingArgs { reason: Some("contracts".into()), description: Some("签过的合同".into()), ..args };
        let (moved, filed) = move_wiki_in(&e.roots, "research/memo", None, &args, Some("s9")).unwrap();
        assert_eq!(moved.slug, "hetong/memo-0920");
        assert!(filed.created);
        assert!(e.folders().iter().find(|f| f.path == "合同").unwrap().awaiting_review());
    }
}
