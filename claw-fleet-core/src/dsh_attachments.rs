//! dsh's image attachments, in both directions.
//!
//! **Outbound** ([`prompt_content`]): the composers hand Fleet a prompt with a
//! trailing `Context files:` block ([`crate::context_files`]). Claude Code and
//! codex read those paths themselves with a file tool, so a path is a complete
//! handle for them. dsh's agent cannot: an image only reaches the model as an
//! `{type:"image"}` content part on `session.prompt`, which the host admits into
//! its durable attachment store. So the image paths are lifted out of the block
//! and encoded; everything else stays in the block as text.
//!
//! **Inbound** ([`image_block_url`]): the durable form dsh writes into a session
//! log is `{type:"image", attachment:{attachmentId:"sha256:…", …}}` — a
//! reference with no bytes. Fleet's transcript renderer needs something an
//! `<img>` can load, and it cannot be inline base64: the transport trims every
//! string leaf over 4 KiB ([`crate::message_trim`]), which would corrupt exactly
//! the payload it is meant to carry. It becomes a `fleet-attachment://` URL
//! instead, served out of the same content-addressed store the composer uploads
//! into ([`crate::user_attachments`]).
//!
//! That store reuse is not a coincidence to be maintained by hand: dsh's
//! `attachmentId` is the SHA-256 of the committed bytes and Fleet's store key is
//! the first 16 hex characters of the SHA-256 of the stored bytes, so the same
//! image has the same key on both sides. Verified live against dsh 0.1.1-rc.2: a
//! 209-byte PNG uploaded through `session.prompt` came back as
//! `attachmentId: "sha256:dae52f01…"`, byte-identical to `shasum -a 256` of the
//! file on disk.
//!
//! # Both backends, without a new endpoint
//!
//! Fleet's rule is that a feature works under `RemoteBackend` too. This one does
//! by construction, because every step already runs on whichever host the agent
//! is on:
//!
//! * **Outbound.** The paths in the block were minted by
//!   `Backend::upload_attachment`, which under `RemoteBackend` uploads the bytes
//!   to the probe and returns a *probe* path. [`prompt_content`] then runs inside
//!   that probe's `fleet serve`, next to the dsh it is talking to, so the file is
//!   local to the reader.
//! * **Inbound.** `get_messages` is a `fleet serve` route (`routes::MESSAGES`),
//!   so [`resolve_image_blocks`] commits to the *probe's* store and emits probe
//!   paths. The desktop renders them through the `fleet-attachment://` protocol,
//!   whose handler goes through `Backend::get_user_attachment` — proxied to the
//!   probe's `routes::USER_ATTACHMENT`. No local file access is assumed anywhere.
//! * **Mobile.** The relay is started by `LocalBackend`, so in a remote
//!   deployment it is the probe's own `fleet serve` that runs it — the store it
//!   thumbnails from is the same one dsh committed into.

use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};

/// Media types dsh's version-one attachment path accepts (`ImageMediaType`).
/// A file that sniffs as anything else stays a text path.
const ACCEPTED: [(&[u8], &str); 5] = [
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    // WebP is `RIFF....WEBP`; the 4-byte length in between is checked below.
    (b"RIFF", "image/webp"),
];

/// Per-image byte ceiling, and the count ceiling for one message. Both read off
/// the live `imageLimits` projection of dsh 0.1.1-rc.2 (`maxImageBytes`
/// 20971520, `maxImagesPerMessage` 20) rather than guessed.
///
/// Enforced here so an over-budget attachment degrades to the text path it
/// always was, instead of making the host refuse the whole prompt — the user's
/// prose must reach the model either way.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGES: usize = 20;

/// The media type these bytes really are, per dsh's accepted set.
///
/// Sniffed from content rather than trusted from the extension: dsh verifies the
/// declared `mediaType` against the decoded bytes and refuses a mismatch, so a
/// `.png` that is actually a JPEG would fail the whole prompt.
fn media_type_of(bytes: &[u8]) -> Option<&'static str> {
    for (magic, media_type) in ACCEPTED {
        if !bytes.starts_with(magic) {
            continue;
        }
        if media_type == "image/webp" {
            // `RIFF` alone is any RIFF container (WAV, AVI); the form type at
            // byte 8 is what makes it WebP.
            return (bytes.len() >= 12 && &bytes[8..12] == b"WEBP").then_some(media_type);
        }
        return Some(media_type);
    }
    None
}

/// One image content part, or `None` when this path is not an image dsh takes.
fn image_part(path: &str) -> Option<Value> {
    let path = Path::new(path);
    // Size first: a 2 GB video should not be read into memory to learn that it
    // is not a PNG.
    if std::fs::metadata(path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let media_type = media_type_of(&bytes)?;
    Some(json!({
        "type": "image",
        "mediaType": media_type,
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        // dsh never interprets this as a path, and records it stripped of local
        // path information — it is the display name the bubble shows.
        "name": path.file_name()?.to_string_lossy(),
    }))
}

/// Build `session.prompt`'s `content` from a composer prompt.
///
/// A prompt with no attachment block — every Fleet-spawned session's first
/// prompt, every handoff note — produces exactly the single text part this used
/// to send unconditionally.
pub fn prompt_content(prompt: &str) -> Vec<Value> {
    let (prose, paths) = crate::context_files::split(prompt);
    if paths.is_empty() {
        return vec![json!({ "type": "text", "text": prompt })];
    }

    let mut images = Vec::new();
    let mut kept = Vec::new();
    for path in paths {
        // Over the count ceiling the remainder degrades to text paths rather
        // than being dropped: the model can still be told which files were
        // meant, and dsh would refuse a 21st image outright.
        match (images.len() < MAX_IMAGES).then(|| image_part(path)).flatten() {
            Some(part) => images.push(part),
            None => kept.push(path),
        }
    }
    if images.is_empty() {
        // Nothing was liftable, so the prompt is unchanged — including its
        // block, byte for byte.
        return vec![json!({ "type": "text", "text": prompt })];
    }

    let text = crate::context_files::render(prose, &kept);
    let mut content = Vec::with_capacity(images.len() + 1);
    // An attachment-only prompt (a bare screenshot, no prose) carries no text
    // part at all rather than an empty one.
    if !text.trim().is_empty() {
        content.push(json!({ "type": "text", "text": text }));
    }
    content.extend(images);
    content
}

/// The `fleet-attachment://` path segment for one dsh image block, as
/// `<key>/<name>`, or `None` when the block is not a resolvable image reference.
///
/// Only derives the key — it does not fetch bytes. A key whose directory is
/// absent from the store yields a URL that the protocol handler answers with a
/// miss, which the frontend renders as a broken-image affordance; see
/// [`crate::dsh_source`] for the fetch that fills the store first.
pub fn image_block_key(block: &Value) -> Option<(String, String)> {
    let attachment = block.get("attachment")?;
    let id = attachment.get("attachmentId")?.as_str()?;
    let digest = id.strip_prefix("sha256:")?;
    // The store key is the first 16 hex chars of the same digest
    // (`user_attachments::content_key`). Anything shorter is not one.
    if digest.len() < KEY_HEX_LEN || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let key = digest[..KEY_HEX_LEN].to_ascii_lowercase();
    let name = attachment
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(sanitize_name)
        .unwrap_or_else(|| default_name(attachment));
    Some((key, name))
}

/// Length of [`crate::user_attachments`]'s content key in hex characters.
const KEY_HEX_LEN: usize = 16;

/// Rewrite every dsh image block in `records` into the store-path form the
/// transcript renderer knows how to display, filling the store first for any
/// image it does not already hold.
///
/// `fetch` reads one attachment's bytes by id — `session.attachment` in
/// production, which proves the session's log references that id before
/// answering. It is a parameter so this orchestration is testable without a live
/// server, the way `dsh_source::history_with` takes its own fetch.
///
/// A fetch that fails leaves the block rewritten anyway: the renderer's
/// broken-image affordance is retryable and says so, whereas an unrewritten
/// block renders as an anonymous unknown-block card that no user can act on.
pub fn resolve_image_blocks<F>(records: &mut [Value], fetch: F)
where
    F: Fn(&str) -> Option<Vec<u8>>,
{
    for record in records {
        let Some(content) = record
            .get_mut("message")
            .and_then(|m| m.get_mut("content"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("image") {
                continue;
            }
            let Some((key, name)) = image_block_key(block) else {
                continue;
            };
            let Some(path) = crate::user_attachments::stored_path(&key, &name) else {
                continue;
            };
            if !crate::user_attachments::exists_in_store(&path) {
                // Only reachable for an image this Fleet never uploaded — one
                // sent from dsh's own web UI, or a session resumed on another
                // machine. Fleet's own sends already put the bytes in the store
                // under this very key, since both sides key on the same digest.
                if let Some(id) = block
                    .get("attachment")
                    .and_then(|a| a.get("attachmentId"))
                    .and_then(Value::as_str)
                {
                    if let Some(bytes) = fetch(id) {
                        let _ = crate::user_attachments::ingest_bytes(&bytes, &name);
                    }
                }
            }
            let media_type = block
                .get("attachment")
                .and_then(|a| a.get("mediaType"))
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            *block = json!({
                "type": "image",
                // `path` rather than `base64`: the transport trims every string
                // leaf over 4 KiB (`message_trim`), which would corrupt an
                // inline payload into something no `<img>` can decode. A store
                // path is also what the composer's own attachments leave in a
                // transcript, so both ends of a dsh conversation render through
                // one path.
                "source": {
                    "type": "path",
                    "media_type": media_type,
                    "path": path.to_string_lossy(),
                },
            });
        }
    }
}

/// Keep a stored name to a bare filename, the only form the store serves.
fn sanitize_name(name: &str) -> String {
    Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| s != "." && s != "..")
        .unwrap_or_else(|| "image.png".to_string())
}

/// Name for an attachment dsh recorded without one — an image pasted into dsh's
/// own web UI, which has no filename to carry. The extension follows the media
/// type so the store's mime lookup still answers correctly.
fn default_name(attachment: &Value) -> String {
    let ext = match attachment.get("mediaType").and_then(Value::as_str) {
        Some("image/jpeg") => "jpg",
        Some("image/webp") => "webp",
        Some("image/gif") => "gif",
        _ => "png",
    };
    format!("image.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest as _;

    /// Minimal byte strings that sniff as each accepted type.
    fn png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\nrest".to_vec()
    }
    fn webp() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(b"WEBPrest");
        v
    }

    #[test]
    fn sniffs_the_four_accepted_media_types() {
        assert_eq!(media_type_of(&png()), Some("image/png"));
        assert_eq!(media_type_of(b"\xff\xd8\xffrest"), Some("image/jpeg"));
        assert_eq!(media_type_of(b"GIF89arest"), Some("image/gif"));
        assert_eq!(media_type_of(b"GIF87arest"), Some("image/gif"));
        assert_eq!(media_type_of(&webp()), Some("image/webp"));
    }

    /// An SVG is an image to a human and to `mime_for_path`, but not to dsh's
    /// version-one admission — declaring it would fail the prompt.
    #[test]
    fn rejects_types_dsh_does_not_admit() {
        assert_eq!(media_type_of(b"<svg xmlns=\"...\"></svg>"), None);
        assert_eq!(media_type_of(b"%PDF-1.7"), None);
        assert_eq!(media_type_of(b""), None);
    }

    /// A RIFF container that is not WebP (a WAV file) must not be declared one.
    #[test]
    fn rejects_a_riff_container_that_is_not_webp() {
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVEfmt ");
        assert_eq!(media_type_of(&wav), None);
        // Truncated right after the magic: not enough bytes to prove WebP.
        assert_eq!(media_type_of(b"RIFF"), None);
    }

    #[test]
    fn a_prompt_without_a_block_is_one_text_part() {
        let content = prompt_content("跑一下测试");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "跑一下测试");
    }

    #[test]
    fn image_paths_become_image_parts_and_leave_the_block() {
        let dir = std::env::temp_dir().join(format!("fleet-dsh-att-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("shot.png");
        std::fs::write(&shot, png()).unwrap();

        let prompt = format!("按这个改\n\nContext files:\n- {}", shot.display());
        let content = prompt_content(&prompt);

        assert_eq!(content.len(), 2, "one text part and one image part");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(
            content[0]["text"], "按这个改",
            "the lifted path must not stay in the text"
        );
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mediaType"], "image/png");
        assert_eq!(content[1]["name"], "shot.png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(content[1]["data"].as_str().unwrap())
                .unwrap(),
            png(),
            "the part must carry the file's exact bytes"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A mixed block keeps its non-image members as text: a PDF path is still
    /// the only handle dsh's file tools have on that file.
    #[test]
    fn non_image_paths_stay_in_the_text_block() {
        let dir = std::env::temp_dir().join(format!("fleet-dsh-mix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("shot.png");
        std::fs::write(&shot, png()).unwrap();
        let doc = dir.join("spec.pdf");
        std::fs::write(&doc, b"%PDF-1.7").unwrap();

        let prompt = format!(
            "看图和文档\n\nContext files:\n- {}\n- {}",
            shot.display(),
            doc.display()
        );
        let content = prompt_content(&prompt);

        assert_eq!(content.len(), 2);
        assert_eq!(
            content[0]["text"],
            format!("看图和文档\n\nContext files:\n- {}", doc.display()),
            "the PDF keeps its path; only the image left the block"
        );
        assert_eq!(content[1]["type"], "image");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A path that cannot be read at all — deleted since the composer listed
    /// it, or on the wrong machine — must not silently drop the user's prose.
    #[test]
    fn an_unreadable_path_leaves_the_prompt_untouched() {
        let prompt = "看下这个\n\nContext files:\n- /nope/gone.png";
        let content = prompt_content(prompt);
        assert_eq!(content.len(), 1);
        assert_eq!(
            content[0]["text"], prompt,
            "with nothing liftable the prompt is sent verbatim, block included"
        );
    }

    /// Prose-free attachment prompt: dropping the empty text part keeps the
    /// turn from opening with a blank user bubble.
    #[test]
    fn an_attachment_only_prompt_carries_no_text_part() {
        let dir = std::env::temp_dir().join(format!("fleet-dsh-bare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("bare.png");
        std::fs::write(&shot, png()).unwrap();

        let content = prompt_content(&format!("\n\nContext files:\n- {}", shot.display()));
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "image");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_oversized_image_degrades_to_its_text_path() {
        let dir = std::env::temp_dir().join(format!("fleet-dsh-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.png");
        let mut bytes = png();
        bytes.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        std::fs::write(&big, &bytes).unwrap();

        let prompt = format!("这张\n\nContext files:\n- {}", big.display());
        let content = prompt_content(&prompt);
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], prompt);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn image_block_key_derives_the_store_key_from_the_digest() {
        let block = json!({
            "type": "image",
            "attachment": {
                "attachmentId": "sha256:dae52f012e32522b25a27af12d128af9ecc990a0581225b2d8f2b817ab574cf7",
                "mediaType": "image/png",
                "name": "probe.png",
                "bytes": 209,
                "width": 96,
                "height": 96,
            },
        });
        assert_eq!(
            image_block_key(&block),
            Some(("dae52f012e32522b".to_string(), "probe.png".to_string()))
        );
    }

    /// dsh's own web UI commits pasted images with no name; the key still
    /// resolves and the name follows the media type.
    #[test]
    fn image_block_key_names_an_unnamed_attachment_by_media_type() {
        let block = json!({
            "type": "image",
            "attachment": {
                "attachmentId": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "mediaType": "image/jpeg",
            },
        });
        assert_eq!(
            image_block_key(&block),
            Some(("0123456789abcdef".to_string(), "image.jpg".to_string()))
        );
    }

    /// A name carrying path separators must not escape the store directory.
    #[test]
    fn image_block_key_strips_path_information_from_the_name() {
        let block = json!({
            "attachment": {
                "attachmentId": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "name": "../../etc/passwd",
            },
        });
        let (_, name) = image_block_key(&block).unwrap();
        assert_eq!(name, "passwd");
    }

    /// The digest dsh reports is the digest of the bytes Fleet uploaded, so an
    /// image Fleet sent is already in the store under the key derived from that
    /// digest — the resolver rewrites it without a single fetch.
    #[test]
    fn resolve_rewrites_a_stored_image_without_fetching() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!("fleet-dsh-res-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized via fleet_home_lock
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let bytes = png();
        let stored = crate::user_attachments::ingest_bytes(&bytes, "shot.png").unwrap();
        let digest: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let mut records = vec![json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    { "type": "text", "text": "按这个改" },
                    { "type": "image", "attachment": {
                        "attachmentId": format!("sha256:{digest}"),
                        "mediaType": "image/png",
                        "name": "shot.png",
                    }},
                ],
            },
        })];

        let fetched = std::cell::Cell::new(0);
        resolve_image_blocks(&mut records, |_| {
            fetched.set(fetched.get() + 1);
            None
        });
        assert_eq!(fetched.get(), 0, "the store already had it");

        let block = &records[0]["message"]["content"][1];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "path");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["path"], stored.to_string_lossy().as_ref());
        assert_eq!(
            records[0]["message"]["content"][0]["text"], "按这个改",
            "the text block must be left alone"
        );

        // SAFETY: serialized via fleet_home_lock
        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// An image Fleet never uploaded — sent from dsh's own web UI — is fetched
    /// once, committed to the store, and then rendered from there.
    #[test]
    fn resolve_fetches_an_image_the_store_is_missing() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!("fleet-dsh-miss-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized via fleet_home_lock
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let bytes = png();
        let digest: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut records = vec![json!({
            "message": { "content": [{ "type": "image", "attachment": {
                "attachmentId": format!("sha256:{digest}"),
                "mediaType": "image/png",
            }}]},
        })];

        let asked = std::cell::RefCell::new(Vec::new());
        resolve_image_blocks(&mut records, |id| {
            asked.borrow_mut().push(id.to_string());
            Some(bytes.clone())
        });
        assert_eq!(asked.borrow().len(), 1, "one fetch for the missing image");
        assert_eq!(asked.borrow()[0], format!("sha256:{digest}"));

        let path = records[0]["message"]["content"][0]["source"]["path"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "the fetched bytes must be readable at the rendered path"
        );

        // SAFETY: serialized via fleet_home_lock
        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A fetch that cannot answer still yields a renderable block: the frontend
    /// shows a retryable broken-image affordance rather than an unknown card.
    #[test]
    fn resolve_rewrites_even_when_the_fetch_fails() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!("fleet-dsh-fail-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized via fleet_home_lock
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let mut records = vec![json!({
            "message": { "content": [{ "type": "image", "attachment": {
                "attachmentId": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "mediaType": "image/webp",
            }}]},
        })];
        resolve_image_blocks(&mut records, |_| None);
        let source = &records[0]["message"]["content"][0]["source"];
        assert_eq!(source["type"], "path");
        assert_eq!(source["media_type"], "image/webp");
        assert!(source["path"].as_str().unwrap().ends_with("image.webp"));

        // SAFETY: serialized via fleet_home_lock
        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Records with no image content must come back byte-identical: this pass
    /// runs over every history read, so it has to be a no-op for the 99% case.
    #[test]
    fn resolve_leaves_non_image_records_untouched() {
        let before = vec![
            json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "hi" }] } }),
            json!({ "type": "assistant", "message": { "content": [
                { "type": "thinking", "thinking": "…" },
                { "type": "tool_use", "name": "Bash", "input": {} },
            ]}}),
            json!({ "type": "system", "subtype": "init" }),
        ];
        let mut after = before.clone();
        resolve_image_blocks(&mut after, |_| None);
        assert_eq!(after, before);
    }

    #[test]
    fn image_block_key_refuses_a_block_that_is_not_a_sha256_reference() {
        assert_eq!(image_block_key(&json!({ "type": "text", "text": "hi" })), None);
        assert_eq!(
            image_block_key(&json!({ "attachment": { "attachmentId": "opaque-id" } })),
            None
        );
        assert_eq!(
            image_block_key(&json!({ "attachment": { "attachmentId": "sha256:short" } })),
            None
        );
        assert_eq!(
            image_block_key(&json!({ "attachment": { "attachmentId": "sha256:zzzzzzzzzzzzzzzzzz" } })),
            None,
            "a non-hex digest is not a content key"
        );
    }
}
