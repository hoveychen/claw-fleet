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

/// Normalise a bare region code as returned by the Windows user geo API —
/// already a region, not a locale. Alpha-2 codes pass through uppercased;
/// numeric UN M.49 regions (Windows may return e.g. `001` for "World") have no
/// alpha-2 form and yield `None`.
pub fn country_from_geo_name(raw: &str) -> Option<String> {
    let code = raw.trim();
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| code.to_ascii_uppercase())
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

/// Windows: the user's "Region" setting (Settings → Time & language → Region →
/// Country or region), which is what `LANG` would have carried on unix. A GUI
/// session sets no `LC_*`, so this is the live path there.
#[cfg(windows)]
fn detect_country_platform() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultGeoName;
    // An ISO-3166 alpha-2 code, or a UN M.49 decimal string when the location
    // has no alpha-2 assignment. 16 UTF-16 units is far past either.
    let mut buf = [0u16; 16];
    // 0 means failure. Non-zero is "characters copied", which the docs leave
    // ambiguous about the trailing NUL, so read up to the NUL rather than
    // trusting the count.
    if unsafe { GetUserDefaultGeoName(&mut buf) } == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    country_from_geo_name(&String::from_utf16_lossy(&buf[..end]))
}

// Anything else (Linux, BSD) with no LC_* env falls through to `None`, i.e. the
// global host — the pre-region behaviour.
#[cfg(not(any(target_os = "macos", windows)))]
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
    fn windows_geo_names_are_bare_region_codes() {
        // GetUserDefaultGeoName hands back the region itself, not a locale, so
        // it must not go through country_from_locale (which drops the first
        // subtag and would read "CN" as a language).
        assert_eq!(country_from_locale("CN"), None);
        assert_eq!(country_from_geo_name("CN").as_deref(), Some("CN"));
        assert_eq!(country_from_geo_name("us").as_deref(), Some("US"));
        // M.49 numeric regions ("001" = World) have no alpha-2 form.
        assert_eq!(country_from_geo_name("001"), None);
        assert_eq!(country_from_geo_name(""), None);
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
