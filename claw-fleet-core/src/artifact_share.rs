//! Share links for artifacts — a URL that hands *one* deliverable to someone
//! who has no Fleet account and no token of their own.
//!
//! The scope is deliberately narrow: these links are served by the local
//! `fleet serve` / `fleet webui` port, so they reach whoever can reach this
//! machine — you on your phone, a colleague on the same LAN. They are **not**
//! internet-reachable, and nothing here uploads anything anywhere. That was
//! 老板's explicit choice over the cloud-hosted alternative; a deliverable
//! that leaves the machine is a decision with different consequences and needs
//! its own.
//!
//! ```text
//! ~/.fleet/artifact-shares.json   # [ShareLink, …]
//! ```
//!
//! One file, unlike the artifact store's per-artifact `meta.json`: share links
//! are only ever created by a person clicking 分享, never by concurrent
//! agents, so there is nothing to contend on. Same reasoning as
//! `artifacts::Folder`.
//!
//! ## What a token grants
//!
//! Exactly one artifact's bytes, at exactly one version, until it expires or
//! is revoked. Nothing else — not the artifact list, not another artifact,
//! not any other Fleet route. The serving route therefore does **not** reuse
//! the management auth path: bearing a share token must never be mistaken for
//! being the operator (see `hooks_server::routes_artifacts`).
//!
//! ## Why the version is pinned
//!
//! A shared link names the version that was current when it was created. If
//! it tracked "current" instead, regenerating the deliverable would silently
//! change what the recipient downloads — after you already told them what was
//! in it. Pinning means the thing you shared is the thing they get.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::session::get_fleet_dir;

/// Longest lifetime a link may be given, in days.
///
/// Not a security boundary — a link on the LAN is only as private as the LAN —
/// but an unbounded default turns "quick, show this to someone" into a
/// permanent hole nobody remembers opening.
pub const MAX_TTL_DAYS: u64 = 365;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareLink {
    /// The bearer secret. Opaque, URL-safe, and the only thing the recipient
    /// needs — so it is also the only thing worth keeping unguessable.
    pub token: String,
    pub artifact_id: String,
    /// Version the link is pinned to; never empty (see module docs).
    pub version: String,
    /// Filename to serve it under, captured at creation so a later rename
    /// cannot change what the recipient's browser saves.
    pub name: String,
    pub created_ms: u64,
    /// `0` means no expiry.
    pub expires_ms: u64,
    /// Bumped every time the link is fetched, so the UI can say whether
    /// anyone actually opened it.
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub last_hit_ms: u64,
}

impl ShareLink {
    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        self.expires_ms != 0 && now_ms >= self.expires_ms
    }
}

// ── Paths ────────────────────────────────────────────────────────────────────

pub fn shares_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join("artifact-shares.json"))
}

fn shares_path_or_err() -> Result<PathBuf, String> {
    shares_path().ok_or_else(|| "cannot determine home dir".to_string())
}

// ── Read ─────────────────────────────────────────────────────────────────────

/// Every link on record, newest first, expired ones included.
///
/// Expired links are kept rather than swept on read: the UI showing "已过期"
/// beside a link the user made is more useful than it vanishing, and a read
/// path that silently rewrites the file would race every other reader.
pub fn list() -> Vec<ShareLink> {
    match shares_path() {
        Some(p) => list_in(&p),
        None => Vec::new(),
    }
}

pub fn list_in(path: &Path) -> Vec<ShareLink> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<ShareLink> = serde_json::from_str(&raw).unwrap_or_default();
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then_with(|| a.token.cmp(&b.token)));
    out
}

/// Links for one artifact, newest first.
pub fn list_for(artifact_id: &str) -> Vec<ShareLink> {
    list().into_iter().filter(|s| s.artifact_id == artifact_id).collect()
}

fn write_all(path: &Path, links: &[ShareLink]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create fleet dir: {e}"))?;
    }
    let body = serde_json::to_vec_pretty(links).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(path, &body)
        .map_err(|e| format!("write artifact-shares.json: {e}"))
}

// ── Create / revoke ──────────────────────────────────────────────────────────

/// Mint a link for `artifact_id`, pinned to `version` (or the current one).
///
/// `ttl_days` of `None` or `0` means it never expires.
pub fn create(
    artifact_id: &str,
    version: Option<&str>,
    ttl_days: Option<u64>,
) -> Result<ShareLink, String> {
    let path = shares_path_or_err()?;
    let root = crate::artifacts::artifacts_dir()
        .ok_or_else(|| "cannot determine home dir".to_string())?;
    create_in(&path, &root, artifact_id, version, ttl_days, now_ms())
}

pub fn create_in(
    path: &Path,
    artifacts_root: &Path,
    artifact_id: &str,
    version: Option<&str>,
    ttl_days: Option<u64>,
    now: u64,
) -> Result<ShareLink, String> {
    // Resolve against the store rather than trusting the caller: a link to an
    // artifact or version that does not exist is a 404 the user only finds out
    // about after sending it to someone.
    let artifact = crate::artifacts::get_in(artifacts_root, artifact_id)?;
    let version = match version.filter(|v| !v.is_empty()) {
        Some(v) => {
            if !artifact.versions.iter().any(|entry| entry.id == v) {
                return Err(format!("artifact '{artifact_id}' has no version '{v}'"));
            }
            v.to_string()
        }
        None => artifact.current_version.clone(),
    };
    let ttl = ttl_days.unwrap_or(0);
    if ttl > MAX_TTL_DAYS {
        return Err(format!("expiry must be at most {MAX_TTL_DAYS} days"));
    }

    let link = ShareLink {
        token: new_token(),
        artifact_id: artifact_id.to_string(),
        version,
        name: artifact.name,
        created_ms: now,
        expires_ms: if ttl == 0 { 0 } else { now + ttl * 24 * 60 * 60 * 1000 },
        hits: 0,
        last_hit_ms: 0,
    };
    let mut links = list_in(path);
    links.push(link.clone());
    write_all(path, &links)?;
    Ok(link)
}

/// A URL-safe bearer secret.
///
/// Two v4 UUIDs' worth of randomness with the dashes removed: 256 bits, no
/// new dependency, and nothing in it that a recipient could edit into another
/// artifact's token.
fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn revoke(token: &str) -> Result<(), String> {
    let path = shares_path_or_err()?;
    revoke_in(&path, token)
}

pub fn revoke_in(path: &Path, token: &str) -> Result<(), String> {
    let mut links = list_in(path);
    let before = links.len();
    links.retain(|s| s.token != token);
    if links.len() == before {
        return Err("no such share link".to_string());
    }
    write_all(path, &links)
}

/// Drop every link for an artifact — called when the artifact itself goes, so
/// a deleted deliverable leaves no live URL behind.
pub fn revoke_for_artifact(artifact_id: &str) -> Result<usize, String> {
    let path = shares_path_or_err()?;
    revoke_for_artifact_in(&path, artifact_id)
}

pub fn revoke_for_artifact_in(path: &Path, artifact_id: &str) -> Result<usize, String> {
    let mut links = list_in(path);
    let before = links.len();
    links.retain(|s| s.artifact_id != artifact_id);
    let removed = before - links.len();
    if removed > 0 {
        write_all(path, &links)?;
    }
    Ok(removed)
}

// ── Resolve (the serving path) ───────────────────────────────────────────────

/// What a presented token grants, or why it grants nothing.
///
/// The error strings are deliberately the same shape for "unknown" and
/// "expired" from the caller's point of view — both answer 404 — but they are
/// distinguished here so a log line can say which.
pub fn resolve(token: &str) -> Result<ShareLink, String> {
    let path = shares_path_or_err()?;
    resolve_in(&path, token, now_ms())
}

pub fn resolve_in(path: &Path, token: &str, now: u64) -> Result<ShareLink, String> {
    // An empty token must never match an empty stored field.
    if token.is_empty() {
        return Err("no share token".to_string());
    }
    let link = list_in(path)
        .into_iter()
        .find(|s| s.token == token)
        .ok_or_else(|| "unknown share token".to_string())?;
    if link.is_expired_at(now) {
        return Err("share link has expired".to_string());
    }
    Ok(link)
}

/// Record a fetch. Best-effort: a failed bookkeeping write must not fail the
/// download the recipient is already receiving.
pub fn record_hit(token: &str) {
    if let Ok(path) = shares_path_or_err() {
        record_hit_in(&path, token, now_ms());
    }
}

pub fn record_hit_in(path: &Path, token: &str, now: u64) {
    let mut links = list_in(path);
    let mut touched = false;
    for link in links.iter_mut() {
        if link.token == token {
            link.hits += 1;
            link.last_hit_ms = now;
            touched = true;
        }
    }
    if touched {
        let _ = write_all(path, &links);
    }
}

// ── The URL you actually send someone ────────────────────────────────────────

/// Build the URL for `token`, or say why there isn't one yet.
///
/// The desktop app does **not** listen on HTTP — it talks to the relay, not to
/// a port of its own — so the thing that serves `/shared` is a running
/// `fleet serve` / `fleet webui`. Both write their live port to
/// `~/.fleet/port` on startup, so its absence is the honest signal that a link
/// would not resolve for anyone, and the UI says so instead of handing over a
/// URL that refuses to connect.
///
/// The host is this machine's LAN IPv4 rather than `127.0.0.1`, because the
/// point of a share link is to open it somewhere else — a phone, a colleague's
/// laptop. `localhost` is the fallback when the machine has no LAN address at
/// all, which at least still works in the browser sitting right here.
pub fn share_url(token: &str) -> Result<String, String> {
    let port = live_serve_port().ok_or_else(|| {
        "no local Fleet server is running — start `fleet webui` and the link will resolve"
            .to_string()
    })?;
    let host = crate::lan_access::lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "localhost".to_string());
    Ok(format!("http://{host}:{port}{}?t={token}", crate::routes::SHARED))
}

/// The port a `fleet serve` / `fleet webui` last recorded, if any.
pub fn live_serve_port() -> Option<u16> {
    let path = crate::launchd::port_file_path()?;
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// An artifact store with one two-version artifact in it.
    fn store_with_artifact() -> (TempDir, TempDir, String) {
        let root = TempDir::new().unwrap();
        let ws = TempDir::new().unwrap();
        let src = ws.path().join("report.pdf");
        fs::write(&src, b"%PDF one").unwrap();
        let a = crate::artifacts::add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        // Write-temp-then-rename, so the archived v1 keeps its own bytes.
        let tmp = ws.path().join("report.pdf.tmp");
        fs::write(&tmp, b"%PDF two").unwrap();
        fs::rename(&tmp, &src).unwrap();
        crate::artifacts::add_in(root.path(), &src, None, None, ws.path(), None).unwrap();
        (root, ws, a.id)
    }

    fn shares(dir: &TempDir) -> PathBuf {
        dir.path().join("artifact-shares.json")
    }

    #[test]
    fn a_new_link_pins_the_current_version_and_the_filename() {
        let (root, ws, id) = store_with_artifact();
        let link =
            create_in(&shares(&ws), root.path(), &id, None, Some(7), 1_000_000).unwrap();

        assert_eq!(link.version, "v2", "current at creation, not 'whatever is current'");
        assert_eq!(link.name, "report.pdf");
        assert_eq!(link.expires_ms, 1_000_000 + 7 * 86_400_000);
        assert_eq!(link.token.len(), 64, "256 bits, hex, no dashes");
        assert_eq!(link.hits, 0);
    }

    #[test]
    fn a_link_can_be_pinned_to_an_older_version() {
        let (root, ws, id) = store_with_artifact();
        let link =
            create_in(&shares(&ws), root.path(), &id, Some("v1"), None, 1_000).unwrap();
        assert_eq!(link.version, "v1");
        assert_eq!(link.expires_ms, 0, "no ttl means no expiry");

        let err = create_in(&shares(&ws), root.path(), &id, Some("v9"), None, 1_000).unwrap_err();
        assert!(err.contains("no version 'v9'"), "{err}");
    }

    #[test]
    fn creating_a_link_for_a_missing_artifact_fails_now_not_later() {
        let (root, ws, _) = store_with_artifact();
        // Better to fail at 分享 time than to hand someone a URL that 404s.
        assert!(create_in(&shares(&ws), root.path(), "nope", None, None, 0).is_err());
    }

    #[test]
    fn resolve_refuses_unknown_empty_and_expired_tokens() {
        let (root, ws, id) = store_with_artifact();
        let path = shares(&ws);
        let link = create_in(&path, root.path(), &id, None, Some(1), 1_000).unwrap();

        assert_eq!(resolve_in(&path, &link.token, 1_000).unwrap().artifact_id, id);
        // Right up to the expiry instant, then not.
        assert!(resolve_in(&path, &link.token, link.expires_ms - 1).is_ok());
        assert!(resolve_in(&path, &link.token, link.expires_ms)
            .unwrap_err()
            .contains("expired"));

        assert!(resolve_in(&path, "", 1_000).unwrap_err().contains("no share token"));
        assert!(resolve_in(&path, "deadbeef", 1_000).unwrap_err().contains("unknown"));
    }

    #[test]
    fn a_link_grants_one_artifact_and_one_version_only() {
        let (root, ws, id) = store_with_artifact();
        let path = shares(&ws);
        let pinned = create_in(&path, root.path(), &id, Some("v1"), None, 1_000).unwrap();

        // The token carries its own artifact id and version; there is no field
        // a recipient could edit to reach anything else.
        let resolved = resolve_in(&path, &pinned.token, 2_000).unwrap();
        assert_eq!(resolved.artifact_id, id);
        assert_eq!(resolved.version, "v1");
        assert_eq!(
            crate::artifacts::read_version_bytes_in(
                root.path(),
                &resolved.artifact_id,
                Some(&resolved.version),
                None
            )
            .unwrap()
            .bytes,
            b"%PDF one",
            "and it keeps serving the bytes that were current when it was made"
        );
    }

    #[test]
    fn revoking_kills_the_link_and_only_that_link() {
        let (root, ws, id) = store_with_artifact();
        let path = shares(&ws);
        let a = create_in(&path, root.path(), &id, None, None, 1).unwrap();
        let b = create_in(&path, root.path(), &id, Some("v1"), None, 2).unwrap();

        revoke_in(&path, &a.token).unwrap();
        assert!(resolve_in(&path, &a.token, 10).is_err());
        assert!(resolve_in(&path, &b.token, 10).is_ok());
        assert!(revoke_in(&path, &a.token).unwrap_err().contains("no such"));
    }

    #[test]
    fn deleting_an_artifact_takes_all_of_its_links() {
        let (root, ws, id) = store_with_artifact();
        let path = shares(&ws);
        let a = create_in(&path, root.path(), &id, None, None, 1).unwrap();
        create_in(&path, root.path(), &id, Some("v1"), None, 2).unwrap();

        assert_eq!(revoke_for_artifact_in(&path, &id).unwrap(), 2);
        assert!(list_in(&path).is_empty(), "no live URL may outlive its artifact");
        assert!(resolve_in(&path, &a.token, 10).is_err());
        // Idempotent: nothing to remove is not an error.
        assert_eq!(revoke_for_artifact_in(&path, &id).unwrap(), 0);
    }

    #[test]
    fn hits_are_counted_so_the_ui_can_say_whether_anyone_opened_it() {
        let (root, ws, id) = store_with_artifact();
        let path = shares(&ws);
        let link = create_in(&path, root.path(), &id, None, None, 1).unwrap();

        record_hit_in(&path, &link.token, 5_000);
        record_hit_in(&path, &link.token, 9_000);
        let stored = list_in(&path).into_iter().next().unwrap();
        assert_eq!(stored.hits, 2);
        assert_eq!(stored.last_hit_ms, 9_000);

        // A hit on a token nobody minted must not create a record.
        record_hit_in(&path, "ghost", 9_500);
        assert_eq!(list_in(&path).len(), 1);
    }

    #[test]
    fn a_missing_shares_file_reads_as_no_links() {
        let dir = TempDir::new().unwrap();
        assert!(list_in(&dir.path().join("artifact-shares.json")).is_empty());
        // Garbage on disk must not blank the page either.
        let path = dir.path().join("artifact-shares.json");
        fs::write(&path, b"{ not json").unwrap();
        assert!(list_in(&path).is_empty());
    }

    #[test]
    fn refuses_a_ttl_beyond_the_ceiling() {
        let (root, ws, id) = store_with_artifact();
        let err = create_in(&shares(&ws), root.path(), &id, None, Some(MAX_TTL_DAYS + 1), 0)
            .unwrap_err();
        assert!(err.contains("at most"), "{err}");
        assert!(create_in(&shares(&ws), root.path(), &id, None, Some(MAX_TTL_DAYS), 0).is_ok());
    }
}
