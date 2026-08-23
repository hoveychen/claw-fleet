//! Which relay host this desktop should use, by system region.
//!
//! Both hostnames below front the *same* relay container, so the choice is a
//! pure network-path preference: an agent registered via one hostname is
//! visible to a phone that authed via the other (same in-memory channel
//! registry, same VAPID keypair). That is what makes swapping the host safe —
//! it is not a separate deployment with separate pairing state.
//!
//! The region is the *desktop's*, not the phone's: the host picked here lands
//! in the pairing QR (`mobile_relay::pairing_url`), and every mobile client
//! stores whatever host it scanned. A phone that needs the other host has to
//! rescan a QR generated with it.

use std::sync::OnceLock;

/// Relay hostname for mainland China — fronted by a mainland reverse proxy.
pub const RELAY_URL_CN: &str = "https://fleet-relay.eternizedlab.com";
/// Relay hostname everywhere else — the muvee host itself, no extra hop.
pub const RELAY_URL_GLOBAL: &str = "https://fleet-relay.muveeai.com";

/// Map an ISO-3166 alpha-2 country code to the relay base URL. `None` (region
/// undetectable) yields the global host, i.e. the historical default — an
/// unknown region must never be a behaviour change.
pub fn relay_url_for_country(country: Option<&str>) -> &'static str {
    match country {
        Some(c) if c.eq_ignore_ascii_case("CN") => RELAY_URL_CN,
        _ => RELAY_URL_GLOBAL,
    }
}

/// Pull the region subtag out of a locale identifier — POSIX (`zh_CN.UTF-8`,
/// `en_US@euro`) or BCP-47 (`zh-Hans-CN`). Returns the uppercased two-letter
/// region, or `None` for locales that carry no region (`en`, `C`, `POSIX`) and
/// for numeric UN M.49 regions (`es-419`), which we have no mapping for.
pub fn country_from_locale(raw: &str) -> Option<String> {
    // Trim the POSIX codeset (`.UTF-8`) and modifier (`@euro`) suffixes.
    let stem = raw.split(['.', '@']).next()?.trim();
    // The region is the last 2-alpha subtag *after* the language: `zh_CN` → CN,
    // `zh-Hans-CN` → CN. Skipping the first subtag is what keeps a bare `en`
    // from reading as the country EN.
    stem.split(['_', '-'])
        .skip(1)
        .filter(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()))
        .last()
        .map(|c| c.to_ascii_uppercase())
}

/// The system's region code, cached for the process lifetime (a machine does
/// not change country mid-run, and the macOS path shells out).
pub fn detect_country() -> Option<String> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED.get_or_init(detect_country_uncached).clone()
}

fn detect_country_uncached() -> Option<String> {
    // POSIX locale envs, in the precedence order the C library uses. Present
    // for a terminal-launched `fleet`, usually absent for a GUI-launched app.
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(c) = std::env::var(key).ok().as_deref().and_then(country_from_locale) {
            return Some(c);
        }
    }
    detect_country_platform()
}

#[cfg(target_os = "macos")]
fn detect_country_platform() -> Option<String> {
    // The GUI app inherits no LANG, so this is the live path on macOS.
    // Absolute path: a bundled app's PATH holds only the four system dirs
    // (see `session::augmented_path_with_front`), and `defaults` is in /usr/bin.
    let out = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    country_from_locale(String::from_utf8_lossy(&out.stdout).trim())
}

// Windows/Linux GUI sessions with no LC_* env fall through to `None`, i.e. the
// global host — the pre-region behaviour. Reading the Windows user geo
// (`GetUserDefaultGeoName`) would need a Win32 binding this host cannot
// compile-check, so it is deliberately left out rather than shipped untested.
#[cfg(not(target_os = "macos"))]
fn detect_country_platform() -> Option<String> {
    None
}

/// The relay base URL for this machine's region.
pub fn region_default_relay_url() -> &'static str {
    relay_url_for_country(detect_country().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_posix_and_bcp47_locales() {
        assert_eq!(country_from_locale("zh_CN.UTF-8").as_deref(), Some("CN"));
        assert_eq!(country_from_locale("zh_CN").as_deref(), Some("CN"));
        assert_eq!(country_from_locale("en_US@euro").as_deref(), Some("US"));
        assert_eq!(country_from_locale("zh-Hans-CN").as_deref(), Some("CN"));
        assert_eq!(country_from_locale("zh-Hant-TW").as_deref(), Some("TW"));
        // Lowercase region in a BCP-47 tag still resolves.
        assert_eq!(country_from_locale("zh-cn").as_deref(), Some("CN"));
    }

    #[test]
    fn regionless_locales_yield_none() {
        assert_eq!(country_from_locale("C"), None);
        assert_eq!(country_from_locale("POSIX"), None);
        assert_eq!(country_from_locale("en"), None);
        assert_eq!(country_from_locale(""), None);
        // UN M.49 numeric regions have no alpha-2 form.
        assert_eq!(country_from_locale("es-419"), None);
    }

    #[test]
    fn only_mainland_china_gets_the_cn_host() {
        assert_eq!(relay_url_for_country(Some("CN")), RELAY_URL_CN);
        assert_eq!(relay_url_for_country(Some("cn")), RELAY_URL_CN);
        // Neighbours on the same UTC+8 offset are *not* mainland.
        assert_eq!(relay_url_for_country(Some("HK")), RELAY_URL_GLOBAL);
        assert_eq!(relay_url_for_country(Some("TW")), RELAY_URL_GLOBAL);
        assert_eq!(relay_url_for_country(Some("SG")), RELAY_URL_GLOBAL);
        assert_eq!(relay_url_for_country(Some("US")), RELAY_URL_GLOBAL);
        // Undetectable region keeps the historical default.
        assert_eq!(relay_url_for_country(None), RELAY_URL_GLOBAL);
    }
}
