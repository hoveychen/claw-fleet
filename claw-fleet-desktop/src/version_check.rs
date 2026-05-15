//! Release version check with a 1-day TTL cache.
//!
//! On first call (or when the cache is older than 24 hours), fetches the
//! latest release from GitHub and writes the result to
//! `~/.fleet/fleet-version-check.json`.  Subsequent calls within the TTL
//! window return the cached result instantly with no network I/O.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const GITHUB_API_URL: &str =
    "https://api.github.com/repos/hoveychen/claw-fleet/releases/latest";

/// 1 day in seconds.
const TTL_SECS: u64 = 24 * 60 * 60;

// ── Public result type ───────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VersionCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
}

// ── Cache file layout ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct CacheFile {
    checked_at: u64,
    latest_version: String,
    release_url: String,
}

// ── GitHub API response (only the fields we need) ───────────────────────────

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-version-check.json"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Parse "v26.3.28" / "26.3.28" / "26.3.28-dev.123" → (major, minor, patch).
fn parse_version(v: &str) -> (u32, u32, u32) {
    let v = v.trim_start_matches('v');
    let base = v.split('-').next().unwrap_or(v);
    let mut parts = base.split('.').filter_map(|p| p.parse::<u32>().ok());
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

// ── Network fetch ────────────────────────────────────────────────────────────

fn fetch_latest() -> Result<(String, String), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get(GITHUB_API_URL)
        .header("User-Agent", "claw-fleet-version-check")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("fetch: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let release: GithubRelease = resp.json().map_err(|e| format!("json: {e}"))?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    Ok((version, release.html_url))
}

/// Fetch from GitHub, update the cache file, and return (version, url).
/// On network failure returns empty strings and logs the error.
fn fetch_and_cache(path: &PathBuf) -> (String, String) {
    match fetch_latest() {
        Ok((version, url)) => {
            let cache = CacheFile {
                checked_at: now_secs(),
                latest_version: version.clone(),
                release_url: url.clone(),
            };
            if let Ok(json) = serde_json::to_string(&cache) {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, json);
            }
            (version, url)
        }
        Err(e) => {
            crate::log_debug(&format!("version_check: fetch failed: {e}"));
            (String::new(), String::new())
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Return version information, using a 1-day cached result when available.
/// Never panics; on any error the `latest_version` field is empty and
/// `has_update` is false.
pub fn check_app_version() -> VersionCheckResult {
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    let (latest_version, release_url) = match cache_path() {
        Some(path) => {
            let cached = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<CacheFile>(&s).ok());

            match cached {
                Some(c) if now_secs().saturating_sub(c.checked_at) < TTL_SECS => {
                    // Cache is still fresh.
                    (c.latest_version, c.release_url)
                }
                _ => {
                    // Cache is stale or absent — go to the network.
                    fetch_and_cache(&path)
                }
            }
        }
        None => fetch_latest().unwrap_or_else(|_| (String::new(), String::new())),
    };

    let has_update = !latest_version.is_empty()
        && parse_version(&latest_version) > parse_version(&current_version);

    VersionCheckResult {
        current_version,
        latest_version,
        has_update,
        release_url,
    }
}
