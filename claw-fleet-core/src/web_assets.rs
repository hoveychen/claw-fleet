//! Serving the web UI's `vite build` output over HTTP.
//!
//! Two servers need this and they get their files from different places:
//!
//! - the app's in-process front door (`claw-fleet-desktop/src/web_serve.rs`)
//!   resolves through Tauri's own `AssetResolver`, i.e. the very bundle the
//!   desktop webview loads — nothing is embedded twice;
//! - `fleet serve` (including the cloud container) reads a directory that the
//!   image ships.
//!
//! So the *source* is a closure supplied by the caller and only the response
//! shaping lives here. Keeping that one copy is the point: the two servers must
//! agree on how `/` maps to `index.html` and on what a miss looks like, or the
//! same bundle behaves differently depending on who served it.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use tiny_http::{Header, Response};

/// One frontend file.
pub struct StaticAsset {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Resolves a request path (leading slash included) to a frontend file.
pub type AssetSource = Arc<dyn Fn(&str) -> Option<StaticAsset> + Send + Sync>;

/// An [`AssetSource`] backed by a directory on disk.
///
/// Used by `fleet serve` and by the desktop's `web_preview` example. Rejects
/// traversal before touching the filesystem: the path arrives straight off the
/// wire, and `root.join("../../etc/passwd")` would otherwise escape the bundle.
pub fn from_dir(root: PathBuf) -> AssetSource {
    Arc::new(move |path: &str| {
        let rel = path.trim_start_matches('/');
        if rel.is_empty() || rel.split('/').any(|seg| seg == ".." || seg == ".") {
            return None;
        }
        // An absolute or drive-qualified component would also escape `join`.
        let candidate = PathBuf::from(rel);
        if candidate.is_absolute() {
            return None;
        }
        let file = root.join(&candidate);
        std::fs::read(&file).ok().map(|bytes| StaticAsset {
            bytes,
            mime: crate::wiki::mime_for_path(&file).to_string(),
        })
    })
}

/// Map a request path onto the bundle and build its response.
///
/// `/` resolves to `/index.html`. A path the bundle doesn't have is a 404
/// rather than a fallback to index: a mistyped asset should stay visibly
/// missing instead of quietly returning HTML that the caller then fails to
/// parse.
pub fn respond(assets: &AssetSource, path: &str) -> Response<Cursor<Vec<u8>>> {
    let wanted = if path == "/" { "/index.html" } else { path };
    match assets(wanted) {
        Some(asset) => {
            let header: Header = format!("Content-Type: {}", asset.mime)
                .parse()
                .unwrap_or_else(|_| {
                    "Content-Type: application/octet-stream".parse().unwrap()
                });
            Response::from_data(asset.bytes).with_header(header)
        }
        None => Response::from_data(b"no such file".to_vec()).with_status_code(404),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"<html>board</html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.js"), b"console.log(1)").unwrap();
        dir
    }

    #[test]
    fn root_resolves_to_index() {
        let dir = fixture_dir();
        let assets = from_dir(dir.path().to_path_buf());
        let hit = assets("/index.html").expect("index must resolve");
        assert_eq!(hit.bytes, b"<html>board</html>");
        assert!(hit.mime.starts_with("text/html"));
    }

    #[test]
    fn nested_paths_keep_their_mime() {
        let dir = fixture_dir();
        let assets = from_dir(dir.path().to_path_buf());
        let js = assets("/assets/app.js").expect("nested asset must resolve");
        assert!(js.mime.starts_with("text/javascript"), "got {}", js.mime);
    }

    /// The path comes straight off the wire, so escaping the bundle root must
    /// be impossible rather than merely unlikely.
    #[test]
    fn traversal_cannot_escape_the_bundle() {
        let dir = fixture_dir();
        let secret = dir.path().parent().unwrap().join("outside.txt");
        std::fs::write(&secret, b"secret").unwrap();
        let assets = from_dir(dir.path().to_path_buf());

        for attempt in [
            "/../outside.txt",
            "/assets/../../outside.txt",
            "/./../outside.txt",
            "//../outside.txt",
        ] {
            assert!(
                assets(attempt).is_none(),
                "traversal must be refused: {attempt}"
            );
        }
        let _ = std::fs::remove_file(&secret);
    }

    #[test]
    fn missing_file_is_a_404_not_an_index_fallback() {
        let dir = fixture_dir();
        let assets = from_dir(dir.path().to_path_buf());
        assert!(assets("/assets/nope.js").is_none());
        assert_eq!(respond(&assets, "/assets/nope.js").status_code().0, 404);
    }

    #[test]
    fn respond_maps_slash_to_index() {
        let dir = fixture_dir();
        let assets = from_dir(dir.path().to_path_buf());
        assert_eq!(respond(&assets, "/").status_code().0, 200);
    }
}
