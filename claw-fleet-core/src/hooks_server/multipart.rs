//! Minimal `multipart/form-data` parser (RFC 7578) for the `/v1/files` upload.
//!
//! Why hand-rolled: the server is `tiny_http` (synchronous, no framework), and
//! the only multipart consumer is one endpoint with two fields — a file part and
//! a short text part. The crates that would do this are either async-runtime
//! bound or unmaintained, so a ~100-line pure parser with tests is the smaller
//! permanent cost. It is deliberately strict: it walks delimiters, it does not
//! try to recover from a malformed body, and it never allocates more than the
//! caller already read into memory.
//!
//! Scope: no nested `multipart/mixed`, no `Content-Transfer-Encoding` decoding
//! (browsers and the OpenAI SDKs send binary as-is), no streaming to disk.

/// One parsed form field.
#[derive(Debug, Clone, PartialEq)]
pub struct FormPart {
    /// `name=` from Content-Disposition. Empty when the header omitted it.
    pub name: String,
    /// `filename=` from Content-Disposition, when present (i.e. a file field).
    pub filename: Option<String>,
    /// The part's own Content-Type, when it declared one.
    pub content_type: Option<String>,
    /// Raw bytes of the part body, trailing CRLF removed.
    pub data: Vec<u8>,
}

impl FormPart {
    /// The body as UTF-8 text with surrounding whitespace trimmed. Used for
    /// short scalar fields like `purpose`.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).trim().to_string()
    }
}

/// Extract the `boundary` parameter from a `Content-Type` header value.
/// Returns `None` when the type isn't multipart or the parameter is missing.
/// Handles both quoted and bare boundaries, and is case-insensitive on the
/// type and parameter name (RFC 7578 §4.1 allows either).
pub fn boundary_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    if !lower.starts_with("multipart/form-data") {
        return None;
    }
    for param in content_type.split(';').skip(1) {
        let param = param.trim();
        let (key, value) = param.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("boundary") {
            continue;
        }
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

/// Find `needle` in `haystack` at or after `from`.
fn find_sub(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Parse one part's header block into (name, filename, content_type).
fn parse_part_headers(raw: &[u8]) -> (String, Option<String>, Option<String>) {
    let text = String::from_utf8_lossy(raw);
    let mut name = String::new();
    let mut filename = None;
    let mut content_type = None;
    for line in text.split("\r\n") {
        let Some((header, value)) = line.split_once(':') else {
            continue;
        };
        let header = header.trim().to_ascii_lowercase();
        let value = value.trim();
        if header == "content-type" {
            content_type = Some(value.to_string());
        } else if header == "content-disposition" {
            // form-data; name="file"; filename="a;b.png"
            // Parameters are split on ';' only outside quotes, so a semicolon
            // inside a quoted filename doesn't truncate it.
            for param in split_outside_quotes(value, ';').into_iter().skip(1) {
                let Some((key, val)) = param.split_once('=') else {
                    continue;
                };
                let key = key.trim().to_ascii_lowercase();
                let val = val.trim().trim_matches('"').to_string();
                if key == "name" {
                    name = val;
                } else if key == "filename" {
                    filename = Some(val);
                }
            }
        }
    }
    (name, filename, content_type)
}

/// Split on `sep`, ignoring separators inside double quotes.
fn split_outside_quotes(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            cur.push(ch);
        } else if ch == sep && !in_quotes {
            out.push(cur.clone());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

/// Parse a `multipart/form-data` body. Pure; unit-tested.
///
/// Errors describe what was wrong with the body (they surface to the caller as
/// a 400), never a panic — a hostile body is expected input here.
pub fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<FormPart>, String> {
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let mut cursor = find_sub(body, delim, 0).ok_or_else(|| "boundary not found in body".to_string())?;
    let mut parts = Vec::new();
    loop {
        // Position just past the delimiter that opens this segment.
        let after_delim = cursor + delim.len();
        if body.len() >= after_delim + 2 && &body[after_delim..after_delim + 2] == b"--" {
            return Ok(parts); // closing delimiter `--boundary--`
        }
        // The segment runs to the next delimiter.
        let Some(next) = find_sub(body, delim, after_delim) else {
            return Err("unterminated multipart body (no closing boundary)".to_string());
        };
        // Segment = CRLF + headers + CRLFCRLF + data + CRLF (before delimiter).
        let seg_start = if body.len() > after_delim + 1 && &body[after_delim..after_delim + 2] == b"\r\n" {
            after_delim + 2
        } else {
            after_delim
        };
        let seg = &body[seg_start..next];
        let split = find_sub(seg, b"\r\n\r\n", 0)
            .ok_or_else(|| "multipart part has no header/body separator".to_string())?;
        let (name, filename, content_type) = parse_part_headers(&seg[..split]);
        let mut data = &seg[split + 4..];
        if data.ends_with(b"\r\n") {
            data = &data[..data.len() - 2];
        }
        parts.push(FormPart { name, filename, content_type, data: data.to_vec() });
        cursor = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(boundary: &str, file_bytes: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        v.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\n",
        );
        v.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
        v.extend_from_slice(file_bytes);
        v.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        v.extend_from_slice(b"Content-Disposition: form-data; name=\"purpose\"\r\n\r\n");
        v.extend_from_slice(b"user_data");
        v.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        v
    }

    #[test]
    fn boundary_parsing_accepts_quoted_and_bare() {
        assert_eq!(
            boundary_from_content_type("multipart/form-data; boundary=abc123").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            boundary_from_content_type("Multipart/Form-Data; Boundary=\"a-b_c\"").as_deref(),
            Some("a-b_c")
        );
        assert_eq!(boundary_from_content_type("application/json"), None);
        assert_eq!(boundary_from_content_type("multipart/form-data"), None);
    }

    #[test]
    fn parses_file_and_scalar_fields() {
        // Binary payload that itself contains CRLF — the split must be
        // delimiter-driven, not newline-driven.
        let file_bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 255];
        let raw = body("X-Bound-42", &file_bytes);
        let parts = parse_multipart(&raw, "X-Bound-42").expect("parse");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "file");
        assert_eq!(parts[0].filename.as_deref(), Some("shot.png"));
        assert_eq!(parts[0].content_type.as_deref(), Some("image/png"));
        assert_eq!(parts[0].data, file_bytes);
        assert_eq!(parts[1].name, "purpose");
        assert_eq!(parts[1].filename, None);
        assert_eq!(parts[1].text(), "user_data");
    }

    #[test]
    fn quoted_filename_keeps_its_semicolon() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"--b\r\n");
        raw.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"we;ird.txt\"\r\n\r\n",
        );
        raw.extend_from_slice(b"hi\r\n--b--\r\n");
        let parts = parse_multipart(&raw, "b").expect("parse");
        assert_eq!(parts[0].filename.as_deref(), Some("we;ird.txt"));
        assert_eq!(parts[0].data, b"hi");
    }

    #[test]
    fn missing_closing_boundary_is_an_error() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"--b\r\n");
        raw.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"\r\n\r\n");
        raw.extend_from_slice(b"hi\r\n");
        assert!(parse_multipart(&raw, "b").is_err());
    }

    #[test]
    fn wrong_boundary_is_an_error() {
        let raw = body("real", b"x");
        assert!(parse_multipart(&raw, "other").is_err());
    }

    #[test]
    fn part_without_header_separator_is_an_error() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"--b\r\n");
        raw.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"\r\n");
        raw.extend_from_slice(b"--b--\r\n");
        assert!(parse_multipart(&raw, "b").is_err());
    }

    #[test]
    fn empty_body_is_an_error_not_a_panic() {
        assert!(parse_multipart(b"", "b").is_err());
    }
}
