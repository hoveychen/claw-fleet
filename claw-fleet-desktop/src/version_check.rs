//! Release version check with a 1-day TTL cache.
//!
//! On first call (or when the cache is older than 24 hours), fetches the
//! latest release from the region-preferred source and writes the result to
//! `~/.fleet/fleet-version-check.json`.  Subsequent calls within the TTL
//! window return the cached result instantly with no network I/O.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const GITHUB_API_URL: &str = "https://api.github.com/repos/hoveychen/claw-fleet/releases/latest";
const CHINA_MANIFEST_URL: &str = "https://fleet.eternizedlab.com/downloads.json";
const CHINA_SITE_URL: &str = "https://fleet.eternizedlab.com/";
const GLOBAL_SITE_URL: &str = "https://hoveychen.github.io/claw-fleet/";

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
}

// ── GitHub API response (only the fields we need) ───────────────────────────

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
}

#[derive(Deserialize)]
struct ChinaManifest {
    schema: u32,
    version: String,
    china: serde_json::Value,
}

const REQUIRED_DESKTOP_ASSETS: [&str; 2] = [
    "claw-fleet-macos.pkg",
    "claw-fleet-windows-x64-setup.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseSource {
    Github,
    ChinaManifest,
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

fn release_sources_for_country(country: Option<&str>) -> [ReleaseSource; 2] {
    if country.is_some_and(|c| c.eq_ignore_ascii_case("CN")) {
        [ReleaseSource::ChinaManifest, ReleaseSource::Github]
    } else {
        [ReleaseSource::Github, ReleaseSource::ChinaManifest]
    }
}

fn normalise_release_version(raw: &str) -> Result<String, String> {
    let version = raw.strip_prefix('v').unwrap_or(raw);
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(format!("invalid stable version: {raw}"));
    }
    Ok(version.to_string())
}

fn parse_release_version(source: ReleaseSource, body: &str) -> Result<String, String> {
    match source {
        ReleaseSource::Github => {
            let release: GithubRelease =
                serde_json::from_str(body).map_err(|e| format!("github json: {e}"))?;
            if REQUIRED_DESKTOP_ASSETS
                .iter()
                .any(|required| !release.assets.iter().any(|asset| asset.name == *required))
            {
                return Err("GitHub release is missing desktop installers".into());
            }
            normalise_release_version(&release.tag_name)
        }
        ReleaseSource::ChinaManifest => {
            let manifest: ChinaManifest =
                serde_json::from_str(body).map_err(|e| format!("china manifest json: {e}"))?;
            let assets = manifest
                .china
                .get("assets")
                .and_then(serde_json::Value::as_object);
            if manifest.schema != 1
                || assets.is_none_or(|assets| {
                    REQUIRED_DESKTOP_ASSETS
                        .iter()
                        .any(|required| !assets.contains_key(*required))
                })
            {
                return Err("unsupported or incomplete China manifest".into());
            }
            normalise_release_version(&manifest.version)
        }
    }
}

fn fetch_latest_with<F>(country: Option<&str>, mut fetch: F) -> Result<String, String>
where
    F: FnMut(ReleaseSource) -> Result<String, String>,
{
    let mut errors = Vec::new();
    for source in release_sources_for_country(country) {
        match fetch(source) {
            Ok(version) => return Ok(version),
            Err(error) => errors.push(format!("{source:?}: {error}")),
        }
    }
    Err(errors.join("; "))
}

fn fetch_latest(country: Option<&str>) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    fetch_latest_with(country, |source| {
        let url = match source {
            ReleaseSource::Github => GITHUB_API_URL,
            ReleaseSource::ChinaManifest => CHINA_MANIFEST_URL,
        };
        let response = client
            .get(url)
            .header("User-Agent", "claw-fleet-version-check")
            .header("Accept", "application/json")
            .send()
            .map_err(|e| format!("fetch {url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        let body = response.text().map_err(|e| format!("read {url}: {e}"))?;
        parse_release_version(source, &body)
    })
}

fn resolve_latest_with<F>(path: &PathBuf, force: bool, now: u64, fetch: F) -> String
where
    F: FnOnce() -> Result<String, String>,
{
    if !force {
        let cached = std::fs::read_to_string(path)
            .ok()
            .and_then(|body| serde_json::from_str::<CacheFile>(&body).ok());
        if let Some(cache) = cached {
            if now.saturating_sub(cache.checked_at) < TTL_SECS {
                return cache.latest_version;
            }
        }
    }
    match fetch() {
        Ok(version) => {
            let cache = CacheFile {
                checked_at: now,
                latest_version: version.clone(),
            };
            if let Ok(json) = serde_json::to_string(&cache) {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, json);
            }
            version
        }
        Err(e) => {
            crate::log_debug(&format!("version_check: fetch failed: {e}"));
            String::new()
        }
    }
}

fn update_landing_url(country: Option<&str>, locale: &str) -> String {
    let is_china = country.is_some_and(|c| c.eq_ignore_ascii_case("CN"));
    let base = if is_china {
        CHINA_SITE_URL
    } else {
        GLOBAL_SITE_URL
    };
    let language = if locale.to_ascii_lowercase().starts_with("zh") {
        "zh"
    } else {
        "en"
    };
    if is_china {
        format!("{base}?lang={language}&source=china#download")
    } else {
        format!("{base}?lang={language}#download")
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Return version information, using a 1-day cached result when available.
/// Never panics; on any error the `latest_version` field is empty and
/// `has_update` is false.
pub fn check_app_version(force: bool, locale: &str) -> VersionCheckResult {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let country = claw_fleet_core::relay_region::detect_country();
    let latest_version = match cache_path() {
        Some(path) => resolve_latest_with(&path, force, now_secs(), || {
            fetch_latest(country.as_deref())
        }),
        None => fetch_latest(country.as_deref()).unwrap_or_default(),
    };

    let has_update = !latest_version.is_empty()
        && parse_version(&latest_version) > parse_version(&current_version);

    VersionCheckResult {
        current_version,
        latest_version,
        has_update,
        release_url: update_landing_url(country.as_deref(), locale),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn mainland_prefers_mirror_and_global_prefers_github() {
        assert_eq!(
            release_sources_for_country(Some("CN")),
            [ReleaseSource::ChinaManifest, ReleaseSource::Github]
        );
        assert_eq!(
            release_sources_for_country(Some("US")),
            [ReleaseSource::Github, ReleaseSource::ChinaManifest]
        );
        assert_eq!(
            release_sources_for_country(None),
            [ReleaseSource::Github, ReleaseSource::ChinaManifest]
        );
    }

    #[test]
    fn parses_both_release_sources_and_rejects_bad_mirror_schema() {
        assert_eq!(
            parse_release_version(
                ReleaseSource::Github,
                r#"{"tag_name":"v2.7.1","assets":[{"name":"claw-fleet-macos.pkg"},{"name":"claw-fleet-windows-x64-setup.exe"}]}"#,
            )
            .unwrap(),
            "2.7.1"
        );
        assert_eq!(
            parse_release_version(
                ReleaseSource::ChinaManifest,
                r#"{"schema":1,"version":"v2.7.1","china":{"assets":{"claw-fleet-macos.pkg":{},"claw-fleet-windows-x64-setup.exe":{}}}}"#,
            )
            .unwrap(),
            "2.7.1"
        );
        assert!(parse_release_version(
            ReleaseSource::ChinaManifest,
            r#"{"schema":2,"version":"v2.7.1","china":{"assets":{"claw-fleet-macos.pkg":{},"claw-fleet-windows-x64-setup.exe":{}}}}"#,
        )
        .is_err());
    }

    #[test]
    fn incomplete_release_is_rejected_during_publish_window() {
        assert!(parse_release_version(
            ReleaseSource::Github,
            r#"{"tag_name":"v2.7.1","assets":[]}"#,
        )
        .is_err());
        assert!(parse_release_version(
            ReleaseSource::ChinaManifest,
            r#"{"schema":1,"version":"v2.7.1","china":{"assets":{"claw-fleet-macos.pkg":{}}}}"#,
        )
        .is_err());
    }

    #[test]
    fn failed_preferred_source_falls_back_in_region_order() {
        let seen = RefCell::new(Vec::new());
        let latest = fetch_latest_with(Some("CN"), |source| {
            seen.borrow_mut().push(source);
            match source {
                ReleaseSource::ChinaManifest => Err("mirror unavailable".into()),
                ReleaseSource::Github => Ok("2.7.1".into()),
            }
        })
        .unwrap();
        assert_eq!(latest, "2.7.1");
        assert_eq!(
            seen.into_inner(),
            [ReleaseSource::ChinaManifest, ReleaseSource::Github]
        );
    }

    #[test]
    fn old_cache_with_github_url_migrates_but_url_is_recomputed() {
        let old = r#"{"checked_at":123,"latest_version":"2.7.1","release_url":"https://github.com/hoveychen/claw-fleet/releases/tag/v2.7.1"}"#;
        let cache: CacheFile = serde_json::from_str(old).unwrap();
        assert_eq!(cache.latest_version, "2.7.1");
        assert_eq!(
            update_landing_url(Some("CN"), "en-US"),
            "https://fleet.eternizedlab.com/?lang=en&source=china#download"
        );
        assert_eq!(
            update_landing_url(Some("US"), "zh-CN"),
            "https://hoveychen.github.io/claw-fleet/?lang=zh#download"
        );
    }

    #[test]
    fn manual_force_bypasses_a_fresh_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version.json");
        std::fs::write(
            &path,
            serde_json::json!({"checked_at": 1_000, "latest_version": "2.6.0"}).to_string(),
        )
        .unwrap();
        let calls = RefCell::new(0);
        let cached = resolve_latest_with(&path, false, 1_001, || {
            *calls.borrow_mut() += 1;
            Ok("2.7.1".into())
        });
        assert_eq!(cached, "2.6.0");
        assert_eq!(*calls.borrow(), 0);
        let refreshed = resolve_latest_with(&path, true, 1_001, || {
            *calls.borrow_mut() += 1;
            Ok("2.7.1".into())
        });
        assert_eq!(refreshed, "2.7.1");
        assert_eq!(*calls.borrow(), 1);
    }
}
