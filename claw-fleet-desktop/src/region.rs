//! Best-effort detection of whether the user is likely in mainland China.
//!
//! We use this to pick a sensible default tunnel provider (OpenFrp for China,
//! Cloudflare for elsewhere) and to surface a one-time onboarding dialog.
//!
//! The check is intentionally cheap and offline: reading the system locale and
//! timezone is enough for a heuristic. False positives (e.g. a Singapore user
//! with `zh_SG` locale in UTC+8) only result in OpenFrp being *suggested*, not
//! forced — the user always picks the provider in the UI.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Likely in mainland China (or another zh-locale UTC+8 region).
    China,
    /// Anything else.
    Other,
}

/// Detect the user's region. Cached for the lifetime of the process.
pub fn detect() -> Region {
    static CACHED: OnceLock<Region> = OnceLock::new();
    *CACHED.get_or_init(|| classify(read_locale().as_deref(), read_tz().as_deref()))
}

/// Pure classification logic — exposed for testing.
fn classify(locale: Option<&str>, tz: Option<&str>) -> Region {
    let locale_says_china = locale
        .map(|l| {
            let lower = l.to_ascii_lowercase();
            lower.starts_with("zh_cn")
                || lower.starts_with("zh-cn")
                || lower == "zh"
                || lower.starts_with("zh_")
        })
        .unwrap_or(false);

    let tz_says_china = tz
        .map(|t| {
            matches!(
                t,
                "Asia/Shanghai"
                    | "Asia/Chongqing"
                    | "Asia/Chungking"
                    | "Asia/Urumqi"
                    | "Asia/Kashgar"
                    | "Asia/Harbin"
                    | "PRC"
            )
        })
        .unwrap_or(false);

    if tz_says_china || locale_says_china {
        Region::China
    } else {
        Region::Other
    }
}

fn read_locale() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() && v != "C" && v != "POSIX" {
                return Some(v);
            }
        }
    }
    None
}

fn read_tz() -> Option<String> {
    if let Ok(v) = std::env::var("TZ") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.find("zoneinfo/") {
            return Some(s[idx + "zoneinfo/".len()..].to_string());
        }
    }
    if let Ok(content) = std::fs::read_to_string("/etc/timezone") {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_china_via_timezone() {
        assert_eq!(classify(None, Some("Asia/Shanghai")), Region::China);
        assert_eq!(classify(None, Some("Asia/Urumqi")), Region::China);
    }

    #[test]
    fn classify_china_via_locale() {
        assert_eq!(classify(Some("zh_CN.UTF-8"), None), Region::China);
        assert_eq!(classify(Some("zh-CN"), None), Region::China);
        assert_eq!(classify(Some("zh_HK.UTF-8"), None), Region::China);
    }

    #[test]
    fn classify_other_for_western_locales() {
        assert_eq!(classify(Some("en_US.UTF-8"), Some("America/New_York")), Region::Other);
        assert_eq!(classify(Some("ja_JP.UTF-8"), Some("Asia/Tokyo")), Region::Other);
    }

    #[test]
    fn classify_other_when_unknown() {
        assert_eq!(classify(None, None), Region::Other);
    }
}
