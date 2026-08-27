//! Attachments: ACP content blocks in, resource links out.
//!
//! # In
//!
//! ACP carries files **inline in the prompt** — there is no upload step and no
//! REST file API in the protocol. An `image` block is base64 in the JSON-RPC
//! frame itself. Fleet's agents read attachments off disk, so an inbound block
//! has to be materialised into the workspace before the prompt is handed over.
//!
//! That is the opposite of the Responses surface, where the caller uploaded via
//! `POST /v1/files` first and referenced a `file_id` — hence a separate module
//! rather than a reuse of that path.
//!
//! # Out
//!
//! ACP has no "list artifacts" call either. A file the agent produced is
//! surfaced as a `resource_link` content block pointing at the retained HTTP
//! file surface, which is the shape `resource_link` exists for. When no public
//! base URL is configured there is nothing to link to, so links are simply not
//! emitted — a path in `tool_call.locations` still tells the client what
//! changed.

use std::path::PathBuf;

use base64::Engine as _;

use super::types::ContentBlock;

/// Where inbound attachments land, relative to the workspace root.
///
/// Same directory the Responses uploads used, so the two surfaces cannot
/// disagree about what is a user-supplied file — which is what
/// `is_upload_rel`-style checks key off.
pub const UPLOADS_DIR: &str = ".fleet-uploads";

/// Reduce a caller-supplied name to a bare, safe basename.
///
/// Traversal (`../`, absolute paths) and the empty/dot names collapse to a
/// default, so a hostile `name` cannot escape the uploads directory.
pub fn sanitize_filename(raw: &str) -> String {
    let base = std::path::Path::new(raw)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return "upload.bin".to_string();
    }
    cleaned.to_string()
}

/// Filename extension for a MIME type, for naming a block that carries none.
fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => "bin",
    }
}

/// Name an inbound attachment. Pure; unit-tested.
///
/// `index` keeps two unnamed blocks in one prompt from colliding.
pub fn attachment_name(uri: Option<&str>, mime: &str, index: usize) -> String {
    match uri.and_then(|u| u.rsplit('/').next()).filter(|s| !s.is_empty()) {
        Some(from_uri) => sanitize_filename(from_uri),
        None => format!("attachment-{index}.{}", ext_for_mime(mime)),
    }
}

/// What went wrong ingesting an attachment.
#[derive(Debug, PartialEq, Eq)]
pub struct IngestError(pub String);

/// Materialise every attachment block in a prompt, returning their paths in
/// order.
///
/// Blocks that are not attachments are skipped. A malformed one is an error
/// rather than a silent drop: a caller whose image vanished but whose request
/// still succeeded has no way to tell the agent never saw it.
pub fn ingest(workspace: &str, blocks: &[ContentBlock]) -> Result<Vec<PathBuf>, IngestError> {
    let mut out = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let (data, mime, uri) = match block {
            ContentBlock::Image { data, mime_type, uri } => (data, mime_type, uri.as_deref()),
            ContentBlock::Audio { data, mime_type } => (data, mime_type, None),
            // A link to something the client holds. Fleet's agent runs in its
            // own container and cannot reach the client's filesystem, so this
            // is refused rather than silently ignored.
            ContentBlock::ResourceLink { uri, .. } => {
                return Err(IngestError(format!(
                    "resource_link {uri} cannot be read from this agent's container; \
                     send the bytes as an image or resource block instead"
                )))
            }
            _ => continue,
        };

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|e| IngestError(format!("attachment {i} is not valid base64: {e}")))?;

        let name = attachment_name(uri, mime, i);
        // Each attachment gets its own subdirectory so the caller's filename
        // survives verbatim — it shows up in the prompt, and `photo.png` reads
        // better than a hash — without one upload overwriting another.
        let dir = PathBuf::from(workspace).join(UPLOADS_DIR).join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir)
            .map_err(|e| IngestError(format!("cannot create upload dir: {e}")))?;
        let path = dir.join(&name);
        std::fs::write(&path, &bytes)
            .map_err(|e| IngestError(format!("cannot write {}: {e}", path.display())))?;
        out.push(path);
    }
    Ok(out)
}

/// Split attachment paths for the agent that will receive them.
///
/// Codex only sees an image through `exec -i`; Claude reads files off disk with
/// its own tools, so it gets paths listed in the prompt instead. Returns
/// `(listed_in_prompt, passed_as_images)`. Pure; unit-tested.
pub fn split_for_tool(tool: &str, paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    if tool != "codex" {
        return (paths.to_vec(), Vec::new());
    }
    let mut in_prompt = Vec::new();
    let mut images = Vec::new();
    for p in paths {
        if crate::wiki::mime_for_path(p).starts_with("image/") {
            images.push(p.to_string_lossy().into_owned());
        } else {
            in_prompt.push(p.clone());
        }
    }
    (in_prompt, images)
}

/// Append an attachment manifest to a prompt.
///
/// Paths, not bytes. The instruction is explicit ("read each") because a bare
/// path in a prompt is easy for a model to acknowledge without opening. Pure;
/// unit-tested.
pub fn prompt_with_attachments(prompt: &str, paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return prompt.to_string();
    }
    let list: Vec<String> =
        paths.iter().map(|p| format!("- {}", p.display())).collect();
    format!(
        "{prompt}\n\nAttached files (read each before answering):\n{}",
        list.join("\n")
    )
}

/// Public base URL for artifact links, e.g. `https://fleet.example.com`.
///
/// Unset means artifacts get no links. That is the honest default: ACP has no
/// artifact-listing call, so a link is only useful if it actually resolves from
/// wherever the client is, and only the deployment knows that.
pub fn public_base_url() -> Option<String> {
    std::env::var("FLEET_PUBLIC_BASE_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// A `resource_link` block for a workspace-relative artifact path.
///
/// `None` when no base URL is configured — see [`public_base_url`].
pub fn artifact_link(rel_path: &str, file_id: &str) -> Option<ContentBlock> {
    let base = public_base_url()?;
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path).to_string();
    Some(ContentBlock::ResourceLink {
        uri: format!("{base}/v1/files/{file_id}/content"),
        name,
        title: None,
        description: None,
        mime_type: Some(crate::wiki::mime_for_path(std::path::Path::new(rel_path)).to_string()),
        size: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitizing_defeats_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("/etc/shadow"), "shadow");
        assert_eq!(sanitize_filename(""), "upload.bin");
        assert_eq!(sanitize_filename(".."), "upload.bin");
        assert_eq!(sanitize_filename("."), "upload.bin");
        assert_eq!(sanitize_filename("photo.png"), "photo.png");
        // A name that is only separators must not survive as one.
        assert_eq!(sanitize_filename("///"), "upload.bin");
    }

    #[test]
    fn unnamed_attachments_are_named_by_mime_and_index() {
        assert_eq!(attachment_name(None, "image/png", 0), "attachment-0.png");
        assert_eq!(attachment_name(None, "image/jpeg", 3), "attachment-3.jpg");
        assert_eq!(attachment_name(None, "application/pdf", 1), "attachment-1.pdf");
        // An unknown type still gets a usable name rather than none.
        assert_eq!(attachment_name(None, "application/x-weird", 2), "attachment-2.bin");
        // Two unnamed blocks in one prompt must not collide.
        assert_ne!(
            attachment_name(None, "image/png", 0),
            attachment_name(None, "image/png", 1)
        );
    }

    #[test]
    fn a_uri_supplies_the_name_but_cannot_supply_a_path() {
        assert_eq!(attachment_name(Some("file:///tmp/photo.png"), "image/png", 0), "photo.png");
        assert_eq!(
            attachment_name(Some("https://x.test/a/b/../../etc/passwd"), "image/png", 0),
            "passwd"
        );
    }

    #[test]
    fn ingest_writes_each_attachment_and_keeps_prompt_order() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().into_owned();
        let png = base64::engine::general_purpose::STANDARD.encode([0x89, 0x50, 0x4E, 0x47]);

        let blocks = vec![
            ContentBlock::text("look at these"),
            ContentBlock::Image {
                data: png.clone(),
                mime_type: "image/png".into(),
                uri: Some("first.png".into()),
            },
            ContentBlock::Image { data: png, mime_type: "image/png".into(), uri: None },
        ];
        let paths = ingest(&ws, &blocks).unwrap();

        assert_eq!(paths.len(), 2, "text blocks are not attachments");
        assert_eq!(paths[0].file_name().unwrap(), "first.png");
        assert_eq!(paths[1].file_name().unwrap(), "attachment-2.png");
        for p in &paths {
            assert_eq!(std::fs::read(p).unwrap(), [0x89, 0x50, 0x4E, 0x47]);
            assert!(p.starts_with(dir.path().join(UPLOADS_DIR)), "must land under uploads");
        }
        // Separate subdirectories, so identical names cannot overwrite.
        assert_ne!(paths[0].parent(), paths[1].parent());
    }

    #[test]
    fn a_prompt_with_no_attachments_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().into_owned();
        assert!(ingest(&ws, &[ContentBlock::text("hi")]).unwrap().is_empty());
        assert!(!dir.path().join(UPLOADS_DIR).exists(), "no attachments, no directory");
    }

    #[test]
    fn bad_base64_is_an_error_not_a_silent_drop() {
        // A caller whose image vanished but whose request still succeeded has
        // no way to know the agent never saw it.
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().into_owned();
        let err = ingest(
            &ws,
            &[ContentBlock::Image {
                data: "not base64!!".into(),
                mime_type: "image/png".into(),
                uri: None,
            }],
        )
        .unwrap_err();
        assert!(err.0.contains("base64"), "{}", err.0);
    }

    #[test]
    fn a_resource_link_the_agent_cannot_reach_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_string_lossy().into_owned();
        let err = ingest(
            &ws,
            &[ContentBlock::ResourceLink {
                uri: "file:///Users/someone/secret.pdf".into(),
                name: "secret.pdf".into(),
                title: None,
                description: None,
                mime_type: None,
                size: None,
            }],
        )
        .unwrap_err();
        assert!(err.0.contains("cannot be read"), "{}", err.0);
    }

    #[test]
    fn attachments_split_per_agent_ingestion() {
        let paths =
            vec![PathBuf::from("/w/a.png"), PathBuf::from("/w/b.pdf")];
        // Claude reads files itself, so everything is listed in the prompt.
        let (in_prompt, images) = split_for_tool("claude", &paths);
        assert_eq!(in_prompt.len(), 2);
        assert!(images.is_empty());
        // Codex only sees an image through `exec -i`.
        let (in_prompt, images) = split_for_tool("codex", &paths);
        assert_eq!(in_prompt, vec![PathBuf::from("/w/b.pdf")]);
        assert_eq!(images, vec!["/w/a.png".to_string()]);
    }

    #[test]
    fn the_manifest_is_appended_only_when_there_is_something_to_list() {
        assert_eq!(prompt_with_attachments("do it", &[]), "do it");
        let got = prompt_with_attachments("do it", &[PathBuf::from("/w/a.png")]);
        assert!(got.starts_with("do it"));
        assert!(got.contains("/w/a.png"));
        assert!(got.contains("read each"), "a bare path is easy to acknowledge without opening");
    }

    #[test]
    fn artifact_links_need_a_configured_base_url() {
        let prev = std::env::var("FLEET_PUBLIC_BASE_URL").ok();

        std::env::remove_var("FLEET_PUBLIC_BASE_URL");
        assert!(
            artifact_link("out/report.pdf", "file_abc").is_none(),
            "with nowhere to point, a link would not resolve"
        );

        std::env::set_var("FLEET_PUBLIC_BASE_URL", "https://fleet.example.com/");
        match artifact_link("out/report.pdf", "file_abc").unwrap() {
            ContentBlock::ResourceLink { uri, name, mime_type, .. } => {
                // The trailing slash on the base must not double up.
                assert_eq!(uri, "https://fleet.example.com/v1/files/file_abc/content");
                assert_eq!(name, "report.pdf");
                assert_eq!(mime_type.as_deref(), Some("application/pdf"));
            }
            other => panic!("expected a resource_link, got {other:?}"),
        }

        match prev {
            Some(v) => std::env::set_var("FLEET_PUBLIC_BASE_URL", v),
            None => std::env::remove_var("FLEET_PUBLIC_BASE_URL"),
        }
    }
}
