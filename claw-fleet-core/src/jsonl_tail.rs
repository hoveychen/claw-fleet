//! Efficient tail-of-file reader for JSONL session files.
//!
//! `Backend::get_messages` historically reads the entire file with
//! `read_to_string` and parses every line. For the SessionDetail UI we only
//! need the most recent N lines to render the conversation, and a 50 MB
//! transcript will lock up the webview. This module provides a reverse-byte
//! scanner that seeks from the end and reads chunks backwards until enough
//! newlines have been accumulated, so we never materialize the whole file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

const CHUNK_SIZE: usize = 64 * 1024;

/// Read up to the last `n` non-empty lines of `path` and JSON-parse each one.
/// Lines that fail to parse are silently skipped, matching the existing
/// `get_messages` behavior. Returned values preserve file order (oldest first).
pub fn read_tail_lines_as_json(path: &Path, n: usize) -> std::io::Result<Vec<Value>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();
    if file_size == 0 {
        return Ok(Vec::new());
    }

    // Read backward until we have collected at least n+1 newlines (so we can
    // safely discard the partial leading line) or until we reach BOF.
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = file_size;
    let target_newlines = n + 1;

    while pos > 0 {
        let read_len = std::cmp::min(CHUNK_SIZE as u64, pos);
        pos -= read_len;
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0u8; read_len as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&buf);
        buf = chunk;
        let newline_count = buf.iter().filter(|&&b| b == b'\n').count();
        if newline_count >= target_newlines {
            break;
        }
    }

    // `from_utf8_lossy` turns any mid-codepoint head bytes into U+FFFD; the
    // partial leading line is discarded below regardless.
    let s = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = s.lines().collect();
    if lines.len() > n {
        let drop = lines.len() - n;
        lines.drain(..drop);
    }
    Ok(lines
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Parse a *forward* incremental tail buffer read from a saved byte offset to
/// EOF, returning the JSON-parsed complete lines plus the number of bytes that
/// were definitively consumed — the length up to and including the last `\n` in
/// `buf`.
///
/// A trailing fragment with no newline is a record still being flushed by the
/// writer; it is left **unconsumed** so the caller re-reads it once the write
/// finishes. Callers MUST advance their saved offset by the returned `consumed`
/// (i.e. `offset + consumed`), never to EOF — advancing past a partial line
/// skips its bytes, and when the rest lands the next read starts mid-record,
/// so that record (often the first thinking/tool_use block of a resumed turn,
/// written in a burst) is malformed forever and permanently lost.
///
/// Complete-but-malformed lines are counted as consumed (re-reading can't help)
/// but dropped from the parsed output, matching `read_tail_lines_as_json`.
pub fn parse_incremental_tail(buf: &str) -> (Vec<Value>, usize) {
    // Consume only through the last newline; a trailing fragment with no `\n`
    // is a half-written record left for the next read (a `\n` is one byte, so
    // `idx + 1` is a valid char boundary right after it).
    let consumed = buf.rfind('\n').map_or(0, |idx| idx + 1);
    let complete = &buf[..consumed];
    let lines = complete
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    (lines, consumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("jsonl_tail_test_{}_{}.jsonl", name, std::process::id()));
        let mut f = File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    #[test]
    fn empty_file_returns_empty() {
        let p = write_tmp("empty", b"");
        let out = read_tail_lines_as_json(&p, 10).unwrap();
        assert!(out.is_empty());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn n_zero_returns_empty() {
        let p = write_tmp("zero", b"{\"a\":1}\n{\"a\":2}\n");
        let out = read_tail_lines_as_json(&p, 0).unwrap();
        assert!(out.is_empty());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn returns_last_n_in_file_order() {
        let p = write_tmp(
            "tail3",
            b"{\"i\":1}\n{\"i\":2}\n{\"i\":3}\n{\"i\":4}\n{\"i\":5}\n",
        );
        let out = read_tail_lines_as_json(&p, 3).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["i"], 3);
        assert_eq!(out[1]["i"], 4);
        assert_eq!(out[2]["i"], 5);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn n_larger_than_file_returns_all() {
        let p = write_tmp("small", b"{\"i\":1}\n{\"i\":2}\n");
        let out = read_tail_lines_as_json(&p, 100).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["i"], 1);
        assert_eq!(out[1]["i"], 2);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn no_trailing_newline() {
        let p = write_tmp("no_nl", b"{\"i\":1}\n{\"i\":2}");
        let out = read_tail_lines_as_json(&p, 5).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[1]["i"], 2);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let p = write_tmp(
            "bad",
            b"{\"i\":1}\nnot json\n{\"i\":3}\n   \n{\"i\":5}\n",
        );
        let out = read_tail_lines_as_json(&p, 10).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["i"], 1);
        assert_eq!(out[1]["i"], 3);
        assert_eq!(out[2]["i"], 5);
        std::fs::remove_file(p).ok();
    }

    // ── parse_incremental_tail (forward incremental readers) ─────────────────

    #[test]
    fn incremental_leaves_trailing_partial_line_unconsumed() {
        // Simulates the watcher firing mid-burst: "A\n" is fully flushed, but
        // the next record is only partially written (no trailing newline yet).
        // The partial fragment must NOT be consumed — its bytes have to be
        // re-read once the writer finishes, or the record is lost forever.
        let buf = "{\"i\":1}\n{\"i\":2,\"partia";
        let (lines, consumed) = parse_incremental_tail(buf);
        assert_eq!(lines.len(), 1, "only the complete line parses");
        assert_eq!(lines[0]["i"], 1);
        // "{\"i\":1}\n" is 8 bytes; the 13-byte partial fragment stays unconsumed.
        assert_eq!(consumed, 8, "offset must stop at the last newline, not EOF");
    }

    #[test]
    fn incremental_resumes_partial_line_next_tick() {
        // First tick sees the partial; second tick reads from `consumed` and
        // must recover the record that was mid-flight before.
        let first = "{\"i\":1}\n{\"i\":2,\"partia";
        let (l1, consumed1) = parse_incremental_tail(first);
        assert_eq!(l1.len(), 1);
        // The rest of record 2 plus a full record 3 land next.
        let full = "{\"i\":1}\n{\"i\":2,\"partial\":true}\n{\"i\":3}\n";
        let (l2, _) = parse_incremental_tail(&full[consumed1..]);
        assert_eq!(l2.len(), 2, "record 2 recovered, record 3 read");
        assert_eq!(l2[0]["i"], 2);
        assert_eq!(l2[0]["partial"], true);
        assert_eq!(l2[1]["i"], 3);
    }

    #[test]
    fn incremental_no_complete_line_consumes_nothing() {
        let (lines, consumed) = parse_incremental_tail("{\"i\":1,\"still_wr");
        assert!(lines.is_empty());
        assert_eq!(consumed, 0);
    }

    #[test]
    fn incremental_complete_lines_all_consumed() {
        let buf = "{\"i\":1}\n{\"i\":2}\n";
        let (lines, consumed) = parse_incremental_tail(buf);
        assert_eq!(lines.len(), 2);
        assert_eq!(consumed, buf.len(), "a clean newline-terminated buffer is fully consumed");
    }

    #[test]
    fn incremental_malformed_complete_line_consumed_but_dropped() {
        // A complete (newline-terminated) but unparseable line can't be helped
        // by re-reading, so it is consumed yet omitted from the parsed output.
        let buf = "not json\n{\"i\":2}\n";
        let (lines, consumed) = parse_incremental_tail(buf);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["i"], 2);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn spans_multiple_chunks() {
        // Build a payload larger than two CHUNK_SIZE windows so the loop has
        // to iterate at least twice, exercising the chunk-stitching path.
        let line = format!("{{\"x\":\"{}\"}}", "a".repeat(1024));
        let mut content = String::new();
        for _ in 0..200 {
            content.push_str(&line);
            content.push('\n');
        }
        let p = write_tmp("big", content.as_bytes());
        let out = read_tail_lines_as_json(&p, 5).unwrap();
        assert_eq!(out.len(), 5);
        for v in &out {
            assert!(v["x"].as_str().unwrap().starts_with('a'));
        }
        std::fs::remove_file(p).ok();
    }
}
