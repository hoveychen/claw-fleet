//! Staging an upload: copying dropped files, walking dropped folders and
//! expanding archives into a batch's staging directory, one numbered
//! sub-directory per file.
//!
//! Everything here treats its input as hostile, because an archive is: member
//! paths are rebuilt segment by segment (`../../.ssh/x`, `/etc/x` and `C:\x`
//! never leave the staging dir — "zip-slip"), sizes are counted on the bytes
//! actually written rather than the sizes an archive declares (a bomb lies),
//! links and devices are skipped, and the whole batch shares one budget of
//! files and bytes.
//!
//! Junk the OS leaves behind (`__MACOSX/`, `.DS_Store`, `._x` AppleDouble
//! files, Office `~$x` lock files, dot-directories like `.git`) is skipped
//! without being reported — it is noise, not something the user meant to file.
//! Everything else that is not staged lands in [`Stager::rejected`] with a
//! reason, so nothing disappears silently.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Caps one batch is staged under.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    /// How many archives deep a member may sit (an archive inside an archive
    /// inside an archive is depth 3). Deeper archives are staged as files.
    pub max_archive_depth: usize,
    /// A member that inflates past `compressed × max_ratio` (and past
    /// [`RATIO_FLOOR`]) is treated as a bomb.
    pub max_ratio: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: 1000,
            max_total_bytes: 2 * 1024 * 1024 * 1024,
            max_file_bytes: 512 * 1024 * 1024,
            max_archive_depth: 3,
            max_ratio: 200,
        }
    }
}

/// Inflation below this size is never called a bomb, whatever the ratio — a
/// 2 KB zip of a 900 KB CSV is fine.
pub const RATIO_FLOOR: u64 = 16 * 1024 * 1024;

/// Deepest a dropped folder is walked.
const MAX_DIR_DEPTH: usize = 32;

/// One file ready for extraction and filing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedFile {
    /// Index of its staging sub-directory; item ids derive from it.
    pub index: usize,
    /// Path as the user knows it: `合同/施工合同.docx`, `说明书.zip/洗碗机.pdf`.
    pub rel_path: String,
    /// Where the bytes now are.
    pub staged: PathBuf,
    pub size: u64,
    /// sha256, lowercase hex.
    pub sha256: String,
}

/// Something in the upload that was not staged, and why.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    pub rel_path: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    /// A single gzip-compressed file (`data.csv.gz`).
    Gz,
}

/// What an archive-looking name is: one we expand, one we refuse (with its
/// format for the message), or not an archive at all (`None`). Office files
/// are zips inside but are documents, so only these extensions count.
pub fn archive_kind(name: &str) -> Option<Result<ArchiveKind, &'static str>> {
    let lower = name.to_lowercase();
    let ends = |s: &str| lower.ends_with(s);
    if ends(".zip") {
        return Some(Ok(ArchiveKind::Zip));
    }
    if ends(".tar.gz") || ends(".tgz") {
        return Some(Ok(ArchiveKind::TarGz));
    }
    if ends(".tar") {
        return Some(Ok(ArchiveKind::Tar));
    }
    if ends(".gz") {
        return Some(Ok(ArchiveKind::Gz));
    }
    const UNSUPPORTED: &[(&str, &str)] = &[
        (".rar", "rar"),
        (".7z", "7z"),
        (".tar.xz", "tar.xz"),
        (".txz", "tar.xz"),
        (".xz", "xz"),
        (".tar.bz2", "tar.bz2"),
        (".tbz2", "tar.bz2"),
        (".tbz", "tar.bz2"),
        (".bz2", "bz2"),
        (".tar.zst", "tar.zst"),
        (".zst", "zst"),
        (".lz", "lzip"),
        (".lzma", "lzma"),
        (".cab", "cab"),
        (".arj", "arj"),
        (".lzh", "lzh"),
        (".lha", "lha"),
        (".z", "compress (.Z)"),
        (".iso", "iso"),
        (".dmg", "dmg"),
    ];
    UNSUPPORTED
        .iter()
        .find(|(ext, _)| ends(ext))
        .map(|(_, fmt)| Err(*fmt))
}

/// True for the OS and editor droppings nobody means to upload.
pub fn is_junk(name: &str) -> bool {
    matches!(name, "__MACOSX" | "Thumbs.db" | "desktop.ini")
        || name.starts_with('.')
        || name.starts_with("~$")
}

/// An archive member's path as safe segments, or `None` when it cannot be one.
///
/// Backslashes count as separators (Windows zips), empty and `.` segments are
/// dropped, and anything that could climb out or root itself — `..`, a drive
/// letter, NUL or other control characters — refuses the whole path rather
/// than being "cleaned" into something the archive's author did not write.
pub fn safe_member_path(raw: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for seg in raw.split(['/', '\\']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.chars().any(char::is_control) {
            return None;
        }
        if seg.len() == 2 && seg.ends_with(':') {
            return None;
        }
        out.push(seg.to_string());
    }
    (!out.is_empty()).then_some(out)
}

/// A zip member name from its raw bytes: UTF-8 when it is (macOS and Linux
/// write UTF-8, often without setting the flag), else GBK/GB18030 when that
/// decodes cleanly (Chinese Windows), else the lossy UTF-8 reading.
pub fn decode_member_name(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let (text, _, had_errors) = encoding_rs::GB18030.decode(raw);
    if !had_errors {
        return text.into_owned();
    }
    String::from_utf8_lossy(raw).into_owned()
}

fn join_rel(base: &str, rest: &str) -> String {
    if base.is_empty() {
        rest.to_string()
    } else {
        format!("{base}/{rest}")
    }
}

/// Stages files into `dest/<index>/<name>`. One per batch.
pub struct Stager {
    dest: PathBuf,
    limits: Limits,
    pub files: Vec<StagedFile>,
    pub rejected: Vec<Rejected>,
    /// Junk entries skipped silently (see the module docs).
    pub skipped_junk: usize,
    total_bytes: u64,
    next_index: usize,
    tmp_seq: usize,
}

/// Why one file could not be written.
enum CopyError {
    TooLarge(&'static str),
    Io(io::Error),
}

/// Copy `reader` to `path` within what is left of the batch budget (`used`
/// bytes already staged), hashing as it goes. Counts the bytes actually read,
/// so an archive that under-declares its sizes is still stopped.
fn copy_capped(
    limits: &Limits,
    used: u64,
    reader: &mut dyn Read,
    path: &Path,
    compressed: Option<u64>,
) -> Result<(u64, String), CopyError> {
    let budget = limits.max_total_bytes.saturating_sub(used);
    let ratio_cap = compressed
        .map(|c| c.saturating_mul(limits.max_ratio).max(RATIO_FLOOR))
        .unwrap_or(u64::MAX);
    let cap = limits.max_file_bytes.min(budget).min(ratio_cap);
    let mut out = fs::File::create(path).map_err(CopyError::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(CopyError::Io)?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > cap {
            drop(out);
            let _ = fs::remove_file(path);
            return Err(CopyError::TooLarge(if cap == ratio_cap {
                "compression ratio too high (possible zip bomb)"
            } else if cap == budget && budget < limits.max_file_bytes {
                "over the size limit for one upload"
            } else {
                "file too large"
            }));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(CopyError::Io)?;
    }
    out.flush().map_err(CopyError::Io)?;
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((written, hex))
}

impl Stager {
    pub fn new(dest: &Path, limits: Limits) -> Self {
        Self {
            dest: dest.to_path_buf(),
            limits,
            files: Vec::new(),
            rejected: Vec::new(),
            skipped_junk: 0,
            total_bytes: 0,
            next_index: 0,
            tmp_seq: 0,
        }
    }

    fn reject(&mut self, rel: &str, reason: impl Into<String>) {
        self.rejected.push(Rejected {
            rel_path: rel.to_string(),
            reason: reason.into(),
        });
    }

    fn full(&self) -> bool {
        self.files.len() >= self.limits.max_files
    }

    /// Stage a file or folder from the local disk under the name `rel`
    /// (usually its own file name).
    pub fn add_path(&mut self, src: &Path, rel: &str) {
        self.add_path_at(src, rel, 0);
    }

    fn add_path_at(&mut self, src: &Path, rel: &str, dir_depth: usize) {
        let leaf = rel.rsplit('/').next().unwrap_or(rel);
        if is_junk(leaf) {
            self.skipped_junk += 1;
            return;
        }
        let meta = match fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => return self.reject(rel, format!("cannot read: {e}")),
        };
        if meta.file_type().is_symlink() {
            return self.reject(rel, "symbolic link skipped");
        }
        if meta.is_dir() {
            if dir_depth >= MAX_DIR_DEPTH {
                return self.reject(rel, "folder nested too deep");
            }
            let mut entries: Vec<_> = match fs::read_dir(src) {
                Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
                Err(e) => return self.reject(rel, format!("cannot list folder: {e}")),
            };
            entries.sort();
            for child in entries {
                let name = child
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.add_path_at(&child, &join_rel(rel, &name), dir_depth + 1);
            }
            return;
        }
        if !meta.is_file() {
            return self.reject(rel, "not a regular file");
        }
        match archive_kind(leaf) {
            Some(Ok(kind)) => self.expand_file(src, rel, kind, 1),
            Some(Err(fmt)) => self.reject(rel, format!("unsupported archive format ({fmt}) — extract it and upload the files")),
            None => match fs::File::open(src) {
                Ok(mut f) => self.stage_reader(rel, &mut f, None),
                Err(e) => self.reject(rel, format!("cannot read: {e}")),
            },
        }
    }

    /// Stage one file's bytes from a reader; `compressed` is the archive's
    /// stored size for the ratio guard.
    fn stage_reader(&mut self, rel: &str, reader: &mut dyn Read, compressed: Option<u64>) {
        if self.full() {
            return self.reject(rel, format!("over the {}-file limit for one upload", self.limits.max_files));
        }
        let leaf = rel.rsplit('/').next().unwrap_or(rel);
        let name = crate::user_attachments::sanitize_name_with(leaf, "file.bin");
        let index = self.next_index;
        let dir = self.dest.join(format!("{index:04}"));
        if let Err(e) = fs::create_dir_all(&dir) {
            return self.reject(rel, format!("staging failed: {e}"));
        }
        let path = dir.join(&name);
        match copy_capped(&self.limits, self.total_bytes, reader, &path, compressed) {
            Ok((size, sha256)) => {
                self.next_index += 1;
                self.total_bytes += size;
                self.files.push(StagedFile {
                    index,
                    rel_path: rel.to_string(),
                    staged: path,
                    size,
                    sha256,
                });
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&dir);
                self.reject(
                    rel,
                    match e {
                        CopyError::TooLarge(why) => why.to_string(),
                        CopyError::Io(e) => format!("read failed: {e}"),
                    },
                );
            }
        }
    }

    /// A temp path under the staging dir for an archive member that must be
    /// seekable (a zip inside a zip).
    fn temp_path(&mut self) -> PathBuf {
        self.tmp_seq += 1;
        let _ = fs::create_dir_all(&self.dest);
        self.dest.join(format!(".tmp-{}", self.tmp_seq))
    }

    fn expand_file(&mut self, src: &Path, rel: &str, kind: ArchiveKind, depth: usize) {
        let file = match fs::File::open(src) {
            Ok(f) => f,
            Err(e) => return self.reject(rel, format!("cannot read: {e}")),
        };
        match kind {
            ArchiveKind::Zip => self.expand_zip(file, rel, depth),
            ArchiveKind::Tar => self.expand_tar(file, rel, depth),
            ArchiveKind::TarGz => self.expand_tar(flate2::read::GzDecoder::new(file), rel, depth),
            ArchiveKind::Gz => {
                let leaf = rel.rsplit('/').next().unwrap_or(rel);
                let inner = &leaf[..leaf.len() - 3];
                let inner = if inner.is_empty() { "file" } else { inner };
                let mut dec = flate2::read::GzDecoder::new(file);
                let size = fs::metadata(src).map(|m| m.len()).ok();
                self.stage_member(&join_rel(rel, inner), &mut dec, size, depth);
            }
        }
    }

    /// Stage one archive member: expand it when it is itself an archive and
    /// depth allows, else stage its bytes.
    fn stage_member(&mut self, rel: &str, reader: &mut dyn Read, compressed: Option<u64>, depth: usize) {
        let leaf = rel.rsplit('/').next().unwrap_or(rel);
        match archive_kind(leaf) {
            Some(Ok(kind)) if depth < self.limits.max_archive_depth => {
                let tmp = self.temp_path();
                match copy_capped(&self.limits, self.total_bytes, reader, &tmp, compressed) {
                    Ok(_) => self.expand_file(&tmp, rel, kind, depth + 1),
                    Err(CopyError::TooLarge(why)) => self.reject(rel, why),
                    Err(CopyError::Io(e)) => self.reject(rel, format!("read failed: {e}")),
                }
                let _ = fs::remove_file(&tmp);
            }
            Some(Err(fmt)) => self.reject(rel, format!("unsupported archive format ({fmt}) — extract it and upload the files")),
            _ => self.stage_reader(rel, reader, compressed),
        }
    }

    fn expand_zip(&mut self, file: fs::File, rel: &str, depth: usize) {
        let mut zip = match zip::ZipArchive::new(io::BufReader::new(file)) {
            Ok(z) => z,
            Err(e) => return self.reject(rel, format!("not a readable zip: {e}")),
        };
        for i in 0..zip.len() {
            let mut entry = match zip.by_index(i) {
                Ok(e) => e,
                Err(e) => {
                    self.reject(&join_rel(rel, &format!("#{i}")), format!("unreadable entry: {e}"));
                    continue;
                }
            };
            let name = decode_member_name(entry.name_raw());
            let Some(segments) = safe_member_path(&name) else {
                self.reject(&join_rel(rel, &name), "unsafe path inside archive");
                continue;
            };
            if segments.iter().any(|s| is_junk(s)) {
                self.skipped_junk += 1;
                continue;
            }
            let member_rel = join_rel(rel, &segments.join("/"));
            if entry.is_dir() {
                continue;
            }
            const S_IFMT: u32 = 0o170000;
            const S_IFREG: u32 = 0o100000;
            if let Some(mode) = entry.unix_mode() {
                if mode & S_IFMT != 0 && mode & S_IFMT != S_IFREG {
                    self.reject(&member_rel, "link or special file skipped");
                    continue;
                }
            }
            let compressed = entry.compressed_size();
            self.stage_member(&member_rel, &mut entry, Some(compressed), depth);
        }
    }

    fn expand_tar(&mut self, reader: impl Read, rel: &str, depth: usize) {
        let mut archive = tar::Archive::new(reader);
        let entries = match archive.entries() {
            Ok(e) => e,
            Err(e) => return self.reject(rel, format!("not a readable tar: {e}")),
        };
        for entry in entries {
            let mut entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    // A corrupt header ends the stream; nothing after it is
                    // reachable.
                    self.reject(rel, format!("tar stream broken: {e}"));
                    break;
                }
            };
            let name = decode_member_name(&entry.path_bytes());
            let Some(segments) = safe_member_path(&name) else {
                self.reject(&join_rel(rel, &name), "unsafe path inside archive");
                continue;
            };
            if segments.iter().any(|s| is_junk(s)) {
                self.skipped_junk += 1;
                continue;
            }
            let member_rel = join_rel(rel, &segments.join("/"));
            match entry.header().entry_type() {
                tar::EntryType::Regular | tar::EntryType::Continuous => {}
                tar::EntryType::Directory
                | tar::EntryType::XGlobalHeader
                | tar::EntryType::XHeader
                | tar::EntryType::GNULongName
                | tar::EntryType::GNULongLink => continue,
                _ => {
                    self.reject(&member_rel, "link or special file skipped");
                    continue;
                }
            }
            self.stage_member(&member_rel, &mut entry, None, depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn tar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, body) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            // `append_data` validates the path; write the raw name instead so
            // hostile names can be built for the guard tests.
            let raw = h.as_old_mut();
            raw.name[..name.len()].copy_from_slice(name.as_bytes());
            h.set_cksum();
            b.append(&h, *body).unwrap();
        }
        b.into_inner().unwrap()
    }

    struct T {
        _tmp: TempDir,
        src: PathBuf,
        stage: PathBuf,
    }

    fn t() -> T {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let stage = tmp.path().join("stage");
        fs::create_dir_all(&src).unwrap();
        T { _tmp: tmp, src, stage }
    }

    fn rels(s: &Stager) -> Vec<String> {
        s.files.iter().map(|f| f.rel_path.clone()).collect()
    }

    #[test]
    fn folders_keep_relative_paths_and_skip_junk() {
        let t = t();
        let root = t.src.join("装修资料");
        fs::create_dir_all(root.join("合同/补充")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("__MACOSX")).unwrap();
        fs::write(root.join("合同/施工合同.docx"), b"docx").unwrap();
        fs::write(root.join("合同/补充/增项.md"), b"# add").unwrap();
        fs::write(root.join(".DS_Store"), b"x").unwrap();
        fs::write(root.join(".git/config"), b"x").unwrap();
        fs::write(root.join("__MACOSX/._a"), b"x").unwrap();
        fs::write(root.join("~$施工合同.docx"), b"x").unwrap();
        let mut s = Stager::new(&t.stage, Limits::default());
        s.add_path(&root, "装修资料");
        assert_eq!(rels(&s), ["装修资料/合同/施工合同.docx", "装修资料/合同/补充/增项.md"]);
        assert_eq!(s.skipped_junk, 4);
        assert!(s.rejected.is_empty());
        let f = &s.files[0];
        assert_eq!(fs::read(&f.staged).unwrap(), b"docx");
        assert_eq!(f.staged, t.stage.join("0000").join("施工合同.docx"));
        assert_eq!(f.size, 4);
        assert_eq!(f.sha256.len(), 64);
    }

    #[test]
    fn zip_members_are_staged_and_zip_slip_is_refused() {
        let t = t();
        let zip = t.src.join("说明书.zip");
        fs::write(
            &zip,
            zip_bytes(&[
                ("manual/洗碗机.pdf", b"%PDF-1.4"),
                ("../../evil.sh", b"rm -rf"),
                ("/etc/passwd", b"root"),
                ("C:/Windows/x.dll", b"x"),
                ("a\\..\\..\\b.txt", b"x"),
                ("__MACOSX/manual/._洗碗机.pdf", b"x"),
                ("tips.txt", b"clean the filter"),
            ]),
        )
        .unwrap();
        let mut s = Stager::new(&t.stage, Limits::default());
        s.add_path(&zip, "说明书.zip");
        // `/etc/passwd` loses its root and stays inside; `..` and drive
        // letters refuse the member outright.
        assert_eq!(
            rels(&s),
            ["说明书.zip/manual/洗碗机.pdf", "说明书.zip/etc/passwd", "说明书.zip/tips.txt"]
        );
        let refused: Vec<&str> = s.rejected.iter().map(|r| r.reason.as_str()).collect();
        assert_eq!(refused, ["unsafe path inside archive"; 3]);
        assert_eq!(s.skipped_junk, 1);
        // Nothing was written outside the staging dir.
        for f in &s.files {
            assert!(f.staged.starts_with(&t.stage), "{:?}", f.staged);
        }
        assert!(!t.stage.parent().unwrap().join("evil.sh").exists());
    }

    #[test]
    fn tar_and_tgz_expand_with_the_same_guards() {
        let t = t();
        let tar = tar_bytes(&[("docs/a.md", b"# a"), ("../escape.txt", b"x")]);
        fs::write(t.src.join("x.tar"), &tar).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar).unwrap();
        fs::write(t.src.join("y.tgz"), gz.finish().unwrap()).unwrap();
        let mut single = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        single.write_all(b"a,b\n1,2\n").unwrap();
        fs::write(t.src.join("data.csv.gz"), single.finish().unwrap()).unwrap();

        let mut s = Stager::new(&t.stage, Limits::default());
        s.add_path(&t.src.join("x.tar"), "x.tar");
        s.add_path(&t.src.join("y.tgz"), "y.tgz");
        s.add_path(&t.src.join("data.csv.gz"), "data.csv.gz");
        assert_eq!(rels(&s), ["x.tar/docs/a.md", "y.tgz/docs/a.md", "data.csv.gz/data.csv"]);
        assert_eq!(s.rejected.len(), 2);
        assert_eq!(fs::read(&s.files[2].staged).unwrap(), b"a,b\n1,2\n");
        // Same bytes, same hash — what dedupe keys on.
        assert_eq!(s.files[0].sha256, s.files[1].sha256);
    }

    #[test]
    fn nested_archives_expand_until_the_depth_limit() {
        let t = t();
        let inner = zip_bytes(&[("deep.txt", b"deep")]);
        let outer = zip_bytes(&[("inner.zip", &inner), ("top.txt", b"top")]);
        fs::write(t.src.join("outer.zip"), &outer).unwrap();
        let mut s = Stager::new(&t.stage, Limits::default());
        s.add_path(&t.src.join("outer.zip"), "outer.zip");
        assert_eq!(rels(&s), ["outer.zip/inner.zip/deep.txt", "outer.zip/top.txt"]);
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(&t.stage)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());

        let mut shallow = Stager::new(&t.stage.join("b"), Limits { max_archive_depth: 1, ..Limits::default() });
        shallow.add_path(&t.src.join("outer.zip"), "outer.zip");
        assert_eq!(rels(&shallow), ["outer.zip/inner.zip", "outer.zip/top.txt"]);
    }

    #[test]
    fn a_bomb_is_stopped_by_bytes_written_not_by_what_it_declares() {
        let t = t();
        // 40 MiB of zeros deflates to a few dozen KiB: far past the ratio cap.
        let zeros = vec![0u8; 40 * 1024 * 1024];
        fs::write(t.src.join("bomb.zip"), zip_bytes(&[("zeros.bin", &zeros), ("ok.txt", b"fine")])).unwrap();
        let mut s = Stager::new(&t.stage, Limits::default());
        s.add_path(&t.src.join("bomb.zip"), "bomb.zip");
        assert_eq!(rels(&s), ["bomb.zip/ok.txt"]);
        assert_eq!(s.rejected[0].rel_path, "bomb.zip/zeros.bin");
        assert!(s.rejected[0].reason.contains("zip bomb"), "{:?}", s.rejected);
        // The partial file was removed.
        assert!(!t.stage.join("0000").join("zeros.bin").exists());
    }

    #[test]
    fn batch_limits_reject_the_overflow_and_keep_the_rest() {
        let t = t();
        for i in 0..5 {
            fs::write(t.src.join(format!("f{i}.txt")), vec![b'x'; 100]).unwrap();
        }
        let limits = Limits { max_files: 3, max_total_bytes: 1000, ..Limits::default() };
        let mut s = Stager::new(&t.stage, limits);
        s.add_path(&t.src, "src");
        assert_eq!(s.files.len(), 3);
        assert_eq!(s.rejected.len(), 2);
        assert!(s.rejected[0].reason.contains("3-file limit"));

        let limits = Limits { max_total_bytes: 250, ..Limits::default() };
        let mut s = Stager::new(&t.stage.join("b"), limits);
        s.add_path(&t.src, "src");
        assert_eq!(s.files.len(), 2);
        assert!(s.rejected.iter().all(|r| r.reason.contains("size limit")), "{:?}", s.rejected);
    }

    #[test]
    fn unsupported_archives_are_reported_not_dropped() {
        let t = t();
        fs::write(t.src.join("旧户型图.rar"), b"Rar!\x1a\x07\x00junk").unwrap();
        fs::write(t.src.join("backup.tar.xz"), b"x").unwrap();
        fs::write(t.src.join("report.docx"), b"PK\x03\x04").unwrap();
        let mut s = Stager::new(&t.stage, Limits::default());
        for n in ["旧户型图.rar", "backup.tar.xz", "report.docx"] {
            s.add_path(&t.src.join(n), n);
        }
        // An Office file is a zip inside but a document to the user.
        assert_eq!(rels(&s), ["report.docx"]);
        let reasons: Vec<&str> = s.rejected.iter().map(|r| r.reason.as_str()).collect();
        assert!(reasons[0].contains("(rar)") && reasons[1].contains("(tar.xz)"), "{reasons:?}");
    }

    #[test]
    fn gbk_member_names_from_chinese_windows_decode() {
        let (gbk, _, _) = encoding_rs::GBK.encode("发票/收据.pdf");
        assert_eq!(decode_member_name(&gbk), "发票/收据.pdf");
        assert_eq!(decode_member_name("合同.docx".as_bytes()), "合同.docx");
        assert_eq!(safe_member_path("a//./b/").unwrap(), ["a", "b"]);
        assert_eq!(safe_member_path("..").as_deref(), None);
        assert_eq!(safe_member_path("x/\u{0}y").as_deref(), None);
        assert!(is_junk(".DS_Store") && is_junk("._x.pdf") && is_junk("~$合同.docx"));
        assert!(!is_junk("合同.docx"));
    }
}
