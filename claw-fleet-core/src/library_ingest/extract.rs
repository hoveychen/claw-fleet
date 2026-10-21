//! Text extraction for library uploads.
//!
//! Turns an uploaded file into plain text, or flags it as something that has to
//! be looked at (images, scanned PDFs). Every reader is bounded: text files are
//! read up to [`TEXT_READ_CAP`], zip parts are decompressed through a
//! `Read::take` cap, structured formats stop once [`RAW_TEXT_BUDGET`] bytes of
//! text are collected, and PDFs are parsed on a worker thread under
//! `catch_unwind` and a timeout.

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use zip::result::ZipError;
use zip::ZipArchive;

/// Most chars of text kept per file (the rest is dropped, `truncated = true`).
pub const MAX_TEXT_CHARS: usize = 200_000;

/// Bytes of a text/html file read; the rest is ignored.
const TEXT_READ_CAP: u64 = 4 << 20;
/// Bytes inspected when the extension does not decide the format.
const SNIFF_BYTES: u64 = 8 << 10;
/// Decompressed bytes read from any single zip part (zip-bomb guard).
const PART_CAP: u64 = 64 << 20;
/// Raw text collected from structured formats before parsing stops. Generous
/// over `MAX_TEXT_CHARS` because normalization drops layout whitespace.
const RAW_TEXT_BUDGET: usize = MAX_TEXT_CHARS * 8;
/// Rows kept per spreadsheet sheet.
const MAX_SHEET_ROWS: usize = 2_000;
/// Highest column honoured when padding spreadsheet gaps (Excel's own limit).
const MAX_SHEET_COLS: usize = 16_384;
/// PDFs larger than this are not parsed.
const PDF_MAX_BYTES: u64 = 150 << 20;
/// Wall-clock limit for one PDF parse.
const PDF_TIMEOUT: Duration = Duration::from_secs(120);
/// Stack for the PDF worker thread; pdf-extract recurses into form XObjects.
const PDF_STACK_BYTES: usize = 64 << 20;
/// Fewer non-whitespace chars than this means the PDF has no usable text layer.
const PDF_MIN_TEXT_CHARS: usize = 40;
/// Deepest XObject-in-XObject nesting handed to pdf-extract (one stack frame
/// per level there; real documents stay in single digits).
const PDF_MAX_FORM_DEPTH: usize = 32;
/// Most XObject invocations one top-level `Do` may expand to.
const PDF_MAX_FORM_CALLS: u64 = 100_000;
/// Longest `/Parent` chain from a page to the page-tree root.
const PDF_MAX_PARENT_HOPS: usize = 64;
/// Notes longer than this are cut (parser errors can quote input).
const MAX_NOTE_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// Plain text, whitespace-normalized (runs of blank lines collapsed to one; trailing spaces trimmed). Empty when nothing usable.
    pub text: String,
    /// How it was obtained: "text" | "html" | "docx" | "xlsx" | "pptx" | "odf" | "pdf" | "none".
    pub method: &'static str,
    /// True when the content can only be understood by looking at it: an image, or a PDF whose text layer is missing/near-empty (scanned).
    pub needs_vision: bool,
    pub truncated: bool,
    /// Short English reason when extraction failed or was skipped (e.g. "encrypted pdf", "legacy .doc not supported", "pdf parser panicked"), else None.
    pub note: Option<String>,
}

impl Extracted {
    fn empty(method: &'static str, note: Option<String>) -> Self {
        Extracted {
            text: String::new(),
            method,
            needs_vision: false,
            truncated: false,
            note: note.map(|n| short(&n)),
        }
    }

    fn skipped(note: impl Into<String>) -> Self {
        Self::empty("none", Some(note.into()))
    }
}

/// Extract text from the file at `path`. `name` is the original file name (extension decides the format; fall back to content sniffing). Never panics, never blocks unboundedly on hostile input.
pub fn extract(path: &Path, name: &str) -> Extracted {
    // Last-resort net: nothing below should panic, but the input is hostile.
    // pdf-extract panics are caught closer to the call (on its own thread).
    match panic::catch_unwind(AssertUnwindSafe(|| dispatch(path, name))) {
        Ok(x) => x,
        Err(_) => Extracted::skipped("extractor panicked"),
    }
}

/// The media type Claude can read for an image file name (png/jpg/jpeg/gif/webp), else None.
pub fn vision_image_type(name: &str) -> Option<&'static str> {
    match ext_of(name).as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// True for image formats a phone produces that Claude cannot read directly but macOS `sips` can convert (heic, heif, tif, tiff, bmp).
pub fn needs_image_conversion(name: &str) -> bool {
    matches!(
        ext_of(name).as_str(),
        "heic" | "heif" | "tif" | "tiff" | "bmp"
    )
}

// ---------------------------------------------------------------------------
// Format selection

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OdfKind {
    Text,
    Sheet,
    Slides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skip {
    Legacy,
    Audio,
    Video,
    Archive,
    /// Known non-text extension.
    Other,
    /// Unknown extension whose bytes look binary.
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Html,
    Docx,
    Xlsx,
    Pptx,
    Odf(OdfKind),
    Pdf,
    Image,
    Unsupported(Skip),
}

#[rustfmt::skip]
const TEXT_EXTS: &[&str] = &[
    "txt", "text", "md", "markdown", "mdx", "rst", "adoc", "org", "tex", "bib",
    "csv", "tsv", "json", "jsonl", "ndjson", "geojson", "yaml", "yml", "toml", "xml", "svg",
    "log", "ini", "cfg", "conf", "properties", "env", "srt", "vtt", "diff", "patch",
    "rs", "py", "pyi", "js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx", "vue", "svelte",
    "go", "java", "kt", "kts", "scala", "groovy", "gradle", "c", "h", "cc", "cpp", "cxx",
    "hpp", "hh", "m", "mm", "cs", "rb", "php", "pl", "pm", "lua", "r", "dart", "swift",
    "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "sql", "graphql", "gql", "proto",
    "css", "scss", "sass", "less", "mk", "cmake", "dockerfile",
];
const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "m4a", "aac", "flac", "ogg", "oga", "opus", "amr", "wma", "aiff", "aif", "caf",
    "mid", "midi",
];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "avi", "mkv", "webm", "wmv", "flv", "3gp", "mpg", "mpeg",
];
const ARCHIVE_EXTS: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "xz", "txz", "zst", "lz4", "lzma", "cab",
    "iso", "dmg", "jar", "apk", "ipa",
];
const OTHER_BINARY_EXTS: &[&str] = &[
    "rtf", "exe", "dll", "so", "dylib", "bin", "o", "a", "lib", "obj", "class", "pyc", "wasm",
    "sqlite", "sqlite3", "db", "mdb", "psd", "ai", "sketch", "fig", "xd", "indd", "ttf", "otf",
    "woff", "woff2", "eot", "ico", "icns", "avif", "xlsb", "pages", "numbers", "epub", "mobi",
    "azw3",
];

fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn format_for_ext(ext: &str) -> Option<Format> {
    let f = match ext {
        "" => return None,
        "html" | "htm" | "xhtml" => Format::Html,
        "docx" | "docm" | "dotx" | "dotm" => Format::Docx,
        "xlsx" | "xlsm" | "xltx" | "xltm" => Format::Xlsx,
        "pptx" | "pptm" | "potx" | "ppsx" => Format::Pptx,
        "odt" | "ott" => Format::Odf(OdfKind::Text),
        "ods" | "ots" => Format::Odf(OdfKind::Sheet),
        "odp" | "otp" => Format::Odf(OdfKind::Slides),
        "pdf" => Format::Pdf,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp" => {
            Format::Image
        }
        "doc" | "dot" | "xls" | "xlt" | "ppt" | "pot" | "pps" => Format::Unsupported(Skip::Legacy),
        e if TEXT_EXTS.contains(&e) => Format::Text,
        e if AUDIO_EXTS.contains(&e) => Format::Unsupported(Skip::Audio),
        e if VIDEO_EXTS.contains(&e) => Format::Unsupported(Skip::Video),
        e if ARCHIVE_EXTS.contains(&e) => Format::Unsupported(Skip::Archive),
        e if OTHER_BINARY_EXTS.contains(&e) => Format::Unsupported(Skip::Other),
        _ => return None,
    };
    Some(f)
}

fn skip_note(skip: Skip, ext: &str) -> String {
    match skip {
        Skip::Legacy if ext.is_empty() => "legacy office binary not supported".into(),
        Skip::Legacy => format!("legacy .{ext} not supported"),
        Skip::Audio => "audio not supported".into(),
        Skip::Video => "video not supported".into(),
        Skip::Archive => "archive not supported".into(),
        Skip::Other if !ext.is_empty() => format!("unsupported file type .{ext}"),
        Skip::Other | Skip::Binary => "binary file not supported".into(),
    }
}

fn dispatch(path: &Path, name: &str) -> Extracted {
    match fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        // A FIFO or device would block or never end.
        Ok(_) => return Extracted::skipped("not a regular file"),
        Err(e) => return Extracted::skipped(format!("cannot read file: {e}")),
    }
    let ext = ext_of(name);
    let format = match format_for_ext(&ext) {
        Some(f) => f,
        None => match sniff_path(path) {
            Ok(f) => f,
            Err(note) => return Extracted::skipped(note),
        },
    };
    match format {
        Format::Text => text_file(path, false),
        Format::Html => text_file(path, true),
        Format::Docx => docx(path),
        Format::Xlsx => xlsx(path),
        Format::Pptx => pptx(path),
        Format::Odf(kind) => odf(path, kind),
        Format::Pdf => pdf(path),
        Format::Image => Extracted {
            needs_vision: true,
            ..Extracted::empty("none", None)
        },
        Format::Unsupported(skip) => Extracted::skipped(skip_note(skip, &ext)),
    }
}

/// Decide the format from content when the extension did not.
fn sniff_path(path: &Path) -> Result<Format, String> {
    let (head, _) = read_capped(path, SNIFF_BYTES)?;
    let f = sniff_bytes(&head);
    if f != Format::Unsupported(Skip::Archive) || !head.starts_with(b"PK\x03\x04") {
        return Ok(f);
    }
    Ok(sniff_zip(path).unwrap_or(f))
}

fn sniff_bytes(h: &[u8]) -> Format {
    let at = |i: usize, m: &[u8]| h.len() >= i + m.len() && &h[i..i + m.len()] == m;
    if at(0, b"%PDF-") {
        return Format::Pdf;
    }
    if at(0, b"\x89PNG\r\n\x1a\n")
        || at(0, b"\xFF\xD8\xFF")
        || at(0, b"GIF8")
        || (at(0, b"RIFF") && at(8, b"WEBP"))
        || at(0, b"II*\0")
        || at(0, b"MM\0*")
    {
        return Format::Image;
    }
    if at(4, b"ftyp") {
        return match h.get(8..12) {
            Some(b"heic" | b"heix" | b"mif1" | b"msf1" | b"heis") => Format::Image,
            Some(b"M4A " | b"M4B ") => Format::Unsupported(Skip::Audio),
            _ => Format::Unsupported(Skip::Video),
        };
    }
    if at(0, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return Format::Unsupported(Skip::Legacy);
    }
    if at(0, b"PK\x03\x04")
        || at(0, b"\x1F\x8B")
        || at(0, b"7z\xBC\xAF\x27\x1C")
        || at(0, b"Rar!\x1A\x07")
        || at(0, b"\xFD7zXZ\0")
        || at(0, b"BZh")
        || at(0, b"\x28\xB5\x2F\xFD")
    {
        return Format::Unsupported(Skip::Archive);
    }
    if at(0, b"ID3") || at(0, b"fLaC") || at(0, b"OggS") || (at(0, b"RIFF") && at(8, b"WAVE")) {
        return Format::Unsupported(Skip::Audio);
    }
    if at(0, b"\x1A\x45\xDF\xA3") {
        return Format::Unsupported(Skip::Video);
    }
    if at(0, b"{\\rtf") {
        return Format::Unsupported(Skip::Other);
    }
    let bom = at(0, b"\xEF\xBB\xBF") || at(0, b"\xFF\xFE") || at(0, b"\xFE\xFF");
    if !bom && h.contains(&0) {
        return Format::Unsupported(Skip::Binary);
    }
    let start = String::from_utf8_lossy(&h[..h.len().min(1024)]).to_ascii_lowercase();
    if start.trim_start().starts_with("<!doctype html") || start.contains("<html") {
        Format::Html
    } else {
        Format::Text
    }
}

/// Tell an office package from a plain zip by its entries.
fn sniff_zip(path: &Path) -> Option<Format> {
    let mut zip = ZipArchive::new(File::open(path).ok()?).ok()?;
    let has = |zip: &ZipArchive<File>, n: &str| zip.index_for_name(n).is_some();
    if has(&zip, "word/document.xml") {
        return Some(Format::Docx);
    }
    if has(&zip, "xl/workbook.xml") {
        return Some(Format::Xlsx);
    }
    if has(&zip, "ppt/presentation.xml") {
        return Some(Format::Pptx);
    }
    if has(&zip, "content.xml") {
        let mut mime = String::new();
        if let Ok(f) = zip.by_name("mimetype") {
            let _ = f.take(256).read_to_string(&mut mime);
        }
        let kind = if mime.contains("spreadsheet") {
            OdfKind::Sheet
        } else if mime.contains("presentation") {
            OdfKind::Slides
        } else {
            OdfKind::Text
        };
        return Some(Format::Odf(kind));
    }
    None
}

// ---------------------------------------------------------------------------
// Shared helpers

/// Read at most `cap` bytes; the flag says whether the file had more.
fn read_capped(path: &Path, cap: u64) -> Result<(Vec<u8>, bool), String> {
    let f = File::open(path).map_err(|e| format!("cannot read file: {e}"))?;
    let mut buf = Vec::new();
    f.take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot read file: {e}"))?;
    let over = buf.len() as u64 > cap;
    if over {
        buf.truncate(cap as usize);
    }
    Ok((buf, over))
}

fn short(s: &str) -> String {
    match s.char_indices().nth(MAX_NOTE_CHARS) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

#[derive(Default)]
struct Log {
    notes: Vec<String>,
    truncated: bool,
}

impl Log {
    fn note(&mut self, s: impl Into<String>) {
        if self.notes.len() < 6 {
            self.notes.push(short(&s.into()));
        }
    }
}

/// Raw text accumulator with a hard size budget.
struct Out {
    s: String,
    full: bool,
}

impl Out {
    fn new() -> Self {
        Out {
            s: String::new(),
            full: false,
        }
    }

    fn push_str(&mut self, t: &str) {
        if self.full {
            return;
        }
        let room = RAW_TEXT_BUDGET.saturating_sub(self.s.len());
        if t.len() <= room {
            self.s.push_str(t);
        } else {
            let mut cut = room;
            while !t.is_char_boundary(cut) {
                cut -= 1;
            }
            self.s.push_str(&t[..cut]);
            self.full = true;
        }
    }

    fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0u8; 4]));
    }

    /// Drop trailing spaces and tabs on the current line.
    fn trim_inline(&mut self) {
        let n = self.s.trim_end_matches([' ', '\t']).len();
        self.s.truncate(n);
    }

    /// A single space, unless at the start or after whitespace.
    fn soft_space(&mut self) {
        if !self.s.is_empty() && !self.s.ends_with(char::is_whitespace) {
            self.push(' ');
        }
    }

    /// Text with whitespace runs collapsed to one space (HTML/ODF semantics).
    fn push_collapsed(&mut self, t: &str) {
        for c in t.chars() {
            if c.is_whitespace() {
                self.soft_space();
            } else {
                self.push(c);
            }
        }
    }

    fn ensure_newline(&mut self) {
        if !self.s.is_empty() && !self.s.ends_with('\n') {
            self.push('\n');
        }
    }

    fn ensure_blank_line(&mut self) {
        self.trim_inline();
        if self.s.is_empty() {
            return;
        }
        while !self.s.ends_with("\n\n") && !self.full {
            self.push('\n');
        }
    }
}

/// Final whitespace normalization: unify line ends, drop control chars, trim
/// line ends, collapse runs of blank lines to one, trim the ends.
fn normalize(raw: &str) -> String {
    let mut clean = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    clean.push('\n');
                }
            }
            '\n' | '\t' => clean.push(c),
            '\u{0B}' | '\u{0C}' | '\u{2028}' | '\u{2029}' => clean.push('\n'),
            '\u{FEFF}' => {}
            c if c.is_control() => {}
            c => clean.push(c),
        }
    }
    let mut out = String::with_capacity(clean.len());
    let mut blank = false;
    for line in clean.split('\n') {
        let line = line.trim_end();
        if line.trim_start().is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank {
                out.push('\n');
            }
        }
        blank = false;
        out.push_str(line);
    }
    out
}

fn truncate_chars(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    match s.char_indices().nth(max) {
        Some((i, _)) => {
            s.truncate(i);
            let n = s.trim_end().len();
            s.truncate(n);
            true
        }
        None => false,
    }
}

fn finish(method: &'static str, raw: &str, truncated: bool, notes: Vec<String>) -> Extracted {
    let mut text = normalize(raw);
    let cut = truncate_chars(&mut text, MAX_TEXT_CHARS);
    Extracted {
        text,
        method,
        needs_vision: false,
        truncated: truncated || cut,
        note: if notes.is_empty() {
            None
        } else {
            Some(short(&notes.join("; ")))
        },
    }
}

/// Resolve `&name;` / `&#N;` / `&#xH;`. Covers XML's five plus common HTML ones.
fn resolve_entity(ent: &str) -> Option<char> {
    if let Some(num) = ent.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code).filter(|&c| c != '\0');
    }
    Some(match ent {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "bull" => '•',
        "times" => '×',
        "divide" => '÷',
        "deg" => '°',
        "yen" => '¥',
        "euro" => '€',
        "pound" => '£',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        _ => return None,
    })
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let semi = after.as_bytes().iter().take(32).position(|&b| b == b';');
        if let Some(c) = semi.and_then(|i| resolve_entity(&after[..i])) {
            out.push(c);
            rest = &after[semi.unwrap_or(0) + 1..];
        } else {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Plain text and HTML

fn text_file(path: &Path, html: bool) -> Extracted {
    let method = if html { "html" } else { "text" };
    let (bytes, capped) = match read_capped(path, TEXT_READ_CAP) {
        Ok(r) => r,
        Err(note) => return Extracted::empty(method, Some(note)),
    };
    let text = decode_text(&bytes, capped);
    if html {
        let out = html_to_text(&text);
        finish(method, &out.s, capped || out.full, Vec::new())
    } else {
        finish(method, &text, capped, Vec::new())
    }
}

/// BOM-led UTF-8/UTF-16, else UTF-8, else GB18030 if it decodes cleanly, else
/// lossy UTF-8. `capped` means the bytes may end mid-character.
fn decode_text(bytes: &[u8], capped: bool) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    for (bom, enc) in [
        (&b"\xFF\xFE"[..], encoding_rs::UTF_16LE),
        (&b"\xFE\xFF"[..], encoding_rs::UTF_16BE),
    ] {
        if let Some(rest) = bytes.strip_prefix(bom) {
            let rest = if capped {
                &rest[..rest.len() & !1]
            } else {
                rest
            };
            return enc.decode_without_bom_handling(rest).0.into_owned();
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => return s.to_owned(),
        Err(e) if capped && e.error_len().is_none() => {
            return String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned();
        }
        Err(_) => {}
    }
    let tails = if capped { 0..4 } else { 0..1 };
    for cut in tails {
        let body = &bytes[..bytes.len().saturating_sub(cut)];
        let (s, had_errors) = encoding_rs::GB18030.decode_without_bom_handling(body);
        if !had_errors {
            return s.into_owned();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|p| p + from)
}

/// Index just past the `>` closing a tag, skipping quoted attribute values.
fn tag_end(b: &[u8], mut i: usize) -> usize {
    let mut quote = None;
    while i < b.len() {
        match (quote, b[i]) {
            (None, b'"' | b'\'') => quote = Some(b[i]),
            (Some(q), c) if c == q => quote = None,
            (None, b'>') => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

fn html_to_text(src: &str) -> Out {
    let b = src.as_bytes();
    let mut out = Out::new();
    let mut pre = 0u32;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            let end = b[i..]
                .iter()
                .position(|&c| c == b'<')
                .map_or(b.len(), |p| p + i);
            let text = decode_entities(&src[i..end]);
            if pre > 0 {
                out.push_str(&text);
            } else {
                out.push_collapsed(&text);
            }
            i = end;
            continue;
        }
        if b[i..].starts_with(b"<!--") {
            i = find_from(b, b"-->", i + 4).map_or(b.len(), |p| p + 3);
            continue;
        }
        if b[i..].starts_with(b"<!") || b[i..].starts_with(b"<?") {
            i = tag_end(b, i + 2);
            continue;
        }
        let closing = b.get(i + 1) == Some(&b'/');
        let name_start = i + 1 + usize::from(closing);
        let name_len = b[name_start..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if name_len == 0 {
            // A bare '<' in text.
            out.push('<');
            i += 1;
            continue;
        }
        let name = src[name_start..name_start + name_len].to_ascii_lowercase();
        let end = tag_end(b, name_start + name_len);
        let self_closing = end >= 2 && b[end - 2] == b'/';
        i = end;
        if !closing
            && !self_closing
            && matches!(
                name.as_str(),
                "script" | "style" | "head" | "template" | "noscript" | "svg"
            )
        {
            let close = format!("</{name}");
            match find_from(b, close.as_bytes(), i) {
                Some(p) => i = tag_end(b, p + close.len()),
                // `</head>` is optional in HTML; script/style run to the end.
                None if name == "head" => {}
                None => i = b.len(),
            }
            continue;
        }
        match name.as_str() {
            "br" => {
                out.trim_inline();
                out.push('\n');
            }
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table" | "ul" | "ol" | "dl"
            | "blockquote" | "hr" | "section" | "article" | "form" => out.ensure_blank_line(),
            "pre" => {
                out.ensure_blank_line();
                pre = if closing {
                    pre.saturating_sub(1)
                } else {
                    pre + 1
                };
            }
            "div" | "li" | "tr" | "dt" | "dd" | "header" | "footer" | "nav" | "main" | "aside"
            | "figure" | "figcaption" | "address" | "caption" | "title" | "body" => {
                out.trim_inline();
                out.ensure_newline();
            }
            "td" | "th" if closing => {
                out.trim_inline();
                out.push('\t');
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Zip packages (OOXML and ODF)

struct Package {
    zip: ZipArchive<File>,
    out: Out,
    log: Log,
}

impl Package {
    fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("cannot read file: {e}"))?;
        match ZipArchive::new(file) {
            Ok(zip) => Ok(Package {
                zip,
                out: Out::new(),
                log: Log::default(),
            }),
            Err(e) => {
                let (head, _) = read_capped(path, 8).unwrap_or_default();
                if head.starts_with(b"\xD0\xCF\x11\xE0") {
                    // Password-protected OOXML is stored as an OLE container.
                    Err("encrypted or legacy office file".into())
                } else {
                    Err(format!("not a valid zip package: {e}"))
                }
            }
        }
    }

    fn has(&self, part: &str) -> bool {
        self.zip.index_for_name(part).is_some()
    }

    /// Stream one XML part through `f` until it returns false. False when the part is absent.
    fn walk(&mut self, part: &str, mut f: impl FnMut(&Event<'_>, &mut Out) -> bool) -> bool {
        let Package { zip, out, log } = self;
        if out.full {
            return zip.index_for_name(part).is_some();
        }
        walk_part(zip, part, log, |ev| f(ev, out) && !out.full)
    }

    fn finish(self, method: &'static str) -> Extracted {
        finish(
            method,
            &self.out.s,
            self.log.truncated || self.out.full,
            self.log.notes,
        )
    }
}

/// Streams a part through quick-xml behind a `PART_CAP` take; a part that
/// had more bytes than the cap is noted and marks the result truncated.
fn walk_part(
    zip: &mut ZipArchive<File>,
    part: &str,
    log: &mut Log,
    mut on_event: impl FnMut(&Event<'_>) -> bool,
) -> bool {
    let file = match zip.by_name(part) {
        Ok(f) => f,
        Err(ZipError::FileNotFound) => return false,
        Err(e) => {
            log.note(format!("{part}: {e}"));
            return true;
        }
    };
    let mut reader = Reader::from_reader(BufReader::new(file.take(PART_CAP)));
    reader.config_mut().check_end_names = false;
    let mut buf = Vec::new();
    let mut error = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(ev) => {
                if !on_event(&ev) {
                    break;
                }
            }
            Err(e) => {
                error = Some(e);
                break;
            }
        }
        buf.clear();
    }
    let mut capped = reader.into_inner().into_inner();
    if capped.limit() == 0 {
        let mut probe = [0u8; 1];
        if matches!(capped.get_mut().read(&mut probe), Ok(n) if n > 0) {
            log.note(format!("part too large: {part}"));
            log.truncated = true;
            return true;
        }
    }
    if let Some(e) = error {
        log.note(format!("malformed {part}: {e}"));
    }
    true
}

/// Text carried by a Text/CData/entity event.
fn xml_text(ev: &Event<'_>, mut sink: impl FnMut(&str)) {
    match ev {
        Event::Text(t) => {
            if let Ok(s) = t.decode() {
                sink(&s);
            }
        }
        Event::CData(t) => {
            if let Ok(s) = t.decode() {
                sink(&s);
            }
        }
        Event::GeneralRef(r) => {
            let c = if r.is_char_ref() {
                r.resolve_char_ref().ok().flatten()
            } else {
                r.decode().ok().and_then(|n| resolve_entity(&n))
            };
            if let Some(c) = c {
                sink(c.encode_utf8(&mut [0u8; 4]));
            }
        }
        _ => {}
    }
}

/// Attribute value by local name, entity-decoded.
fn attr(e: &BytesStart<'_>, local: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .map(|a| decode_entities(&String::from_utf8_lossy(&a.value)))
}

/// The prefixed `r:id` relationship attribute (not a bare `id`).
fn rel_id(e: &BytesStart<'_>) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.prefix().is_some() && a.key.local_name().as_ref() == b"id")
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn start_of<'a, 'b>(ev: &'a Event<'b>) -> Option<&'a BytesStart<'b>> {
    match ev {
        Event::Start(e) | Event::Empty(e) => Some(e),
        _ => None,
    }
}

/// Resolve a relationship target against the directory of its source part.
fn resolve_target(base: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_string(),
        None => format!("{base}/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Relationship Id -> part path, for internal targets.
fn read_rels(pkg: &mut Package, rels: &str, base: &str) -> Vec<(String, String)> {
    let mut map = Vec::new();
    pkg.walk(rels, |ev, _| {
        if let Some(e) = start_of(ev) {
            if e.local_name().as_ref() == b"Relationship"
                && attr(e, b"TargetMode").as_deref() != Some("External")
            {
                if let (Some(id), Some(target)) = (attr(e, b"Id"), attr(e, b"Target")) {
                    map.push((id, resolve_target(base, &target)));
                }
            }
        }
        true
    });
    map
}

fn lookup<'a>(map: &'a [(String, String)], id: &str) -> Option<&'a String> {
    map.iter().find(|(k, _)| k == id).map(|(_, v)| v)
}

/// Parts named `<prefix>N.xml`, ordered by N.
fn numbered_parts(pkg: &Package, prefix: &str) -> Vec<(usize, String)> {
    let mut v: Vec<(usize, String)> = pkg
        .zip
        .file_names()
        .filter_map(|n| {
            let k = n.strip_prefix(prefix)?.strip_suffix(".xml")?.parse().ok()?;
            Some((k, n.to_string()))
        })
        .collect();
    v.sort();
    v
}

/// WordprocessingML and DrawingML share these local names: t (text), r (run),
/// p (paragraph), tab, br, tc/tr (table cell/row). `mc:Fallback` repeats the
/// `mc:Choice` content, so it is skipped.
#[derive(Default)]
struct OoxmlText {
    in_t: bool,
    runs: u32,
    cells: u32,
    skip: u32,
}

impl OoxmlText {
    fn on_event(&mut self, ev: &Event<'_>, out: &mut Out) -> bool {
        if self.skip > 0 {
            match ev {
                Event::Start(_) => self.skip += 1,
                Event::End(_) => self.skip -= 1,
                _ => {}
            }
            return true;
        }
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                b"t" => self.in_t = true,
                b"r" => self.runs += 1,
                b"tc" => self.cells += 1,
                b"Fallback" => self.skip = 1,
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                // Outside runs, `tab` is a tab-stop definition, not a tab.
                b"tab" if self.runs > 0 => out.push('\t'),
                b"br" | b"cr" => out.push('\n'),
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"t" => self.in_t = false,
                b"r" => self.runs = self.runs.saturating_sub(1),
                b"p" if self.cells > 0 => out.soft_space(),
                b"p" => out.push('\n'),
                b"tc" => {
                    self.cells = self.cells.saturating_sub(1);
                    out.trim_inline();
                    out.push('\t');
                }
                b"tr" => {
                    out.trim_inline();
                    out.push('\n');
                }
                _ => {}
            },
            ev if self.in_t => xml_text(ev, |s| out.push_str(s)),
            _ => {}
        }
        true
    }
}

fn docx(path: &Path) -> Extracted {
    let mut pkg = match Package::open(path) {
        Ok(p) => p,
        Err(note) => return Extracted::empty("docx", Some(note)),
    };
    let mut body = OoxmlText::default();
    if !pkg.walk("word/document.xml", |ev, out| body.on_event(ev, out)) {
        pkg.log.note("word/document.xml missing");
    }
    for part in ["word/footnotes.xml", "word/endnotes.xml"] {
        if pkg.has(part) {
            pkg.out.ensure_blank_line();
            let mut notes = OoxmlText::default();
            pkg.walk(part, |ev, out| notes.on_event(ev, out));
        }
    }
    pkg.finish("docx")
}

/// Slides in presentation order (via presentation.xml), else by file number.
/// Labels are the 1-based position in the first case, the file number in the second.
fn slide_order(pkg: &mut Package) -> Vec<(usize, String)> {
    let rels = read_rels(pkg, "ppt/_rels/presentation.xml.rels", "ppt");
    let mut ids = Vec::new();
    pkg.walk("ppt/presentation.xml", |ev, _| {
        if let Some(e) = start_of(ev) {
            if e.local_name().as_ref() == b"sldId" {
                ids.extend(rel_id(e));
            }
        }
        !matches!(ev, Event::End(e) if e.local_name().as_ref() == b"sldIdLst")
    });
    let ordered: Vec<String> = ids
        .iter()
        .filter_map(|id| lookup(&rels, id))
        .filter(|p| pkg.has(p))
        .cloned()
        .collect();
    if !ordered.is_empty() {
        return ordered
            .into_iter()
            .enumerate()
            .map(|(i, p)| (i + 1, p))
            .collect();
    }
    numbered_parts(pkg, "ppt/slides/slide")
}

fn pptx(path: &Path) -> Extracted {
    let mut pkg = match Package::open(path) {
        Ok(p) => p,
        Err(note) => return Extracted::empty("pptx", Some(note)),
    };
    let slides = slide_order(&mut pkg);
    if slides.is_empty() {
        pkg.log.note("no slides found");
    }
    for (label, part) in slides {
        if pkg.out.full {
            break;
        }
        pkg.out.ensure_blank_line();
        pkg.out.push_str(&format!("--- slide {label} ---\n"));
        let mut w = OoxmlText::default();
        pkg.walk(&part, |ev, out| w.on_event(ev, out));
    }
    pkg.finish("pptx")
}

fn read_shared_strings(pkg: &mut Package) -> Vec<String> {
    let mut sst = Vec::new();
    let mut cur = String::new();
    let (mut in_si, mut in_t, mut phonetic) = (false, false, 0u32);
    pkg.walk("xl/sharedStrings.xml", |ev, _| {
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = true;
                    cur.clear();
                }
                b"t" if phonetic == 0 => in_t = true,
                // Phonetic reading runs (`rPh`) repeat the text in kana.
                b"rPh" => phonetic += 1,
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == b"si" => sst.push(String::new()),
            Event::End(e) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = false;
                    sst.push(std::mem::take(&mut cur));
                }
                b"t" => in_t = false,
                b"rPh" => phonetic = phonetic.saturating_sub(1),
                _ => {}
            },
            ev if in_si && in_t => xml_text(ev, |s| cur.push_str(s)),
            _ => {}
        }
        true
    });
    sst
}

/// (sheet name, worksheet part) in workbook order.
fn sheet_list(pkg: &mut Package) -> Vec<(String, String)> {
    let rels = read_rels(pkg, "xl/_rels/workbook.xml.rels", "xl");
    let mut declared = Vec::new();
    pkg.walk("xl/workbook.xml", |ev, _| {
        if let Some(e) = start_of(ev) {
            if e.local_name().as_ref() == b"sheet" {
                declared.push((attr(e, b"name").unwrap_or_default(), rel_id(e)));
            }
        }
        true
    });
    let mut sheets = Vec::new();
    for (i, (name, rid)) in declared.into_iter().enumerate() {
        let by_rel = rid
            .and_then(|r| lookup(&rels, &r).cloned())
            .filter(|p| pkg.has(p));
        let part = by_rel.unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", i + 1));
        if pkg.has(&part) {
            let name = if name.is_empty() {
                format!("Sheet{}", i + 1)
            } else {
                name
            };
            sheets.push((name, part));
        }
    }
    if sheets.is_empty() {
        sheets = numbered_parts(pkg, "xl/worksheets/sheet")
            .into_iter()
            .map(|(n, p)| (format!("Sheet{n}"), p))
            .collect();
    }
    sheets
}

/// Zero-based column of an A1-style reference ("C7" -> 2).
fn column_of(r: &str) -> Option<usize> {
    let letters: Vec<u8> = r.bytes().take_while(|b| b.is_ascii_alphabetic()).collect();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let n = letters.iter().fold(0usize, |n, b| {
        n * 26 + usize::from(b.to_ascii_uppercase() - b'A' + 1)
    });
    Some(n - 1)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    Shared,
    Bool,
    Plain,
}

struct SheetWalker<'a> {
    shared: &'a [String],
    row: Vec<String>,
    next_col: usize,
    col: usize,
    kind: CellKind,
    value: String,
    in_v: bool,
    in_is: bool,
    in_t: bool,
    phonetic: u32,
    rows: usize,
    capped: bool,
}

impl<'a> SheetWalker<'a> {
    fn new(shared: &'a [String]) -> Self {
        SheetWalker {
            shared,
            row: Vec::new(),
            next_col: 0,
            col: 0,
            kind: CellKind::Plain,
            value: String::new(),
            in_v: false,
            in_is: false,
            in_t: false,
            phonetic: 0,
            rows: 0,
            capped: false,
        }
    }

    fn begin_cell(&mut self, e: &BytesStart<'_>) {
        self.col = attr(e, b"r")
            .and_then(|r| column_of(&r))
            .unwrap_or(self.next_col)
            .min(MAX_SHEET_COLS - 1);
        self.kind = match attr(e, b"t").as_deref() {
            Some("s") => CellKind::Shared,
            Some("b") => CellKind::Bool,
            _ => CellKind::Plain,
        };
        self.value.clear();
    }

    fn end_cell(&mut self) {
        let raw = self.value.trim();
        let v = match self.kind {
            CellKind::Shared => raw
                .parse::<usize>()
                .ok()
                .and_then(|i| self.shared.get(i))
                .cloned()
                .unwrap_or_default(),
            CellKind::Bool => match raw {
                "1" => "TRUE".into(),
                "0" => "FALSE".into(),
                other => other.into(),
            },
            CellKind::Plain => self.value.clone(),
        };
        // One cell must stay on one line and in one column.
        let v = v.replace(['\t', '\r', '\n'], " ");
        if !v.trim().is_empty() {
            if self.row.len() <= self.col {
                self.row.resize(self.col + 1, String::new());
            }
            self.row[self.col] = v;
        }
        self.next_col = (self.col + 1).min(MAX_SHEET_COLS - 1);
    }

    fn end_row(&mut self, out: &mut Out) {
        while self.row.last().is_some_and(|c| c.is_empty()) {
            self.row.pop();
        }
        if !self.row.is_empty() {
            out.push_str(&self.row.join("\t"));
            out.push('\n');
            self.rows += 1;
        }
        self.row.clear();
    }

    fn on_event(&mut self, ev: &Event<'_>, out: &mut Out) -> bool {
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                b"row" => {
                    if self.rows >= MAX_SHEET_ROWS {
                        self.capped = true;
                        return false;
                    }
                    self.row.clear();
                    self.next_col = 0;
                }
                b"c" => self.begin_cell(e),
                b"v" => self.in_v = true,
                b"is" => self.in_is = true,
                b"t" if self.in_is && self.phonetic == 0 => self.in_t = true,
                b"rPh" => self.phonetic += 1,
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == b"c" => {
                self.begin_cell(e);
                self.end_cell();
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"c" => self.end_cell(),
                b"row" => self.end_row(out),
                b"v" => self.in_v = false,
                b"is" => self.in_is = false,
                b"t" => self.in_t = false,
                b"rPh" => self.phonetic = self.phonetic.saturating_sub(1),
                b"sheetData" => return false,
                _ => {}
            },
            ev if self.in_v || (self.in_is && self.in_t) => {
                xml_text(ev, |s| self.value.push_str(s))
            }
            _ => {}
        }
        true
    }
}

fn xlsx(path: &Path) -> Extracted {
    let mut pkg = match Package::open(path) {
        Ok(p) => p,
        Err(note) => return Extracted::empty("xlsx", Some(note)),
    };
    let shared = read_shared_strings(&mut pkg);
    let sheets = sheet_list(&mut pkg);
    if sheets.is_empty() {
        pkg.log.note("no worksheets found");
    }
    for (name, part) in sheets {
        if pkg.out.full {
            break;
        }
        pkg.out.ensure_blank_line();
        pkg.out.push_str(&format!("## {name}\n"));
        let mut w = SheetWalker::new(&shared);
        pkg.walk(&part, |ev, out| w.on_event(ev, out));
        if w.capped {
            pkg.log.truncated = true;
            pkg.log
                .note(format!("sheet {name} cut at {MAX_SHEET_ROWS} rows"));
        }
    }
    pkg.finish("xlsx")
}

/// ODF `content.xml`: text:p / text:h become lines, table cells are
/// tab-separated, odp pages get slide markers. Whitespace inside paragraphs is
/// collapsed as the ODF spec requires; `text:s` / `text:tab` add it back.
struct OdfText {
    kind: OdfKind,
    paras: u32,
    cells: u32,
    skip: u32,
    slides: usize,
}

impl OdfText {
    fn on_event(&mut self, ev: &Event<'_>, out: &mut Out) -> bool {
        if self.skip > 0 {
            match ev {
                Event::Start(_) => self.skip += 1,
                Event::End(_) => self.skip -= 1,
                _ => {}
            }
            return true;
        }
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                b"p" | b"h" => self.paras += 1,
                b"table-cell" | b"covered-table-cell" => self.cells += 1,
                // Comments, speaker notes and deleted tracked text.
                b"annotation" | b"notes" | b"tracked-changes" => self.skip = 1,
                b"table" if self.kind == OdfKind::Sheet => {
                    out.ensure_blank_line();
                    let name = attr(e, b"name").unwrap_or_default();
                    out.push_str(&format!("## {name}\n"));
                }
                b"page" if self.kind == OdfKind::Slides => {
                    self.slides += 1;
                    out.ensure_blank_line();
                    out.push_str(&format!("--- slide {} ---\n", self.slides));
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"s" if self.paras > 0 => {
                    let n = attr(e, b"c").and_then(|c| c.parse().ok()).unwrap_or(1usize);
                    out.push_str(&" ".repeat(n.min(100)));
                }
                b"tab" if self.paras > 0 => out.push('\t'),
                b"line-break" if self.paras > 0 => out.push('\n'),
                b"table-cell" | b"covered-table-cell" => {
                    let n = attr(e, b"number-columns-repeated")
                        .and_then(|c| c.parse().ok())
                        .unwrap_or(1usize);
                    out.push_str(&"\t".repeat(n.clamp(1, 256)));
                }
                b"p" | b"h" if self.cells == 0 => out.push('\n'),
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"p" | b"h" => {
                    self.paras = self.paras.saturating_sub(1);
                    if self.cells > 0 {
                        out.soft_space();
                    } else {
                        out.push('\n');
                    }
                }
                b"table-cell" | b"covered-table-cell" => {
                    self.cells = self.cells.saturating_sub(1);
                    out.trim_inline();
                    out.push('\t');
                }
                b"table-row" => {
                    out.trim_inline();
                    out.push('\n');
                }
                _ => {}
            },
            ev if self.paras > 0 => xml_text(ev, |s| out.push_collapsed(s)),
            _ => {}
        }
        true
    }
}

fn odf(path: &Path, kind: OdfKind) -> Extracted {
    let mut pkg = match Package::open(path) {
        Ok(p) => p,
        Err(note) => return Extracted::empty("odf", Some(note)),
    };
    let mut w = OdfText {
        kind,
        paras: 0,
        cells: 0,
        skip: 0,
        slides: 0,
    };
    if !pkg.walk("content.xml", |ev, out| w.on_event(ev, out)) {
        pkg.log.note("content.xml missing");
    }
    pkg.finish("odf")
}

// ---------------------------------------------------------------------------
// PDF

enum PdfRun {
    Text(String),
    Failed(String),
    Encrypted,
    /// Not handed to pdf-extract because it would recurse without bound.
    Refused(&'static str),
    Panicked,
    TimedOut,
}

/// NFKC targets of U+2F00..=U+2FD5 (Kangxi Radicals), 3 UTF-8 bytes each.
/// Some CJK fonts' ToUnicode maps (PingFang as embedded by Chrome, for one)
/// send common ideographs such as 一 文 用 to these radicals, which breaks
/// search. pdftotext folds them the same way.
const KANGXI_FOLD: &str = "\
    一丨丶丿乙亅二亠人儿入八冂冖冫几凵刀力勹匕匚匸十卜卩厂厶又口囗土士夂夊夕大女子宀\
    寸小尢尸屮山巛工己巾干幺广廴廾弋弓彐彡彳心戈戶手支攴文斗斤方无日曰月木欠止歹殳毋\
    比毛氏气水火爪父爻爿片牙牛犬玄玉瓜瓦甘生用田疋疒癶白皮皿目矛矢石示禸禾穴立竹米糸\
    缶网羊羽老而耒耳聿肉臣自至臼舌舛舟艮色艸虍虫血行衣襾見角言谷豆豕豸貝赤走足身車辛\
    辰辵邑酉釆里金長門阜隶隹雨靑非面革韋韭音頁風飛食首香馬骨高髟鬥鬯鬲鬼魚鳥鹵鹿麥麻\
    黃黍黑黹黽鼎鼓鼠鼻齊齒龍龜龠";

/// Map CJK radical code points back to the unified ideographs they stand for.
fn fold_cjk_radicals(s: &str) -> std::borrow::Cow<'_, str> {
    let is_radical = |c: char| ('\u{2E80}'..='\u{2FD5}').contains(&c);
    if !s.chars().any(is_radical) {
        return std::borrow::Cow::Borrowed(s);
    }
    s.chars()
        .map(|c| match c {
            '\u{2F00}'..='\u{2FD5}' => {
                let i = (c as usize - 0x2F00) * 3;
                KANGXI_FOLD
                    .get(i..i + 3)
                    .and_then(|t| t.chars().next())
                    .unwrap_or(c)
            }
            '\u{2E9F}' => '母',
            '\u{2EF3}' => '龟',
            c => c,
        })
        .collect()
}

/// Why pdf-extract must not run on `doc`, if it would recurse without bound.
///
/// pdf-extract recurses once per `Do` into the invoked XObject (any XObject,
/// image data included, parsed as content) and once per `/Parent` hop while a
/// page lacks Resources/MediaBox. Neither has a cycle check: a self-invoking
/// form overflows the stack, which aborts the whole process (no unwinding),
/// and a `/Parent` self-loop spins forever. This mirrors both walks.
fn pdf_recursion_hazard(doc: &pdf_extract::Document) -> Option<&'static str> {
    use pdf_extract::{content::Content, Dictionary, Document, Object, ObjectId};
    use std::collections::{HashMap, HashSet};

    // One level of indirection, as pdf-extract's `maybe_deref`.
    fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
        match o {
            Object::Reference(id) => doc.get_object(*id).ok(),
            o => Some(o),
        }
    }
    // `holder./Resources/XObject`, the dictionary `Do` names are looked up in.
    fn xobjects<'a>(doc: &'a Document, holder: &'a Dictionary) -> Option<&'a Dictionary> {
        let res = deref(doc, holder.get(b"Resources").ok()?)?.as_dict().ok()?;
        deref(doc, res.get(b"XObject").ok()?)?.as_dict().ok()
    }

    for (_, page) in doc.get_pages() {
        let mut seen = HashSet::new();
        let mut next = Some(page);
        while let Some(id) = next {
            if !seen.insert(id) {
                return Some("pdf page tree has a /Parent cycle");
            }
            if seen.len() > PDF_MAX_PARENT_HOPS {
                return Some("pdf page tree too deep");
            }
            next = doc
                .get_dictionary(id)
                .ok()
                .and_then(|d| d.get(b"Parent").ok())
                .and_then(|p| p.as_reference().ok());
        }
    }

    // Every name any resource dictionary binds, for XObjects that have no
    // Resources of their own and so resolve `Do` against their caller's.
    let mut by_name: HashMap<&[u8], Vec<ObjectId>> = HashMap::new();
    for obj in doc.objects.values() {
        let holder = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        for (name, target) in xobjects(doc, holder).into_iter().flat_map(|d| d.iter()) {
            if let Ok(id) = target.as_reference() {
                by_name.entry(name.as_slice()).or_default().push(id);
            }
        }
    }
    let nodes: HashSet<ObjectId> = by_name.values().flatten().copied().collect();
    let mut edges: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    for &id in &nodes {
        let Ok(Object::Stream(s)) = doc.get_object(id) else {
            continue;
        };
        let raw = if s.filters().is_ok() {
            s.decompressed_content()
                .unwrap_or_else(|_| s.content.clone())
        } else {
            s.content.clone()
        };
        let Ok(content) = Content::decode(&raw) else {
            continue;
        };
        let own_resources = s
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|r| deref(doc, r))
            .is_some_and(|r| r.as_dict().is_ok());
        let own = xobjects(doc, &s.dict);
        let out = edges.entry(id).or_default();
        for op in content.operations.iter().filter(|op| op.operator == "Do") {
            let Some(name) = op.operands.first().and_then(|n| n.as_name().ok()) else {
                continue;
            };
            if own_resources {
                let target = own.and_then(|d| d.get(name).ok());
                out.extend(target.and_then(|t| t.as_reference().ok()));
            } else if let Some(targets) = by_name.get(name) {
                out.extend(targets.iter().copied());
            }
        }
    }

    // Iterative DFS (this check must not recurse either): a back edge is a
    // cycle; finished nodes carry (depth, invocations) for the limits.
    let mut done: HashMap<ObjectId, (usize, u64)> = HashMap::new();
    let mut on_path: HashSet<ObjectId> = HashSet::new();
    for &root in &nodes {
        if done.contains_key(&root) {
            continue;
        }
        let mut stack: Vec<(ObjectId, usize)> = vec![(root, 0)];
        on_path.insert(root);
        while let Some((node, next)) = stack.pop() {
            let children = edges.get(&node).map(Vec::as_slice).unwrap_or(&[]);
            if let Some(&child) = children.get(next) {
                stack.push((node, next + 1));
                if on_path.contains(&child) {
                    return Some("pdf form xobjects invoke themselves");
                }
                if !done.contains_key(&child) {
                    on_path.insert(child);
                    stack.push((child, 0));
                }
                continue;
            }
            let (mut depth, mut calls) = (0usize, 1u64);
            for c in children {
                let (d, n) = done.get(c).copied().unwrap_or((0, 0));
                depth = depth.max(d);
                calls = calls.saturating_add(n);
            }
            if depth + 1 > PDF_MAX_FORM_DEPTH {
                return Some("pdf form xobjects nest too deeply");
            }
            if calls > PDF_MAX_FORM_CALLS {
                return Some("pdf form xobjects expand too much");
            }
            on_path.remove(&node);
            done.insert(node, (depth + 1, calls));
        }
    }
    None
}

/// Load, decrypt (empty password), check for recursion hazards, then run
/// pdf-extract's plain-text output. Same steps as
/// `pdf_extract::extract_text_from_mem`, plus the hazard check.
fn pdf_to_text(bytes: &[u8]) -> PdfRun {
    let mut doc = match pdf_extract::Document::load_mem(bytes) {
        Ok(d) => d,
        Err(e) => return PdfRun::Failed(e.to_string()),
    };
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return PdfRun::Encrypted;
    }
    if let Some(why) = pdf_recursion_hazard(&doc) {
        return PdfRun::Refused(why);
    }
    let mut text = String::new();
    match pdf_extract::output_doc(&doc, &mut pdf_extract::PlainTextOutput::new(&mut text)) {
        Ok(()) => PdfRun::Text(text),
        Err(e) => PdfRun::Failed(e.to_string()),
    }
}

/// Runs `pdf_to_text` on a worker thread: panics are caught, and a parse that
/// runs past `PDF_TIMEOUT` is abandoned (the thread cannot be killed and
/// finishes in the background).
fn run_pdf_extract(bytes: Arc<Vec<u8>>) -> PdfRun {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("pdf-extract".into())
        .stack_size(PDF_STACK_BYTES)
        .spawn(move || {
            let r = panic::catch_unwind(AssertUnwindSafe(|| pdf_to_text(&bytes)));
            let _ = tx.send(r.unwrap_or(PdfRun::Panicked));
        });
    if let Err(e) = spawned {
        return PdfRun::Failed(format!("cannot start parser thread: {e}"));
    }
    match rx.recv_timeout(PDF_TIMEOUT) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => PdfRun::TimedOut,
        Err(mpsc::RecvTimeoutError::Disconnected) => PdfRun::Panicked,
    }
}

fn pdf(path: &Path) -> Extracted {
    let failed = |note: String| Extracted {
        needs_vision: true,
        ..Extracted::empty("pdf", Some(note))
    };
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > PDF_MAX_BYTES {
        return Extracted::empty(
            "pdf",
            Some(format!("pdf too large to parse ({} MiB)", size >> 20)),
        );
    }
    let bytes = match read_capped(path, PDF_MAX_BYTES) {
        Ok((b, _)) => Arc::new(b),
        Err(note) => return Extracted::empty("pdf", Some(note)),
    };
    match run_pdf_extract(Arc::clone(&bytes)) {
        PdfRun::Text(raw) => {
            let mut x = finish("pdf", &fold_cjk_radicals(&raw), false, Vec::new());
            let visible = x.text.chars().filter(|c| !c.is_whitespace()).count();
            let garbled = x
                .text
                .chars()
                .filter(|&c| c == '\u{FFFD}' || ('\u{E000}'..='\u{F8FF}').contains(&c))
                .count();
            if visible < PDF_MIN_TEXT_CHARS {
                x.needs_vision = true;
                x.note = Some("pdf has little or no text layer".into());
            } else if garbled * 10 > visible * 3 {
                x.needs_vision = true;
                x.note = Some("pdf text layer looks garbled".into());
            }
            x
        }
        // Some encrypted files fail to load at all; the trailer still says so.
        PdfRun::Encrypted => failed("encrypted pdf".into()),
        PdfRun::Failed(_) if bytes.windows(8).any(|w| w == b"/Encrypt") => {
            failed("encrypted pdf".into())
        }
        PdfRun::Failed(e) => failed(format!("pdf parse error: {e}")),
        PdfRun::Refused(why) => failed(why.into()),
        PdfRun::Panicked => failed("pdf parser panicked".into()),
        PdfRun::TimedOut => failed("pdf parse timed out".into()),
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    fn write_zip(dir: &Path, name: &str, parts: &[(&str, &str)]) -> PathBuf {
        let p = dir.join(name);
        let mut zw = zip::ZipWriter::new(File::create(&p).unwrap());
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (part, body) in parts {
            zw.start_file(*part, opts).unwrap();
            zw.write_all(body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
        p
    }

    /// A PDF with correct xref offsets from bare object bodies (object i+1 = objects[i]).
    fn build_pdf(objects: &[String]) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                objects.len() + 1,
                xref
            )
            .bytes(),
        );
        out
    }

    fn one_page_pdf(content: &str) -> Vec<u8> {
        build_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .into(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                content
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        ])
    }

    #[test]
    fn pdf_with_text_layer_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = one_page_pdf(
            "BT /F1 24 Tf 72 720 Td (Invoice 2026 total 128.50) Tj \
             0 -36 Td (Paid by bank transfer, thank you.) Tj ET",
        );
        let p = write_file(dir.path(), "invoice.pdf", &bytes);
        let x = extract(&p, "invoice.pdf");
        assert_eq!(x.method, "pdf");
        assert_eq!(
            x.text,
            "Invoice 2026 total 128.50\nPaid by bank transfer, thank you."
        );
        assert!(!x.needs_vision, "{x:?}");
        assert_eq!(x.note, None);
    }

    #[test]
    fn pdf_with_one_short_line_counts_as_near_empty() {
        // 22 visible chars is under PDF_MIN_TEXT_CHARS: the text is still
        // returned, but the page is flagged for vision.
        let dir = tempfile::tempdir().unwrap();
        let bytes = one_page_pdf("BT /F1 24 Tf 72 720 Td (Invoice 2026 total 128.50) Tj ET");
        let p = write_file(dir.path(), "short.pdf", &bytes);
        let x = extract(&p, "short.pdf");
        assert_eq!(x.text, "Invoice 2026 total 128.50");
        assert!(x.needs_vision);
    }

    #[test]
    fn pdf_without_text_needs_vision() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "scan.pdf", &one_page_pdf(""));
        let x = extract(&p, "scan.pdf");
        assert_eq!(x.method, "pdf");
        assert!(x.needs_vision);
        assert_eq!(x.note.as_deref(), Some("pdf has little or no text layer"));
    }

    fn encrypted_pdf(user_password: &str) -> Vec<u8> {
        use pdf_extract::{EncryptionState, EncryptionVersion, Permissions, StringFormat};
        let plain = one_page_pdf(
            "BT /F1 24 Tf 72 720 Td (Quarterly numbers that only the owner may read.) Tj ET",
        );
        let mut doc = pdf_extract::Document::load_mem(&plain).unwrap();
        let id = pdf_extract::Object::String(b"0123456789abcdef".to_vec(), StringFormat::Literal);
        doc.trailer
            .set("ID", pdf_extract::Object::Array(vec![id.clone(), id]));
        let state = EncryptionState::try_from(EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password,
            key_length: 128,
            permissions: Permissions::all(),
        })
        .unwrap();
        doc.encrypt(&state).unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn encrypted_pdf_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "locked.pdf", &encrypted_pdf("user"));
        let x = extract(&p, "locked.pdf");
        assert_eq!(x.method, "pdf");
        assert_eq!(x.note.as_deref(), Some("encrypted pdf"));
        assert!(x.needs_vision && x.text.is_empty());

        // An empty user password only restricts permissions; text still comes out.
        let p = write_file(dir.path(), "restricted.pdf", &encrypted_pdf(""));
        let x = extract(&p, "restricted.pdf");
        assert_eq!(x.text, "Quarterly numbers that only the owner may read.");
        assert_eq!(x.note, None);
    }

    /// Page 3 invokes `/X1`; object 5 is the XObject (`form` = its content and
    /// extra dict entries), object 6 an optional second one.
    fn xobject_pdf(form5: &str, form6: Option<&str>) -> Vec<u8> {
        let stream = |dict: &str, body: &str| {
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 9 9] {dict} /Length {} >>\nstream\n{body}\nendstream",
                body.len()
            )
        };
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /XObject << /X1 5 0 R >> >> >>"
                .into(),
            "<< /Length 6 >>\nstream\n/X1 Do\nendstream".into(),
        ];
        let split = |f: &str| {
            f.split_once('|')
                .map(|(d, b)| (d.to_string(), b.to_string()))
                .unwrap()
        };
        let (d5, b5) = split(form5);
        objs.push(stream(&d5, &b5));
        if let Some(f6) = form6 {
            let (d6, b6) = split(f6);
            objs.push(stream(&d6, &b6));
        }
        build_pdf(&objs)
    }

    // pdf-extract has no cycle check: without the hazard check these abort
    // the whole test binary with a stack overflow (or spin forever).
    #[test]
    fn recursive_pdf_xobjects_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            (
                "self.pdf",
                xobject_pdf("/Resources << /XObject << /X1 5 0 R >> >>|/X1 Do", None),
            ),
            // No Resources of its own: resolves `/X1` against the page's, i.e. itself.
            ("inherit.pdf", xobject_pdf("|/X1 Do", None)),
            (
                "mutual.pdf",
                xobject_pdf(
                    "/Resources << /XObject << /B 6 0 R >> >>|/B Do",
                    Some("/Resources << /XObject << /A 5 0 R >> >>|/A Do"),
                ),
            ),
        ];
        for (name, bytes) in cases {
            let p = write_file(dir.path(), name, &bytes);
            let x = extract(&p, name);
            assert_eq!(
                x.note.as_deref(),
                Some("pdf form xobjects invoke themselves"),
                "{name}"
            );
            assert!(x.needs_vision, "{name}");
        }
        // A legitimate nested form is not refused.
        let ok = xobject_pdf(
            "/Resources << /XObject << /Inner 6 0 R >> >>|/Inner Do",
            Some("/Resources << >>|BT ET"),
        );
        let p = write_file(dir.path(), "nested.pdf", &ok);
        assert_eq!(
            extract(&p, "nested.pdf").note.as_deref(),
            Some("pdf has little or no text layer")
        );
    }

    #[test]
    fn pdf_parent_cycle_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = build_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            // No Resources, and /Parent points at itself.
            "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".into(),
            "<< /Length 5 >>\nstream\nBT ET\nendstream".into(),
        ]);
        let p = write_file(dir.path(), "loop.pdf", &bytes);
        let x = extract(&p, "loop.pdf");
        assert_eq!(x.note.as_deref(), Some("pdf page tree has a /Parent cycle"));
    }

    #[test]
    fn broken_pdf_is_reported_not_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            dir.path(),
            "bad.pdf",
            b"%PDF-1.7\n1 0 obj << /Type /Catalog",
        );
        let x = extract(&p, "bad.pdf");
        assert_eq!(x.method, "pdf");
        assert!(x.text.is_empty());
        assert!(x.needs_vision);
        let note = x.note.unwrap();
        assert!(
            note.starts_with("pdf parse error") || note == "pdf parser panicked",
            "{note}"
        );
    }

    #[test]
    fn gbk_text_decodes_to_chinese() {
        let dir = tempfile::tempdir().unwrap();
        let src = "你好，世界。\n这是一个测试文件，包含中文与 ASCII 混排。";
        let (gbk, _, _) = encoding_rs::GBK.encode(src);
        assert!(std::str::from_utf8(&gbk).is_err());
        let p = write_file(dir.path(), "gbk.txt", &gbk);
        let x = extract(&p, "gbk.txt");
        assert_eq!(x.method, "text");
        assert_eq!(x.text, src);
    }

    #[test]
    fn utf16le_with_bom() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xFF, 0xFE];
        for u in "Hello 世界\r\nSecond line  \r\n".encode_utf16() {
            bytes.extend(u.to_le_bytes());
        }
        let p = write_file(dir.path(), "u16.txt", &bytes);
        let x = extract(&p, "u16.txt");
        assert_eq!(x.text, "Hello 世界\nSecond line");
    }

    #[test]
    fn utf8_bom_and_blank_line_collapse() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            dir.path(),
            "a.md",
            "\u{FEFF}# Title   \n\n\n\n  indented\t\n\n\nend\n\n".as_bytes(),
        );
        let x = extract(&p, "a.md");
        assert_eq!(x.text, "# Title\n\n  indented\n\nend");
    }

    #[test]
    fn html_is_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let html = r#"<!DOCTYPE html><html><head><title>Ignored</title>
<style>body { color: red }</style></head>
<body><script>var x = "<p>not text</p>";</script>
<h1>Title</h1>
<p>a &amp; b &lt;c&gt; &quot;q&quot; &#39;s&#39; &#20013;&#x6587;&nbsp;end</p>
<div>one</div><div>two<br>three</div><!-- hidden comment -->
<table><tr><td>c1</td><td>c2</td></tr></table>
<ul><li>x</li><li>y</li></ul>
<pre>keep   spacing</pre>
</body></html>"#;
        let p = write_file(dir.path(), "page.html", html.as_bytes());
        let x = extract(&p, "page.html");
        assert_eq!(x.method, "html");
        let t = &x.text;
        assert!(!t.contains("var x") && !t.contains("not text"), "{t}");
        assert!(!t.contains("Ignored") && !t.contains("color"), "{t}");
        assert!(!t.contains("hidden"), "{t}");
        assert!(t.contains("a & b <c> \"q\" 's' 中文 end"), "{t}");
        assert!(t.contains("\none\ntwo\nthree\n"), "{t}");
        assert!(t.contains("c1\tc2"), "{t}");
        assert!(t.contains("x\ny"), "{t}");
        assert!(t.contains("keep   spacing"), "{t}");
        assert!(t.starts_with("Title\n\n"), "{t}");
    }

    #[test]
    fn unknown_extension_is_sniffed() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "blob.xyz", b"\x00\x01\x02binary\x00stuff");
        let x = extract(&p, "blob.xyz");
        assert_eq!(x.method, "none");
        assert!(!x.needs_vision);
        assert_eq!(x.note.as_deref(), Some("binary file not supported"));

        let p = write_file(dir.path(), "notes.xyz", b"plain words\n");
        let x = extract(&p, "notes.xyz");
        assert_eq!((x.method, x.text.as_str()), ("text", "plain words"));

        let p = write_file(dir.path(), "Makefile", b"all:\n\tcargo build\n");
        assert_eq!(extract(&p, "Makefile").text, "all:\n\tcargo build");

        let pdf = one_page_pdf(
            "BT /F1 12 Tf 72 720 Td (Sniffed upload without an extension keeps its text) Tj ET",
        );
        let p = write_file(dir.path(), "upload", &pdf);
        let x = extract(&p, "upload");
        assert_eq!(x.method, "pdf");
        assert!(x.text.contains("Sniffed upload"), "{x:?}");
        assert!(!x.needs_vision, "{x:?}");
    }

    #[test]
    fn skipped_formats_say_why() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            dir.path(),
            "old.doc",
            b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1rest",
        );
        let x = extract(&p, "old.doc");
        assert_eq!(x.method, "none");
        assert_eq!(x.note.as_deref(), Some("legacy .doc not supported"));
        let x = extract(&p, "mystery");
        assert_eq!(
            x.note.as_deref(),
            Some("legacy office binary not supported")
        );
        assert_eq!(
            extract(&p, "a.mp3").note.as_deref(),
            Some("audio not supported")
        );
        assert_eq!(
            extract(&p, "a.MOV").note.as_deref(),
            Some("video not supported")
        );
        assert_eq!(
            extract(&p, "a.zip").note.as_deref(),
            Some("archive not supported")
        );
        assert_eq!(
            extract(&p, "a.rtf").note.as_deref(),
            Some("unsupported file type .rtf")
        );
        let x = extract(&dir.path().join("missing.txt"), "missing.txt");
        assert_eq!(x.method, "none");
        assert!(x.note.unwrap().starts_with("cannot read file"));
        let x = extract(dir.path(), "dir.txt");
        assert_eq!(x.note.as_deref(), Some("not a regular file"));
    }

    #[test]
    fn images_need_vision() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "x.bin", b"\x89PNG\r\n\x1a\n....");
        for name in [
            "photo.JPG",
            "a.png",
            "b.webp",
            "IMG_1.HEIC",
            "scan.tiff",
            "c.bmp",
            "noext",
        ] {
            let x = extract(&p, name);
            assert_eq!((x.method, x.needs_vision), ("none", true), "{name}");
            assert!(x.text.is_empty());
        }
    }

    #[test]
    fn image_tables() {
        assert_eq!(vision_image_type("a.png"), Some("image/png"));
        assert_eq!(vision_image_type("a.JPG"), Some("image/jpeg"));
        assert_eq!(vision_image_type("a.jpeg"), Some("image/jpeg"));
        assert_eq!(vision_image_type("a.gif"), Some("image/gif"));
        assert_eq!(vision_image_type("dir/a.WebP"), Some("image/webp"));
        for n in ["a.heic", "a.bmp", "a.svg", "a.pdf", "png", ""] {
            assert_eq!(vision_image_type(n), None, "{n}");
        }
        for n in ["a.heic", "a.HEIF", "a.tif", "a.tiff", "a.bmp"] {
            assert!(needs_image_conversion(n), "{n}");
        }
        for n in ["a.png", "a.jpg", "a.webp", "a.gif", "a.pdf", "heic"] {
            assert!(!needs_image_conversion(n), "{n}");
        }
    }

    const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

    #[test]
    fn docx_text_tabs_breaks_tables_footnotes() {
        let dir = tempfile::tempdir().unwrap();
        let doc = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W_NS}><w:body>
<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t>Hello</w:t></w:r><w:r><w:tab/><w:t xml:space="preserve">world &amp; co</w:t></w:r></w:p>
<w:p><w:r><w:t>Line one</w:t><w:br/><w:t>Line two</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></mc:Choice><mc:Fallback><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></mc:Fallback></mc:AlternateContent></w:r></w:p>
<w:p><w:del><w:r><w:delText>deleted</w:delText></w:r></w:del><w:r><w:t>kept</w:t></w:r></w:p>
</w:body></w:document>"#
        );
        let foot = format!(
            r#"<w:footnotes {W_NS}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:t>A footnote</w:t></w:r></w:p></w:footnote></w:footnotes>"#
        );
        let p = write_zip(
            dir.path(),
            "a.docx",
            &[("word/document.xml", &doc), ("word/footnotes.xml", &foot)],
        );
        let x = extract(&p, "a.docx");
        assert_eq!(x.method, "docx");
        assert_eq!(
            x.text,
            "Hello\tworld & co\nLine one\nLine two\nA1\tB1\nBoxed\n\nkept\n\nA footnote"
        );
        assert_eq!((x.truncated, x.note), (false, None));
    }

    #[test]
    fn docx_bomb_is_capped() {
        // 70 MiB of spaces deflates to a few hundred KiB; reading stops at
        // PART_CAP (64 MiB). Takes well under a second even in debug builds.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bomb.docx");
        let mut zw = zip::ZipWriter::new(File::create(&p).unwrap());
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(1));
        zw.start_file("word/document.xml", opts).unwrap();
        write!(
            zw,
            "<w:document {W_NS}><w:body><w:p><w:r><w:t>hello</w:t></w:r></w:p>"
        )
        .unwrap();
        let chunk = vec![b' '; 1 << 20];
        for _ in 0..70 {
            zw.write_all(&chunk).unwrap();
        }
        write!(
            zw,
            "<w:p><w:r><w:t>tail</w:t></w:r></w:p></w:body></w:document>"
        )
        .unwrap();
        zw.finish().unwrap();
        assert!(fs::metadata(&p).unwrap().len() < 1 << 20);

        let started = std::time::Instant::now();
        let x = extract(&p, "bomb.docx");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(x.method, "docx");
        assert_eq!(x.text, "hello");
        assert!(x.truncated);
        assert_eq!(x.note.as_deref(), Some("part too large: word/document.xml"));
    }

    #[test]
    fn docx_text_budget_stops_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let big = "word ".repeat(RAW_TEXT_BUDGET / 4);
        let doc = format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>{big}</w:t></w:r></w:p></w:body></w:document>");
        let p = write_zip(dir.path(), "big.docx", &[("word/document.xml", &doc)]);
        let x = extract(&p, "big.docx");
        assert!(x.truncated);
        assert_eq!(x.text.chars().count(), MAX_TEXT_CHARS - 1); // trailing space trimmed
    }

    const S_NS: &str = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    #[test]
    fn xlsx_cells_and_sheets() {
        let dir = tempfile::tempdir().unwrap();
        let workbook = format!(
            r#"<workbook {S_NS}><sheets><sheet name="Data &amp; more" sheetId="1" r:id="rId7"/><sheet name="Second" sheetId="2" r:id="rId8"/></sheets></workbook>"#
        );
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId8" Type="worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId7" Type="worksheet" Target="/xl/worksheets/sheet2.xml"/><Relationship Id="rId9" Type="hyperlink" Target="https://x" TargetMode="External"/></Relationships>"#;
        let sst = format!(
            r#"<sst {S_NS} count="3" uniqueCount="3"><si><t>Name</t></si><si><r><rPr><b/></rPr><t>Hel</t></r><r><t xml:space="preserve">lo</t></r></si><si><t>東京</t><rPh sb="0" eb="2"><t>トウキョウ</t></rPh></si></sst>"#
        );
        let data = format!(
            r#"<worksheet {S_NS}><sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="D1" t="inlineStr"><is><t>inline</t></is></c></row>
<row r="2"><c r="A2"><v>128.5</v></c><c r="B2" t="b"><v>1</v></c><c r="C2" t="str"><f>A2*2</f><v>257</v></c><c r="D2" t="b"><v>0</v></c></row>
<row r="3"><c r="A3" s="1"/><c r="B3" t="s"><v>2</v></c></row>
<row r="4"/>
</sheetData><mergeCells count="0"/></worksheet>"#
        );
        let second = format!(
            r#"<worksheet {S_NS}><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>only</t></is></c></row></sheetData></worksheet>"#
        );
        let p = write_zip(
            dir.path(),
            "book.xlsx",
            &[
                ("xl/workbook.xml", &workbook),
                ("xl/_rels/workbook.xml.rels", rels),
                ("xl/sharedStrings.xml", &sst),
                ("xl/worksheets/sheet1.xml", &second),
                ("xl/worksheets/sheet2.xml", &data),
            ],
        );
        let x = extract(&p, "book.xlsx");
        assert_eq!(x.method, "xlsx");
        assert_eq!(
            x.text,
            "## Data & more\nName\tHello\t\tinline\n128.5\tTRUE\t257\tFALSE\n\t東京\n\n## Second\nonly"
        );
        assert_eq!((x.truncated, x.note), (false, None));
    }

    #[test]
    fn xlsx_row_cap_and_fallback_sheet_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut rows = String::new();
        for r in 1..=MAX_SHEET_ROWS + 5 {
            rows.push_str(&format!(r#"<row r="{r}"><c r="A{r}"><v>{r}</v></c></row>"#));
        }
        let sheet = format!(r#"<worksheet {S_NS}><sheetData>{rows}</sheetData></worksheet>"#);
        // No workbook.xml: sheets come from xl/worksheets/sheetN.xml.
        let p = write_zip(
            dir.path(),
            "rows.xlsx",
            &[("xl/worksheets/sheet1.xml", &sheet)],
        );
        let x = extract(&p, "rows.xlsx");
        assert!(x.text.starts_with("## Sheet1\n1\n2\n"), "{}", &x.text[..40]);
        assert!(x.text.ends_with(&format!("\n{MAX_SHEET_ROWS}")));
        assert!(x.truncated);
        assert_eq!(x.note.as_deref(), Some("sheet Sheet1 cut at 2000 rows"));
    }

    fn slide(text: &str) -> String {
        format!(
            r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:tabLst><a:tab pos="1" algn="l"/></a:tabLst></a:pPr><a:r><a:t>{text}</a:t></a:r></a:p><a:p><a:r><a:t>Body</a:t></a:r><a:br/><a:r><a:t>more</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        )
    }

    #[test]
    fn pptx_slides_in_numeric_order() {
        let dir = tempfile::tempdir().unwrap();
        let (s1, s2, s10) = (slide("First"), slide("Second"), slide("Tenth"));
        let p = write_zip(
            dir.path(),
            "deck.pptx",
            &[
                ("ppt/slides/slide10.xml", &s10),
                ("ppt/slides/slide2.xml", &s2),
                ("ppt/slides/slide1.xml", &s1),
            ],
        );
        let x = extract(&p, "deck.pptx");
        assert_eq!(x.method, "pptx");
        assert_eq!(
            x.text,
            "--- slide 1 ---\nFirst\nBody\nmore\n\n--- slide 2 ---\nSecond\nBody\nmore\n\n\
             --- slide 10 ---\nTenth\nBody\nmore"
        );
    }

    #[test]
    fn pptx_follows_presentation_order() {
        let dir = tempfile::tempdir().unwrap();
        let pres = r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="257" r:id="rId3"/><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#;
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="slide" Target="slides/slide2.xml"/></Relationships>"#;
        let (s1, s2) = (slide("Moved down"), slide("Moved up"));
        let p = write_zip(
            dir.path(),
            "deck.pptx",
            &[
                ("ppt/presentation.xml", pres),
                ("ppt/_rels/presentation.xml.rels", rels),
                ("ppt/slides/slide1.xml", &s1),
                ("ppt/slides/slide2.xml", &s2),
            ],
        );
        let t = extract(&p, "deck.pptx").text;
        assert!(t.starts_with("--- slide 1 ---\nMoved up\n"), "{t}");
        assert!(t.contains("--- slide 2 ---\nMoved down\n"), "{t}");
    }

    #[test]
    fn odt_paragraphs_spaces_tables() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:text><text:h text:outline-level="1">Heading</text:h><text:p>Para with<text:s text:c="3"/>gap<text:tab/>tab and   collapsed
   spaces</text:p><table:table table:name="T1"><table:table-row><table:table-cell><text:p>c1</text:p></table:table-cell><table:table-cell table:number-columns-repeated="2"/><table:table-cell><text:p>c4</text:p></table:table-cell></table:table-row></table:table><text:p>After<office:annotation><text:p>comment</text:p></office:annotation> table &amp; more</text:p></office:text></office:body></office:document-content>"#;
        let p = write_zip(
            dir.path(),
            "a.odt",
            &[
                ("mimetype", "application/vnd.oasis.opendocument.text"),
                ("content.xml", content),
            ],
        );
        let x = extract(&p, "a.odt");
        assert_eq!(x.method, "odf");
        assert_eq!(
            x.text,
            "Heading\nPara with   gap\ttab and collapsed spaces\nc1\t\t\tc4\nAfter table & more"
        );
        // Same package without an extension is recognised by its parts.
        let y = extract(&p, "upload");
        assert_eq!((y.method, y.text), ("odf", x.text));
    }

    #[test]
    fn ods_sheets_get_headers() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:spreadsheet><table:table table:name="Q1"><table:table-row><table:table-cell><text:p>a</text:p></table:table-cell><table:table-cell><text:p>1</text:p></table:table-cell><table:table-cell table:number-columns-repeated="16380"/></table:table-row><table:table-row table:number-rows-repeated="1048575"><table:table-cell table:number-columns-repeated="16384"/></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#;
        let p = write_zip(dir.path(), "s.ods", &[("content.xml", content)]);
        assert_eq!(extract(&p, "s.ods").text, "## Q1\na\t1");
    }

    #[test]
    fn invalid_package_is_noted() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "fake.docx", b"not a zip at all");
        let x = extract(&p, "fake.docx");
        assert_eq!(x.method, "docx");
        assert!(x.text.is_empty());
        assert!(x.note.unwrap().starts_with("not a valid zip package"));
        let p = write_zip(dir.path(), "empty.docx", &[("other.xml", "<a/>")]);
        assert_eq!(
            extract(&p, "empty.docx").note.as_deref(),
            Some("word/document.xml missing")
        );
        let p = write_zip(
            dir.path(),
            "broken.docx",
            &[("word/document.xml", "<w:t>ok</w:t><<<")],
        );
        let x = extract(&p, "broken.docx");
        assert_eq!(x.text, "ok");
        assert!(x.note.unwrap().starts_with("malformed word/document.xml"));
    }

    #[test]
    fn truncates_on_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            dir.path(),
            "long.txt",
            "中".repeat(MAX_TEXT_CHARS + 50).as_bytes(),
        );
        let x = extract(&p, "long.txt");
        assert!(x.truncated);
        assert_eq!(x.text.chars().count(), MAX_TEXT_CHARS);
        let p = write_file(
            dir.path(),
            "short.txt",
            "中".repeat(MAX_TEXT_CHARS).as_bytes(),
        );
        assert!(!extract(&p, "short.txt").truncated);
    }

    #[test]
    fn text_read_cap_keeps_valid_prefix() {
        let dir = tempfile::tempdir().unwrap();
        // 3-byte chars so the 4 MiB cap lands mid-character.
        let body = "中".repeat((TEXT_READ_CAP as usize) / 3 + 10);
        let p = write_file(dir.path(), "big.txt", body.as_bytes());
        let x = extract(&p, "big.txt");
        assert!(x.truncated);
        assert!(x.text.chars().all(|c| c == '中'));
        // GBK: a one-byte prefix makes the cap split a two-byte char; the
        // partial tail must be dropped, not turn the file into lossy UTF-8.
        let (unit, _, _) = encoding_rs::GBK.encode("中文");
        let mut gbk = b"x".to_vec();
        gbk.extend(unit.repeat(TEXT_READ_CAP as usize / unit.len() + 1));
        let p = write_file(dir.path(), "big-gbk.txt", &gbk);
        let x = extract(&p, "big-gbk.txt");
        assert!(x.truncated);
        assert!(x.text.starts_with("x中文中文"), "{}", &x.text[..20]);
        assert!(x.text[1..].chars().all(|c| c == '中' || c == '文'));
    }

    #[test]
    fn helpers() {
        assert_eq!(column_of("A1"), Some(0));
        assert_eq!(column_of("C7"), Some(2));
        assert_eq!(column_of("AA10"), Some(26));
        assert_eq!(column_of("XFD1"), Some(16383));
        assert_eq!(column_of("12"), None);
        assert_eq!(
            resolve_target("xl", "worksheets/s.xml"),
            "xl/worksheets/s.xml"
        );
        assert_eq!(resolve_target("xl", "/xl/a.xml"), "xl/a.xml");
        assert_eq!(
            resolve_target("ppt/slides", "../media/x.png"),
            "ppt/media/x.png"
        );
        assert_eq!(decode_entities("&amp;&#x41;&#66;&bogus;&"), "&AB&bogus;&");
        assert_eq!(KANGXI_FOLD.len(), 214 * 3);
        assert_eq!(KANGXI_FOLD.chars().count(), 214);
        assert_eq!(
            fold_cjk_radicals("这是\u{2F00}份\u{2F64}来\u{2F42}本，\u{2FD5}\u{2EF3}"),
            "这是一份用来文本，龠龟"
        );
        assert_eq!(normalize("a\r\nb\rc\u{0}d\u{0C}e"), "a\nb\ncd\ne");
    }
}
