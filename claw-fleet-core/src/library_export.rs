//! 资料 export: a folder, a hand-picked set of items, or a whole project's
//! library — wiki docs and artifacts alike — packed into one zip.
//!
//! The archive mirrors the tree the user was looking at: each item lands under
//! its schema folder's path (relative to the folder being exported), unfiled
//! items under one directory of their own. Docs go out as their original files
//! — a markdown doc as its `.md`, an `htmlDir` doc as a directory holding its
//! entry and assets — and artifacts as their stored bytes under their original
//! file name. Redirect stubs a move left behind are not docs
//! ([`crate::wiki::list_docs_in`] skips them) and so never appear.
//!
//! Written through [`crate::zip_stream::ZipStream`], one member at a time from
//! disk, so a project of video renders never has to fit in memory. A member
//! whose bytes are gone from the store is reported in
//! [`LibraryZip::skipped`] rather than failing the whole export.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::artifacts::{self, is_at_or_under};
use crate::library::{self, Roots};
use crate::wiki;
use crate::zip_stream::{sanitize_member_name, unique_member_name, ZipStream};

/// [`ExportSpec::scope`]: one folder and everything beneath it.
pub const SCOPE_FOLDER: &str = "folder";
/// [`ExportSpec::scope`]: the items named in [`ExportSpec::items`].
pub const SCOPE_ITEMS: &str = "items";
/// [`ExportSpec::scope`]: every item of the project, filed or not.
pub const SCOPE_PROJECT: &str = "project";

/// Directory unfiled items go under when the caller names none.
const DEFAULT_UNFILED_DIR: &str = "Unfiled";

/// Longest file or directory name an item gets in the archive, in chars.
/// Titles can be whole sentences; most filesystems cap a name at 255 bytes,
/// and a CJK char is three of them.
const MAX_NAME_CHARS: usize = 80;

/// What to export.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSpec {
    /// Any workspace of the project (a worktree resolves to its checkout).
    pub workspace_path: String,
    /// [`SCOPE_FOLDER`], [`SCOPE_ITEMS`] or [`SCOPE_PROJECT`].
    pub scope: String,
    /// The folder exported (folder scope), or the folder the selection was
    /// made in (items scope) — archive paths are relative to it. Ignored for a
    /// project export, whose paths are the full folder paths.
    #[serde(default)]
    pub folder: String,
    /// Items scope: `doc:<slug>` / `artifact:<id>`, the keys the clients
    /// already use for an entry.
    #[serde(default)]
    pub items: Vec<String>,
    /// Name of the directory unfiled items go under — UI copy, so the client
    /// passes it in its own language.
    #[serde(default)]
    pub unfiled_label: String,
}

/// What an export wrote.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryZip {
    /// Suggested archive name.
    pub filename: String,
    /// Library items packed — what the folder card counts.
    pub item_count: usize,
    /// Archive members; more than `item_count` when an `htmlDir` doc brought
    /// its assets.
    pub file_count: usize,
    pub total_bytes: u64,
    /// Titles of items whose stored bytes are missing.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// One item to pack: where it goes in the archive and where its bytes are.
struct Member {
    /// Archive directory (already relative), `""` = the archive root.
    dir: String,
    /// Sort key and the name shown in `skipped`.
    title: String,
    source: Source,
}

enum Source {
    /// A single file, archived as `name`.
    File { name: String, path: PathBuf },
    /// A directory (an `htmlDir` doc's version), archived as directory `name`.
    Dir { name: String, path: PathBuf },
    /// The bytes are gone.
    Missing,
}

/// Pack `spec` into a zip at `dest`.
pub fn export_to_file(spec: &ExportSpec, dest: &Path) -> Result<LibraryZip, String> {
    export_to_file_in(&Roots::home()?, spec, dest)
}

pub fn export_to_file_in(roots: &Roots, spec: &ExportSpec, dest: &Path) -> Result<LibraryZip, String> {
    // Resolve before creating the file, so a bad request leaves nothing behind.
    let (filename, members) = plan(roots, spec)?;
    let file = fs::File::create(dest).map_err(|e| format!("create '{}': {e}", dest.display()))?;
    let result = write_zip(filename, &members, std::io::BufWriter::new(file));
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// Pack `spec` into any sink.
pub fn export_in<W: Write>(roots: &Roots, spec: &ExportSpec, sink: W) -> Result<LibraryZip, String> {
    let (filename, members) = plan(roots, spec)?;
    write_zip(filename, &members, sink)
}

/// The suggested archive name and the members, in archive order.
fn plan(roots: &Roots, spec: &ExportSpec) -> Result<(String, Vec<Member>), String> {
    let records = library::project_records_in(roots, &spec.workspace_path)?;
    let base = match spec.scope.as_str() {
        SCOPE_PROJECT => String::new(),
        SCOPE_FOLDER | SCOPE_ITEMS => artifacts::normalize_dir_path(&spec.folder)?,
        other => return Err(format!("unknown export scope '{other}'")),
    };
    if spec.scope == SCOPE_FOLDER && base.is_empty() {
        return Err("a folder export needs a folder".to_string());
    }
    let wanted: HashSet<&str> = spec.items.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if spec.scope == SCOPE_ITEMS && wanted.is_empty() {
        return Err("no items selected".to_string());
    }
    let unfiled = match sanitize_segment(&spec.unfiled_label) {
        s if s.is_empty() => DEFAULT_UNFILED_DIR.to_string(),
        s => s,
    };

    let included = |key: &str, folder: &Option<String>| match spec.scope.as_str() {
        SCOPE_FOLDER => folder.as_deref().is_some_and(|f| is_at_or_under(f, &base)),
        SCOPE_ITEMS => wanted.contains(key),
        _ => true,
    };
    let dir_of = |folder: &Option<String>| -> String {
        match folder.as_deref() {
            None => unfiled.clone(),
            Some(f) if !base.is_empty() && is_at_or_under(f, &base) => {
                f[base.len()..].trim_start_matches('/').to_string()
            }
            Some(f) => f.to_string(),
        }
    };

    let mut members = Vec::new();
    for (doc, folder) in &records.docs {
        if !included(&format!("{}:{}", library::ITEM_DOC, doc.slug), folder) {
            continue;
        }
        let title = if doc.title.trim().is_empty() {
            wiki::slug_basename(&doc.slug).to_string()
        } else {
            doc.title.clone()
        };
        members.push(Member { dir: dir_of(folder), source: doc_source(roots, doc, &title), title });
    }
    for (a, folder) in &records.artifacts {
        if !included(&format!("{}:{}", library::ITEM_ARTIFACT, a.id), folder) {
            continue;
        }
        let blob = artifacts::blob_path(&roots.artifacts, a);
        let source = if blob.is_file() {
            Source::File { name: file_name(&a.name, &a.id), path: blob }
        } else {
            Source::Missing
        };
        let title = if a.title.is_empty() { a.name.clone() } else { a.title.clone() };
        members.push(Member { dir: dir_of(folder), title, source });
    }
    members.sort_by(|a, b| a.dir.cmp(&b.dir).then_with(|| a.title.cmp(&b.title)));

    let project_name = wiki::workspace_name_of(&records.key);
    let stem = match spec.scope.as_str() {
        SCOPE_PROJECT => project_name,
        _ if base.is_empty() => project_name,
        _ => base.rsplit('/').next().unwrap_or(&base).to_string(),
    };
    let stem = match sanitize_segment(&stem) {
        s if s.is_empty() => "library".to_string(),
        s => s,
    };
    Ok((format!("{stem}.zip"), members))
}

/// Where a doc's current version lives on disk, as an archive source.
fn doc_source(roots: &Roots, doc: &wiki::WikiDoc, title: &str) -> Source {
    if doc.current_version.is_empty() || doc.current_version.contains(['/', '\\']) || doc.current_version.contains("..") {
        return Source::Missing;
    }
    let version = roots
        .wiki
        .join(wiki::slug_to_dirname(&doc.slug))
        .join("versions")
        .join(&doc.current_version);
    let name = sanitize_segment(title);
    let name = if name.is_empty() { wiki::slug_basename(&doc.slug).to_string() } else { name };
    match doc.kind.as_str() {
        "htmlDir" if version.is_dir() => Source::Dir { name, path: version },
        "htmlDir" => Source::Missing,
        kind => {
            let entry = version.join(&doc.entry);
            if !entry.is_file() {
                return Source::Missing;
            }
            let ext = if kind == "markdown" { "md" } else { "html" };
            let name = if name.to_lowercase().ends_with(&format!(".{ext}")) {
                name
            } else {
                format!("{name}.{ext}")
            };
            Source::File { name, path: entry }
        }
    }
}

fn write_zip<W: Write>(filename: String, members: &[Member], sink: W) -> Result<LibraryZip, String> {
    let mut zip = ZipStream::new(sink);
    let mut used: HashSet<String> = HashSet::new();
    let mut report = LibraryZip { filename, ..Default::default() };
    let joined = |dir: &str, name: &str| if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") };

    for m in members {
        let dir = sanitize_dir(&m.dir);
        match &m.source {
            Source::Missing => report.skipped.push(m.title.clone()),
            Source::File { name, path } => {
                let member = unique_member_name(&sanitize_member_name(&joined(&dir, name)), &mut used);
                report.total_bytes += add_file(&mut zip, &member, path)?;
                report.file_count += 1;
                report.item_count += 1;
            }
            Source::Dir { name, path } => {
                // Claim the directory name itself so a second doc of the same
                // title gets `name (2)/…` instead of merging into this one.
                let root = unique_member_name(&sanitize_member_name(&joined(&dir, name)), &mut used);
                let mut files = Vec::new();
                collect_files(path, "", &mut files);
                files.sort();
                for rel in files {
                    let member = sanitize_member_name(&format!("{root}/{rel}"));
                    used.insert(member.clone());
                    report.total_bytes += add_file(&mut zip, &member, &path.join(&rel))?;
                    report.file_count += 1;
                }
                report.item_count += 1;
            }
        }
    }
    zip.finish().map_err(|e| format!("finish archive: {e}"))?;
    Ok(report)
}

fn add_file<W: Write>(zip: &mut ZipStream<W>, member: &str, path: &Path) -> Result<u64, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("open '{}': {e}", path.display()))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    zip.add(member, len, &mut f)
        .map_err(|e| format!("archive '{member}': {e}"))?;
    Ok(len)
}

/// Regular files under `dir` as `/`-joined relative paths; symlinks skipped.
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => collect_files(&entry.path(), &rel, out),
            Ok(ft) if ft.is_file() => out.push(rel),
            _ => {}
        }
    }
}

/// A title or file name as one path segment: separators and the characters
/// Windows refuses in a name become `_`, control characters go, and it is cut
/// to [`MAX_NAME_CHARS`] (keeping a short extension).
fn sanitize_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    if cleaned.chars().count() <= MAX_NAME_CHARS {
        return cleaned.to_string();
    }
    let ext = cleaned
        .rfind('.')
        .map(|i| &cleaned[i..])
        .filter(|e| e.len() <= 8 && e.len() > 1 && !e.contains(' '))
        .unwrap_or("");
    let stem: String = cleaned[..cleaned.len() - ext.len()]
        .chars()
        .take(MAX_NAME_CHARS - ext.chars().count())
        .collect();
    format!("{}{ext}", stem.trim_end())
}

/// Each segment of a folder path through [`sanitize_segment`].
fn sanitize_dir(dir: &str) -> String {
    dir.split('/')
        .map(sanitize_segment)
        .filter(|s| !s.is_empty() && s != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// An artifact's file name, or its id when the name has nothing usable.
fn file_name(name: &str, id: &str) -> String {
    match sanitize_segment(name) {
        s if s.is_empty() => id.to_string(),
        s => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::Artifact;
    use std::io::Cursor;
    use tempfile::TempDir;

    struct Env {
        _tmp: TempDir,
        roots: Roots,
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
        Env { _tmp: tmp, roots, ws }
    }

    impl Env {
        fn ws(&self) -> String {
            self.ws.display().to_string()
        }

        fn folder(&self, path: &str) -> library::LibraryFolder {
            library::create_folder_in(&self.roots, &self.ws(), path, None).unwrap()
        }

        /// A markdown doc filed in `folder` (`""` = unfiled).
        fn doc(&self, name: &str, title: &str, body: &str, folder: &str) -> wiki::WikiDoc {
            let prefix = if folder.is_empty() {
                String::new()
            } else {
                let f = library::list_folders_in(&self.roots, &self.ws())
                    .unwrap()
                    .into_iter()
                    .find(|f| f.path == folder)
                    .expect("folder exists");
                format!("{}/", f.wiki_prefix)
            };
            wiki::publish_text_in(
                &self.roots.wiki,
                &format!("{prefix}{name}"),
                Some(title),
                body,
                &self.ws,
                wiki::TextPublishMode::Replace,
            )
            .unwrap()
        }

        /// An `htmlDir` doc with one asset, filed in `folder`.
        fn site(&self, name: &str, title: &str, folder: &str) -> wiki::WikiDoc {
            let src = self.ws.join(format!("src-{name}"));
            fs::create_dir_all(src.join("assets")).unwrap();
            fs::write(src.join("index.html"), format!("<title>{title}</title>hi")).unwrap();
            fs::write(src.join("assets/app.js"), "console.log(1)").unwrap();
            let doc = wiki::publish_in(&self.roots.wiki, &src, Some(name), Some(title), &self.ws).unwrap();
            library::move_item_in(&self.roots, library::ITEM_DOC, &doc.slug, folder).unwrap();
            wiki::get_doc_in(&self.roots.wiki, &wiki::resolve_slug_in(&self.roots.wiki, &doc.slug).unwrap()).unwrap()
        }

        fn artifact(&self, file: &str, body: &[u8], folder: &str) -> Artifact {
            let dir = self.ws.join(format!("out-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            let src = dir.join(file);
            fs::write(&src, body).unwrap();
            let a = artifacts::add_in(&self.roots.artifacts, &src, None, None, &self.ws, None).unwrap();
            if !folder.is_empty() {
                library::move_item_in(&self.roots, library::ITEM_ARTIFACT, &a.id, folder).unwrap();
            }
            a
        }

        fn export(&self, spec: ExportSpec) -> (LibraryZip, Vec<(String, Vec<u8>)>) {
            let mut buf = Cursor::new(Vec::new());
            let report = export_in(&self.roots, &spec, &mut buf).unwrap();
            (report, read_zip(&buf.into_inner()))
        }

        fn spec(&self, scope: &str, folder: &str, items: &[String]) -> ExportSpec {
            ExportSpec {
                workspace_path: self.ws(),
                scope: scope.to_string(),
                folder: folder.to_string(),
                items: items.to_vec(),
                unfiled_label: "未归档".to_string(),
            }
        }
    }

    /// Members of a stored, data-descriptor zip, read with a real extractor.
    fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, bytes).unwrap();
        let script = r#"
import json, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    assert z.testzip() is None
    print(json.dumps([[i.filename, z.read(i.filename).decode("latin-1")] for i in z.infolist()]))
"#;
        let out = std::process::Command::new("python3").arg("-c").arg(script).arg(&archive).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let parsed: Vec<Vec<String>> = serde_json::from_slice(&out.stdout).unwrap();
        parsed
            .into_iter()
            .map(|p| (p[0].clone(), p[1].chars().map(|c| c as u8).collect()))
            .collect()
    }

    fn names(members: &[(String, Vec<u8>)]) -> Vec<&str> {
        let mut v: Vec<&str> = members.iter().map(|(n, _)| n.as_str()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_folder_export_packs_docs_and_artifacts_with_subfolders() {
        let e = env();
        e.folder("设计");
        e.folder("设计/截图");
        e.folder("其他");
        e.doc("spec", "接口说明", "# spec", "设计");
        e.site("mock", "交互稿", "设计/截图");
        e.artifact("shot.png", b"png", "设计/截图");
        e.artifact("elsewhere.pdf", b"pdf", "其他");
        e.artifact("loose.txt", b"x", "");

        let (report, members) = e.export(e.spec(SCOPE_FOLDER, "设计", &[]));
        assert_eq!(
            names(&members),
            vec![
                "截图/shot.png",
                "截图/交互稿/assets/app.js",
                "截图/交互稿/index.html",
                "接口说明.md",
            ]
        );
        assert_eq!(report.item_count, 3, "the folder card counts three items");
        assert_eq!(report.file_count, 4);
        assert_eq!(report.filename, "设计.zip");
        let md = members.iter().find(|(n, _)| n == "接口说明.md").unwrap();
        assert_eq!(md.1, b"# spec", "a markdown doc goes out as its original text");
    }

    #[test]
    fn a_project_export_takes_every_item_and_puts_unfiled_ones_aside() {
        let e = env();
        e.folder("a/b");
        e.doc("n1", "Note", "one", "a/b");
        e.artifact("r.pdf", b"pdf", "a");
        e.doc("loose", "Loose", "two", "");
        e.artifact("loose.bin", b"bin", "");

        let (report, members) = e.export(e.spec(SCOPE_PROJECT, "", &[]));
        assert_eq!(
            names(&members),
            vec!["a/b/Note.md", "a/r.pdf", "未归档/Loose.md", "未归档/loose.bin"]
        );
        assert_eq!(report.item_count, 4);
        assert_eq!(report.total_bytes, 3 + 3 + 3 + 3);
        assert_eq!(report.filename, "project.zip");
    }

    #[test]
    fn a_selection_export_takes_only_the_picked_items_relative_to_the_open_folder() {
        let e = env();
        e.folder("docs/sub");
        e.folder("other");
        let d = e.doc("keep", "Keep", "k", "docs/sub");
        e.doc("skip", "Skip", "s", "docs/sub");
        let a = e.artifact("pick.txt", b"p", "docs");
        let far = e.artifact("far.txt", b"f", "other");
        let items = vec![
            format!("doc:{}", wiki::resolve_slug_in(&e.roots.wiki, &d.slug).unwrap()),
            format!("artifact:{}", a.id),
            format!("artifact:{}", far.id),
        ];
        let (report, members) = e.export(e.spec(SCOPE_ITEMS, "docs", &items));
        // Outside the open folder (a search hit) keeps its full folder path.
        assert_eq!(names(&members), vec!["other/far.txt", "pick.txt", "sub/Keep.md"]);
        assert_eq!(report.item_count, 3);
    }

    #[test]
    fn same_named_items_are_suffixed_and_unsafe_titles_cleaned() {
        let e = env();
        e.folder("f");
        e.artifact("same.txt", b"1", "f");
        e.artifact("same.txt", b"2", "f");
        e.doc("x", "a/b: c?", "body", "f");
        e.site("s1", "Site", "f");
        e.site("s2", "Site", "f");
        let (_, members) = e.export(e.spec(SCOPE_FOLDER, "f", &[]));
        assert_eq!(
            names(&members),
            vec![
                "Site (2)/assets/app.js",
                "Site (2)/index.html",
                "Site/assets/app.js",
                "Site/index.html",
                "a_b_ c_.md",
                "same (2).txt",
                "same.txt",
            ]
        );
    }

    #[test]
    fn a_missing_blob_is_skipped_not_fatal_and_redirects_are_not_exported() {
        let e = env();
        e.folder("f");
        e.folder("g");
        let gone = e.artifact("gone.bin", b"zz", "f");
        e.artifact("here.bin", b"ok", "f");
        fs::remove_file(artifacts::blob_path(&e.roots.artifacts, &gone)).unwrap();
        // Moving a doc leaves a redirect stub at its old slug.
        let d = e.doc("moved", "Moved", "m", "f");
        library::move_item_in(&e.roots, library::ITEM_DOC, &d.slug, "g").unwrap();

        let (report, members) = e.export(e.spec(SCOPE_PROJECT, "", &[]));
        assert_eq!(names(&members), vec!["f/here.bin", "g/Moved.md"]);
        assert_eq!(report.skipped, vec!["gone.bin".to_string()]);
    }

    #[test]
    fn a_file_export_streams_to_disk_and_bad_requests_write_nothing() {
        let e = env();
        e.folder("f");
        e.artifact("a.txt", b"abc", "f");
        let dest = e.ws.join("out.zip");
        let r = export_to_file_in(&e.roots, &e.spec(SCOPE_FOLDER, "f", &[]), &dest).unwrap();
        assert_eq!(r.item_count, 1);
        assert_eq!(names(&read_zip(&fs::read(&dest).unwrap())), vec!["a.txt"]);

        let bad = e.ws.join("bad.zip");
        assert!(export_to_file_in(&e.roots, &e.spec(SCOPE_ITEMS, "", &[]), &bad).is_err());
        assert!(export_to_file_in(&e.roots, &e.spec(SCOPE_FOLDER, "", &[]), &bad).is_err());
        assert!(export_to_file_in(&e.roots, &e.spec("bogus", "", &[]), &bad).is_err());
        assert!(!bad.exists());
    }

    #[test]
    fn long_titles_are_cut_keeping_the_extension() {
        let long = "长".repeat(200);
        let cut = sanitize_segment(&format!("{long}.pdf"));
        assert_eq!(cut.chars().count(), MAX_NAME_CHARS);
        assert!(cut.ends_with(".pdf"));
        assert_eq!(sanitize_segment("  trailing dots... "), "trailing dots");
        assert_eq!(sanitize_dir("a/../b"), "a/b");
    }
}
