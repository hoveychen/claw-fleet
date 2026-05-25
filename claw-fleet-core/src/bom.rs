use std::fs;
use std::io;
use std::path::Path;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Read a file to a `String`, stripping a leading UTF-8 BOM if present.
///
/// CC transcripts produced by Claude Code itself are clean UTF-8, but some
/// Windows editors (Notepad, older PowerShell redirects) silently prepend the
/// `EF BB BF` byte sequence. The BOM survives `fs::read_to_string` as a
/// leading `U+FEFF`, which then makes `serde_json::from_str` fail on the very
/// first character of the first line.
pub fn read_to_string_no_bom(path: impl AsRef<Path>) -> io::Result<String> {
    let bytes = fs::read(path)?;
    let slice = if bytes.starts_with(UTF8_BOM) {
        &bytes[UTF8_BOM.len()..]
    } else {
        &bytes[..]
    };
    Ok(String::from_utf8_lossy(slice).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(prefix: &[u8], body: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tmp");
        f.write_all(prefix).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn bom_prefixed_jsonl_parses_as_json() {
        let body = r#"{"role":"user","content":"hi"}"#;
        let f = write_fixture(UTF8_BOM, body);

        let s = read_to_string_no_bom(f.path()).expect("read");

        // The BOM must NOT survive into the returned string, otherwise the
        // first call to serde_json::from_str on the first line will fail with
        // "trailing characters" / "expected value at column 1".
        assert!(
            !s.starts_with('\u{FEFF}'),
            "BOM leaked into returned string: first chars = {:?}",
            s.chars().take(4).collect::<String>()
        );
        let parsed: serde_json::Value =
            serde_json::from_str(s.trim_end()).expect("json parses");
        assert_eq!(parsed["role"], "user");
    }

    #[test]
    fn plain_utf8_passes_through_unchanged() {
        let body = "{\"a\":1}\n{\"b\":2}\n";
        let f = write_fixture(&[], body);
        let s = read_to_string_no_bom(f.path()).expect("read");
        assert_eq!(s, body);
    }
}
