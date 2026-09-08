//! Streaming, store-only zip writer.
//!
//! Exists because [`crate::wiki`]'s `zip_dir` cannot be reused for artifacts:
//! it buffers the whole archive in a `Vec<u8>` and writes u32 sizes and
//! offsets, which is fine under the wiki's 100 MB per-version cap and wrong
//! here. A single artifact may be 4 GiB ([`crate::artifacts::MAX_ARTIFACT_BYTES`]),
//! and a folder of them is trivially past `u32::MAX` — the exact case the
//! 产出 page exists for is a folder of 4K renders. So this one:
//!
//! - **streams**: each member is copied from disk to the sink in 64 KiB
//!   chunks, so peak memory is one buffer regardless of archive size;
//! - **speaks zip64** when it has to (a member or an offset past
//!   `u32::MAX`, or more than 65534 members), and stays a plain zip when it
//!   does not, so small archives keep opening in the oldest tools.
//!
//! Store-only (method 0) on purpose, same as the wiki's: deliverables are
//! already-compressed formats — mp4, xlsx, pdf, zip — and deflating them
//! spends CPU to grow the output by a fraction of a percent.
//!
//! ## Why a data descriptor
//!
//! A local file header carries the CRC-32 and sizes *before* the data, but a
//! CRC is only known after reading the bytes. Computing it up front would read
//! every file twice, doubling the I/O on the one operation that is already
//! I/O-bound. Instead each member sets the general-purpose flag bit 3 and
//! writes a data descriptor *after* its bytes — the standard way to stream —
//! while the central directory (written at the end, when everything is known)
//! carries the true values every extractor actually reads.

use std::io::{self, Read, Write};

/// Copy buffer. Large enough that a 500 MB member is ~8000 write calls, small
/// enough to stay off the "one allocation per archive is a problem" list.
const COPY_BUF: usize = 64 * 1024;

/// Values above this need zip64 fields. Overridable in tests so the zip64
/// branches are exercised without writing 4 GiB.
const U32_MAX: u64 = u32::MAX as u64;

/// Signatures, spelled out once.
const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_DESCRIPTOR: u32 = 0x0807_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD64: u32 = 0x0606_4b50;
const SIG_EOCD64_LOCATOR: u32 = 0x0706_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;

/// General-purpose bit 3: sizes and CRC follow the data, not precede it.
const FLAG_DATA_DESCRIPTOR: u16 = 1 << 3;
/// Bit 11: the name is UTF-8. Deliverable names are routinely CJK.
const FLAG_UTF8_NAME: u16 = 1 << 11;

/// What one member ended up being, kept for the central directory.
struct Entry {
    name: String,
    crc: u32,
    size: u64,
    offset: u64,
    /// Whether this entry's local header used zip64 sizes.
    zip64: bool,
}

/// Writes a store-only zip into any sink.
///
/// The sink only has to be `Write` — no `Seek` — which is what lets the same
/// writer serve a file on disk and an HTTP response body.
pub struct ZipStream<W: Write> {
    sink: W,
    /// Bytes written so far; also the offset of the next local header.
    written: u64,
    entries: Vec<Entry>,
    /// Threshold past which zip64 fields are emitted. Only tests lower it.
    zip64_threshold: u64,
}

impl<W: Write> ZipStream<W> {
    pub fn new(sink: W) -> Self {
        Self { sink, written: 0, entries: Vec::new(), zip64_threshold: U32_MAX }
    }

    /// Lower the zip64 threshold so the zip64 paths can be tested without a
    /// 4 GiB fixture. Not for production callers.
    #[cfg(test)]
    fn with_zip64_threshold(mut self, threshold: u64) -> Self {
        self.zip64_threshold = threshold;
        self
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.sink.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Append one member, streaming from `source`.
    ///
    /// `name` is the path inside the archive: forward slashes, no leading
    /// slash, no `..` — the caller owns that, and [`sanitize_member_name`] is
    /// the helper for it. `declared_size` is the size known before reading
    /// (from a `stat`), used only to decide whether this member needs zip64
    /// fields; the size actually written is whatever the reader yields.
    pub fn add<R: Read>(
        &mut self,
        name: &str,
        declared_size: u64,
        source: &mut R,
    ) -> io::Result<()> {
        let zip64 = declared_size > self.zip64_threshold
            || self.written > self.zip64_threshold;
        let name_bytes = name.as_bytes();
        let offset = self.written;

        // ── Local file header ────────────────────────────────────────────
        self.put(&SIG_LOCAL.to_le_bytes())?;
        // 4.5 when zip64 fields are present, 2.0 otherwise.
        self.put(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes())?;
        self.put(&(FLAG_DATA_DESCRIPTOR | FLAG_UTF8_NAME).to_le_bytes())?;
        self.put(&0u16.to_le_bytes())?; // method: stored
        self.put(&0u32.to_le_bytes())?; // dos time+date
        // With bit 3 set these three are zero here and real in the descriptor.
        self.put(&0u32.to_le_bytes())?; // crc
        self.put(&0u32.to_le_bytes())?; // compressed size
        self.put(&0u32.to_le_bytes())?; // uncompressed size
        self.put(&(name_bytes.len() as u16).to_le_bytes())?;
        // A zip64 extra field must be present in the local header for the
        // descriptor's 8-byte sizes to be read as such.
        self.put(&(if zip64 { 20u16 } else { 0u16 }).to_le_bytes())?;
        self.put(name_bytes)?;
        if zip64 {
            self.put(&0x0001u16.to_le_bytes())?; // zip64 extra id
            self.put(&16u16.to_le_bytes())?; // size of the two u64s
            self.put(&0u64.to_le_bytes())?; // uncompressed (in descriptor)
            self.put(&0u64.to_le_bytes())?; // compressed (in descriptor)
        }

        // ── Data ─────────────────────────────────────────────────────────
        let mut hasher = crc32fast::Hasher::new();
        let mut buf = vec![0u8; COPY_BUF];
        let mut size: u64 = 0;
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            self.put(&buf[..n])?;
            size += n as u64;
        }
        let crc = hasher.finalize();

        // ── Data descriptor ──────────────────────────────────────────────
        self.put(&SIG_DESCRIPTOR.to_le_bytes())?;
        self.put(&crc.to_le_bytes())?;
        if zip64 {
            self.put(&size.to_le_bytes())?; // compressed
            self.put(&size.to_le_bytes())?; // uncompressed
        } else {
            self.put(&(size as u32).to_le_bytes())?;
            self.put(&(size as u32).to_le_bytes())?;
        }

        self.entries.push(Entry { name: name.to_string(), crc, size, offset, zip64 });
        Ok(())
    }

    /// Write the central directory and the end-of-archive records.
    pub fn finish(mut self) -> io::Result<W> {
        let cd_offset = self.written;

        for i in 0..self.entries.len() {
            // Cloned out of the loop's borrow so `put` can take &mut self.
            let (name, crc, size, offset, entry_zip64) = {
                let e = &self.entries[i];
                (e.name.clone(), e.crc, e.size, e.offset, e.zip64)
            };
            // A central entry needs zip64 if *its* numbers do not fit, which
            // can be true even when the local header did not need it: a small
            // member sitting past the 4 GiB mark.
            let big_size = size > self.zip64_threshold;
            let big_offset = offset > self.zip64_threshold;
            let zip64 = entry_zip64 || big_size || big_offset;
            let extra_len: u16 = if zip64 {
                // Only the fields that actually overflow are present, in a
                // fixed order: uncompressed, compressed, offset.
                4 + (if big_size { 16 } else { 0 }) + (if big_offset { 8 } else { 0 })
            } else {
                0
            };
            let name_bytes = name.clone().into_bytes();

            self.put(&SIG_CENTRAL.to_le_bytes())?;
            self.put(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes())?; // made by
            self.put(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes())?; // needed
            self.put(&(FLAG_DATA_DESCRIPTOR | FLAG_UTF8_NAME).to_le_bytes())?;
            self.put(&0u16.to_le_bytes())?; // method
            self.put(&0u32.to_le_bytes())?; // dos time+date
            self.put(&crc.to_le_bytes())?;
            let stored_size = if big_size { u32::MAX } else { size as u32 };
            self.put(&stored_size.to_le_bytes())?; // compressed
            self.put(&stored_size.to_le_bytes())?; // uncompressed
            self.put(&(name_bytes.len() as u16).to_le_bytes())?;
            self.put(&extra_len.to_le_bytes())?;
            self.put(&0u16.to_le_bytes())?; // comment len
            self.put(&0u16.to_le_bytes())?; // disk number
            self.put(&0u16.to_le_bytes())?; // internal attrs
            self.put(&0u32.to_le_bytes())?; // external attrs
            self.put(&(if big_offset { u32::MAX } else { offset as u32 }).to_le_bytes())?;
            self.put(&name_bytes)?;
            if zip64 && extra_len > 4 {
                self.put(&0x0001u16.to_le_bytes())?;
                self.put(&(extra_len - 4).to_le_bytes())?;
                if big_size {
                    self.put(&size.to_le_bytes())?; // uncompressed
                    self.put(&size.to_le_bytes())?; // compressed
                }
                if big_offset {
                    self.put(&offset.to_le_bytes())?;
                }
            }
        }

        let cd_size = self.written - cd_offset;
        let count = self.entries.len() as u64;
        // The classic EOCD's fields are 16- and 32-bit; anything past that has
        // to go through the zip64 records, with sentinels left behind so an
        // old reader still finds a coherent (if truncated) directory.
        let need_eocd64 = count > u16::MAX as u64 - 1
            || cd_size > self.zip64_threshold
            || cd_offset > self.zip64_threshold;

        if need_eocd64 {
            let eocd64_offset = self.written;
            self.put(&SIG_EOCD64.to_le_bytes())?;
            self.put(&44u64.to_le_bytes())?; // size of this record minus 12
            self.put(&45u16.to_le_bytes())?; // made by
            self.put(&45u16.to_le_bytes())?; // needed
            self.put(&0u32.to_le_bytes())?; // this disk
            self.put(&0u32.to_le_bytes())?; // cd start disk
            self.put(&count.to_le_bytes())?; // entries on this disk
            self.put(&count.to_le_bytes())?; // entries total
            self.put(&cd_size.to_le_bytes())?;
            self.put(&cd_offset.to_le_bytes())?;

            self.put(&SIG_EOCD64_LOCATOR.to_le_bytes())?;
            self.put(&0u32.to_le_bytes())?; // disk with the eocd64
            self.put(&eocd64_offset.to_le_bytes())?;
            self.put(&1u32.to_le_bytes())?; // total disks
        }

        self.put(&SIG_EOCD.to_le_bytes())?;
        self.put(&0u16.to_le_bytes())?; // this disk
        self.put(&0u16.to_le_bytes())?; // cd start disk
        let stored_count = if count > u16::MAX as u64 - 1 { u16::MAX } else { count as u16 };
        self.put(&stored_count.to_le_bytes())?;
        self.put(&stored_count.to_le_bytes())?;
        self.put(&(if cd_size > self.zip64_threshold { u32::MAX } else { cd_size as u32 })
            .to_le_bytes())?;
        self.put(&(if cd_offset > self.zip64_threshold { u32::MAX } else { cd_offset as u32 })
            .to_le_bytes())?;
        self.put(&0u16.to_le_bytes())?; // comment len

        self.sink.flush()?;
        Ok(self.sink)
    }
}

/// Make `name` safe to use as a path inside an archive.
///
/// Zip member names are attacker-controlled input to whatever extracts them
/// ("zip slip"), and these names come from artifact filenames and user-typed
/// folders. Backslashes become separators, `.`/`..` segments and empties are
/// dropped, and control characters go. An entirely unusable name collapses to
/// `file`, because a member with an empty name is not extractable at all.
pub fn sanitize_member_name(name: &str) -> String {
    let cleaned: Vec<String> = name
        .replace('\\', "/")
        .split('/')
        .map(|seg| {
            seg.trim()
                .chars()
                .filter(|c| !c.is_control() && *c != ':')
                .collect::<String>()
        })
        .filter(|seg| !seg.is_empty() && seg != "." && seg != "..")
        .collect();
    if cleaned.is_empty() {
        return "file".to_string();
    }
    cleaned.join("/")
}

/// De-duplicate a member name against the ones already used, the way a
/// browser's download folder does: `report.pdf`, `report (2).pdf`.
///
/// Two artifacts filed in one folder can share a filename — `report.pdf` from
/// two sessions is the ordinary case — and a zip with two identical member
/// names extracts to one file, silently losing the other.
pub fn unique_member_name(name: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let (dir, file) = match name.rfind('/') {
        Some(i) => (&name[..=i], &name[i + 1..]),
        None => ("", name),
    };
    // A leading dot is the whole name, not an extension: `.env` must not
    // become ` (2).env`.
    let dot = file[1..].rfind('.').map(|i| i + 1);
    let (stem, ext) = match dot {
        Some(i) => (&file[..i], &file[i..]),
        None => (file, ""),
    };
    for n in 2.. {
        let candidate = format!("{dir}{stem} ({n}){ext}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the loop returns on the first unused candidate")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::Cursor;
    use std::process::Command;
    use tempfile::TempDir;

    fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut z = ZipStream::new(Vec::new());
        for (name, body) in members {
            z.add(name, body.len() as u64, &mut Cursor::new(*body)).unwrap();
        }
        z.finish().unwrap()
    }

    /// Read the archive back with a real extractor and return its members.
    ///
    /// Python's `zipfile` rather than the `unzip` binary, and the reason is a
    /// finding worth keeping: macOS ships Info-ZIP UnZip 6.00, which ignores
    /// general-purpose bit 11 and interprets member names in the local code
    /// page — a CJK name comes out as `????.md` and the extraction then fails
    /// on the mangled path. That is the extractor's age, not a defect in the
    /// archive, and pinning our correctness to it would mean giving up UTF-8
    /// names. `zipfile` honours the flag, verifies every CRC through
    /// `testzip()`, and is present wherever these tests run.
    ///
    /// [`unzip_accepts_structure`] covers the "an old tool can still read it"
    /// half separately, on ASCII-named archives.
    fn extract(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, bytes).unwrap();
        let script = r#"
import json, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    bad = z.testzip()
    if bad is not None:
        print("CRCFAIL:" + bad, file=sys.stderr)
        sys.exit(2)
    out = [[i.filename, z.read(i.filename).decode("latin-1")] for i in z.infolist()]
print(json.dumps(out))
"#;
        let run = Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(&archive)
            .output()
            .expect("python3 must be available to verify the archive");
        assert!(
            run.status.success(),
            "python zipfile rejected the archive: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let raw = String::from_utf8(run.stdout).unwrap();
        let parsed: Vec<Vec<String>> = serde_json::from_str(raw.trim()).unwrap();
        let mut got: Vec<(String, Vec<u8>)> = parsed
            .into_iter()
            .map(|pair| {
                // latin-1 on the python side is a byte-preserving transport,
                // so this recovers the exact bytes.
                (pair[0].clone(), pair[1].chars().map(|c| c as u8).collect())
            })
            .collect();
        got.sort();
        got
    }

    /// Assert the *old* extractor also accepts the archive's structure.
    ///
    /// Only meaningful for ASCII-named archives (see [`extract`]), and
    /// skipped where the binary is missing. An empty archive is excluded by
    /// the caller: `unzip -t` reports "zipfile is empty" and exits non-zero
    /// for one, which is its own convention rather than a verdict on the
    /// bytes — an EOCD-only archive is valid and every modern reader opens it.
    fn unzip_accepts_structure(bytes: &[u8]) {
        if Command::new("unzip").arg("-v").output().is_err() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, bytes).unwrap();
        let test = Command::new("unzip").arg("-t").arg(&archive).output().unwrap();
        assert!(
            test.status.success(),
            "unzip -t rejected the archive: {}{}",
            String::from_utf8_lossy(&test.stdout),
            String::from_utf8_lossy(&test.stderr)
        );
    }

    #[test]
    fn a_plain_archive_round_trips_with_cjk_names_and_nesting() {
        let bytes = zip_of(&[
            ("report.pdf", b"%PDF-1.4 body"),
            ("nested/交付说明.md", "# 说明\n给客户的。\n".as_bytes()),
            ("empty.txt", b""),
        ]);
        let got = extract(&bytes);
        assert_eq!(
            got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["empty.txt", "nested/交付说明.md", "report.pdf"],
            "CJK names and nesting must survive, and an empty member must exist"
        );
        assert_eq!(got[2].1, b"%PDF-1.4 body");
        assert_eq!(got[1].1, "# 说明\n给客户的。\n".as_bytes());
        assert!(got[0].1.is_empty());
    }

    /// The same archive shape, ASCII-named, must also satisfy the 16-year-old
    /// extractor that ships with macOS — that is what a recipient may have.
    #[test]
    fn an_ascii_named_archive_satisfies_the_system_unzip() {
        unzip_accepts_structure(&zip_of(&[
            ("report.pdf", b"%PDF-1.4 body"),
            ("nested/notes.md", b"# notes"),
            ("empty.txt", b""),
        ]));
    }

    /// The whole reason this module exists rather than reusing wiki's writer:
    /// offsets and sizes past `u32::MAX`. The threshold is lowered so the
    /// zip64 branches run against a few hundred bytes.
    #[test]
    fn the_zip64_branches_produce_an_archive_unzip_accepts() {
        let big = vec![b'x'; 300];
        let mut z = ZipStream::new(Vec::new()).with_zip64_threshold(64);
        z.add("big-one.bin", big.len() as u64, &mut Cursor::new(&big[..])).unwrap();
        // Small, but its *offset* is now past the (lowered) threshold — the
        // case where a central entry needs zip64 though its local header did
        // not.
        z.add("after.txt", 5, &mut Cursor::new(&b"hello"[..])).unwrap();
        let bytes = z.finish().unwrap();

        // The zip64 end-of-central-directory record must be there.
        assert!(
            bytes.windows(4).any(|w| w == SIG_EOCD64.to_le_bytes()),
            "expected a zip64 EOCD record"
        );
        let got = extract(&bytes);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("after.txt".to_string(), b"hello".to_vec()));
        assert_eq!(got[1].1.len(), 300);
        unzip_accepts_structure(&bytes);
    }

    #[test]
    fn an_empty_archive_is_still_a_valid_archive() {
        let bytes = zip_of(&[]);
        // 22 bytes: an EOCD and nothing else.
        assert_eq!(bytes.len(), 22);
        assert!(extract(&bytes).is_empty());
    }

    #[test]
    fn the_declared_size_only_picks_the_format_never_the_content() {
        // A stat that disagrees with the bytes (the file changed under us)
        // must still produce a correct archive: the descriptor and central
        // directory carry what was actually read.
        let mut z = ZipStream::new(Vec::new());
        z.add("a.txt", 9_999, &mut Cursor::new(&b"four"[..])).unwrap();
        let bytes = z.finish().unwrap();
        assert_eq!(extract(&bytes), vec![("a.txt".to_string(), b"four".to_vec())]);
    }

    #[test]
    fn member_names_cannot_escape_the_extraction_directory() {
        assert_eq!(sanitize_member_name("../../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_member_name("/abs/path.pdf"), "abs/path.pdf");
        assert_eq!(sanitize_member_name("a/./b/../c.txt"), "a/b/c.txt");
        assert_eq!(sanitize_member_name("win\\style\\name.txt"), "win/style/name.txt");
        assert_eq!(sanitize_member_name("交付/报告.pdf"), "交付/报告.pdf");
        // Nothing usable left.
        assert_eq!(sanitize_member_name("../.."), "file");
        assert_eq!(sanitize_member_name(""), "file");
        assert_eq!(sanitize_member_name("   "), "file");
    }

    #[test]
    fn duplicate_member_names_are_suffixed_not_overwritten() {
        let mut used = HashSet::new();
        assert_eq!(unique_member_name("report.pdf", &mut used), "report.pdf");
        assert_eq!(unique_member_name("report.pdf", &mut used), "report (2).pdf");
        assert_eq!(unique_member_name("report.pdf", &mut used), "report (3).pdf");
        // Per directory, so the same name in two folders is untouched.
        assert_eq!(unique_member_name("sub/report.pdf", &mut used), "sub/report.pdf");
        assert_eq!(unique_member_name("sub/report.pdf", &mut used), "sub/report (2).pdf");
        // A dotfile's leading dot is not an extension.
        assert_eq!(unique_member_name(".env", &mut used), ".env");
        assert_eq!(unique_member_name(".env", &mut used), ".env (2)");
        // No extension at all.
        assert_eq!(unique_member_name("README", &mut used), "README");
        assert_eq!(unique_member_name("README", &mut used), "README (2)");
    }

    #[test]
    fn two_members_with_the_same_name_would_otherwise_collapse() {
        // Guards the reason `unique_member_name` exists: what a zip with two
        // identical names actually does on extraction.
        let mut used = HashSet::new();
        let a = unique_member_name(&sanitize_member_name("r.txt"), &mut used);
        let b = unique_member_name(&sanitize_member_name("r.txt"), &mut used);
        let bytes = zip_of(&[(&a, b"first"), (&b, b"second")]);
        let got = extract(&bytes);
        assert_eq!(got.len(), 2, "both must land");
        assert_eq!(got[0], ("r (2).txt".to_string(), b"second".to_vec()));
        assert_eq!(got[1], ("r.txt".to_string(), b"first".to_vec()));
    }

    #[test]
    fn a_streamed_member_matches_a_buffered_one_byte_for_byte() {
        // Several copy-buffer crossings, so the chunk loop's CRC and size
        // accounting is exercised rather than assumed.
        let body: Vec<u8> = (0..COPY_BUF * 2 + 7).map(|i| (i % 251) as u8).collect();
        let bytes = zip_of(&[("big.bin", &body)]);
        let got = extract(&bytes);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, body, "content must survive a multi-chunk copy");
    }
}
