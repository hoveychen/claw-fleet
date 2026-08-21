//! Persistent store for *user-direction* attachments — files the user hands to
//! the agent: images pasted into a composer, files picked in the decision panel.
//!
//! The mirror image of the decision-asset store in [`crate::mcp_ipc`], which
//! covers the agent → user direction. Both exist for the same reason: the path
//! that lands in a transcript (`Context files:\n- <path>`) or a decision record
//! (`@<path>`) is the *only* handle history has on that file. It must still
//! resolve when the conversation is read back weeks later.
//!
//! Staging to `$TMPDIR` fails that on two counts: the OS reclaims the directory
//! on its own schedule, and under `RemoteBackend` a desktop-side temp path names
//! a file on the wrong machine entirely — the agent never could read it.
//!
//! Layout: `~/.fleet/user-attachments/<sha256[..16]>/<name>`. Content-addressed,
//! so the same screenshot pasted into five sessions costs one copy on disk.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::mcp_ipc::DecisionAssetBytes;

/// `~/.fleet/user-attachments` (None when the home dir can't be determined).
pub fn user_attachments_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("user-attachments"))
}

/// Whether `path` points at a file that still lives inside the user-attachments
/// store. The mobile composer persists attachment chips across reloads and calls
/// this to prune ones whose backing file has since been cleared.
///
/// Deliberately NOT a general filesystem-existence oracle: the path is
/// canonicalized (resolving `..`/symlinks, failing when it doesn't exist) and
/// must resolve to a file under the canonicalized store root, so a client can
/// only ever confirm files it legitimately uploaded — never probe arbitrary
/// paths on the host.
pub fn exists_in_store(path: &Path) -> bool {
    let Some(base) = user_attachments_dir() else {
        return false;
    };
    let Ok(base) = base.canonicalize() else {
        return false; // store dir absent ⇒ nothing can exist in it
    };
    let Ok(canon) = path.canonicalize() else {
        return false; // missing path ⇒ canonicalize fails
    };
    canon.starts_with(&base) && canon.is_file()
}

/// The path one `<key>/<name>` pair occupies in the store, whether or not the
/// file is there yet.
///
/// The layout is this module's business, so callers that hold store coordinates
/// rather than a path — the dsh transcript renderer derives them from an
/// attachment digest ([`crate::dsh_attachments`]) — ask here instead of joining
/// `.fleet/user-attachments` themselves.
pub fn stored_path(key: &str, name: &str) -> Option<PathBuf> {
    if key.is_empty() || key.contains('/') || key.contains('\\') || key.contains("..") {
        return None;
    }
    if !valid_name(name) {
        return None;
    }
    user_attachments_dir().map(|base| base.join(key).join(name))
}

/// Reject anything that isn't a bare filename (path separators, `..`, absolute).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !Path::new(name).is_absolute()
}

/// Strip a source filename down to something safe to use as the stored `name`.
/// Falls back to a generic name when the input has nothing usable — an
/// attachment whose name we can't represent is still worth storing.
fn sanitize_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if valid_name(&base) {
        base
    } else {
        "attachment.bin".to_string()
    }
}

/// Copy `source` into the content-addressed store and return its persistent
/// absolute path — the path callers splice into a prompt or an `@mention`.
///
/// Re-ingesting identical bytes is a no-op that returns the same path, so the
/// caller needn't track whether it has seen this file before.
pub fn ingest(source: &Path, name: &str) -> Result<PathBuf, String> {
    let bytes = fs::read(source).map_err(|e| format!("read attachment: {e}"))?;
    ingest_bytes(&bytes, name)
}

/// [`ingest`] for callers that already hold the bytes (a clipboard paste never
/// touches disk before this point).
pub fn ingest_bytes(bytes: &[u8], name: &str) -> Result<PathBuf, String> {
    if bytes.len() as u64 > crate::backend::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment too large: {} bytes (max {})",
            bytes.len(),
            crate::backend::MAX_ATTACHMENT_BYTES
        ));
    }
    let base = user_attachments_dir().ok_or("cannot determine home dir")?;
    let key = content_key(bytes);
    let name = sanitize_name(name);

    let dir = base.join(&key);
    fs::create_dir_all(&dir).map_err(|e| format!("create user-attachment dir: {e}"))?;
    let dest = dir.join(&name);

    // Same key ⇒ same bytes, so an existing file is already correct. Skipping
    // the rewrite keeps a re-paste cheap and avoids touching a file another
    // session may be reading.
    if !dest.exists() {
        fs::write(&dest, bytes).map_err(|e| format!("write user-attachment: {e}"))?;
    }
    Ok(dest)
}

/// First 16 hex chars of the SHA-256 of the content. Short enough to keep the
/// path readable in a transcript, wide enough (64 bits) that an accidental
/// collision between two of one user's attachments is not a real concern.
fn content_key(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Reserved key for attachments that pre-date the store: Fleet used to stage
/// pastes into `$TMPDIR/fleet-pasted/` and write *that* path into the prompt, so
/// every transcript from before this change names a file there. They are still
/// on disk until the OS reaps them, so history can show them meanwhile — which
/// is the whole point, since the transcripts a user goes back and reads are
/// exactly the old ones.
///
/// Not a hazard the store isn't: one fixed directory, bare filenames only.
pub const LEGACY_PASTED_KEY: &str = "_pasted";

/// Read one file back out of the store. Backs the `fleet-attachment://`
/// protocol, so it carries the same path-traversal defense as
/// [`crate::mcp_ipc::read_decision_asset`].
pub fn read_user_attachment(key: &str, name: &str) -> Result<DecisionAssetBytes, String> {
    if key == LEGACY_PASTED_KEY {
        return read_legacy_pasted(name);
    }
    let base = user_attachments_dir().ok_or("cannot determine home dir")?;
    if key.is_empty() || key.contains('/') || key.contains('\\') || key.contains("..") {
        return Err("invalid user-attachment key".to_string());
    }
    if !valid_name(name) {
        return Err(format!("invalid attachment name '{name}'"));
    }

    let dir = base.join(key);
    let joined = dir.join(name);
    let canon_dir = dir
        .canonicalize()
        .map_err(|_| "user-attachment dir not found".to_string())?;
    let canon_file = joined
        .canonicalize()
        .map_err(|_| format!("attachment '{name}' not found"))?;
    if !canon_file.starts_with(&canon_dir) {
        return Err(format!("invalid path '{name}'"));
    }

    let bytes = fs::read(&canon_file).map_err(|e| format!("read '{name}': {e}"))?;
    Ok(DecisionAssetBytes {
        bytes,
        mime: crate::wiki::mime_for_path(&canon_file).to_string(),
    })
}

/// Serve a pre-store paste out of `$TMPDIR/fleet-pasted/`. Bare filenames only,
/// so this reads nothing outside that one directory.
fn read_legacy_pasted(name: &str) -> Result<DecisionAssetBytes, String> {
    if !valid_name(name) {
        return Err(format!("invalid attachment name '{name}'"));
    }
    let dir = std::env::temp_dir().join("fleet-pasted");
    let joined = dir.join(name);
    let canon_dir = dir
        .canonicalize()
        .map_err(|_| "legacy paste dir not found".to_string())?;
    let canon_file = joined
        .canonicalize()
        .map_err(|_| format!("attachment '{name}' not found"))?;
    if !canon_file.starts_with(&canon_dir) {
        return Err(format!("invalid path '{name}'"));
    }
    let bytes = fs::read(&canon_file).map_err(|e| format!("read '{name}': {e}"))?;
    Ok(DecisionAssetBytes {
        bytes,
        mime: crate::wiki::mime_for_path(&canon_file).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("fleet-user-att-test-{tag}-{nanos}"))
    }

    #[test]
    fn ingest_dedups_serves_and_defends() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = fresh_tmp_dir("store");
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized via fleet_home_lock
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let png = b"\x89PNG\r\n\x1a\nFAKE".to_vec();

        // Ingesting bytes yields a path under the store, and reading it back
        // through the protocol entry point returns the same bytes + a mime.
        let p1 = ingest_bytes(&png, "shot.png").unwrap();
        assert!(p1.starts_with(tmp.join(".fleet").join("user-attachments")));
        let key = p1.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();

        let got = read_user_attachment(&key, "shot.png").unwrap();
        assert_eq!(got.bytes, png);
        assert_eq!(got.mime, "image/png");

        // Content-addressed: identical bytes re-ingest to the identical path.
        let p2 = ingest_bytes(&png, "shot.png").unwrap();
        assert_eq!(p1, p2, "same content must not be stored twice");

        // Different content lands in a different bucket.
        let p3 = ingest_bytes(b"other", "shot.png").unwrap();
        assert_ne!(p1.parent(), p3.parent());

        // ingest() from a file on disk agrees with ingest_bytes().
        let src = tmp.join("orig.png");
        std::fs::write(&src, &png).unwrap();
        assert_eq!(ingest(&src, "shot.png").unwrap(), p1);

        // A source name carrying a directory is reduced to its basename rather
        // than escaping the bucket.
        let p4 = ingest_bytes(b"nested", "../../evil.png").unwrap();
        assert_eq!(p4.file_name().unwrap(), "evil.png");

        // Path traversal is rejected on both the key and the name.
        assert!(read_user_attachment(&key, "../../etc/passwd").is_err());
        assert!(read_user_attachment("../x", "shot.png").is_err());
        assert!(read_user_attachment(&key, "missing.png").is_err());

        // Oversize is refused rather than silently truncated.
        let huge = vec![0u8; (crate::backend::MAX_ATTACHMENT_BYTES + 1) as usize];
        assert!(ingest_bytes(&huge, "big.bin").is_err());

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn exists_in_store_confirms_only_present_in_store_files() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = fresh_tmp_dir("exists");
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized via fleet_home_lock
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        // An uploaded file is confirmed…
        let stored = ingest_bytes(b"present", "here.png").unwrap();
        assert!(exists_in_store(&stored));

        // …a same-shaped path with no backing file is not…
        let ghost = stored.parent().unwrap().parent().unwrap().join("deadbeef").join("gone.png");
        assert!(!exists_in_store(&ghost));

        // …and an out-of-store path is never confirmed, even if it exists on disk.
        let outside = tmp.join("outside.txt");
        std::fs::write(&outside, b"x").unwrap();
        assert!(!exists_in_store(&outside));

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn legacy_key_serves_pre_store_pastes() {
        // Transcripts written before the store exists name files in
        // $TMPDIR/fleet-pasted/. Those are the histories a user actually goes
        // back to read, so they have to render while the files survive.
        let dir = std::env::temp_dir().join("fleet-pasted");
        std::fs::create_dir_all(&dir).unwrap();
        let name = "paste-legacy-test-0001.png";
        let src = dir.join(name);
        std::fs::write(&src, b"\x89PNG\r\n\x1a\nOLD").unwrap();

        let got = read_user_attachment(LEGACY_PASTED_KEY, name).unwrap();
        assert_eq!(got.bytes, b"\x89PNG\r\n\x1a\nOLD");
        assert_eq!(got.mime, "image/png");

        // The escape hatch is still just one directory of bare filenames.
        assert!(read_user_attachment(LEGACY_PASTED_KEY, "../../etc/passwd").is_err());
        assert!(read_user_attachment(LEGACY_PASTED_KEY, "nope.png").is_err());

        let _ = std::fs::remove_file(&src);
    }
}
