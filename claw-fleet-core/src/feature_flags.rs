//! Startup feature flags — capabilities Fleet keeps **off** until the user
//! opts in with an environment variable, read once at process start.
//!
//! Currently one flag: the terminal surface (`FLEET_TERMINAL`). "Open me a shell
//! here" is the one capability in the app whose blast radius is the whole
//! machine — the desktop page, the browser build served by `fleet serve`, and
//! the phone all reach the same [`crate::proc_runner`] pty host — so it ships
//! disabled and a user who wants it exports `FLEET_TERMINAL=1` before
//! launching. Running *named* commands (the clone dialog, the repository page's
//! command panel) is a different, narrower capability and stays on.
//!
//! **Why the value is cached rather than re-read.** The flag is a launch-time
//! property of the process: a `fleet serve` that started without it must not
//! start honouring a shell request because something later mutated its
//! environment, and the three clients of one host must never disagree with
//! each other mid-run about whether the surface exists. `OnceLock` makes that
//! structural instead of a convention.
//!
//! Note for GUI launches: an app started from Finder/Explorer inherits the
//! launchd/login environment, not a terminal's — exporting the var in
//! `~/.zshrc` does **not** reach it. `launchctl setenv FLEET_TERMINAL 1` (or
//! launching the app from a shell that has it) does.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Which optional surfaces this host exposes — the payload every client fetches
/// once at boot so its UI matches what the backend will actually allow.
///
/// Deliberately a struct with named fields rather than a bare bool: the next
/// flag added here reaches all three clients without a second round trip and
/// without any of them growing a new endpoint.
/// Not `Copy`: `locale_default` is a `String`. Every caller hands the struct
/// straight to `serde`, so there was nothing leaning on the implicit copy.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HostFeatures {
    /// The terminal page / phone terminal — interactive shells on this machine.
    pub terminal: bool,
    /// Which language this host wants a client that has never been told
    /// otherwise to open in — the language subtag of [`LOCALE_ENV`], e.g.
    /// `"zh"`. `None` = this host has no opinion.
    ///
    /// A presentation default, not a capability: the browser build's settings
    /// live in one browser's localStorage, so a host that is configured for
    /// Chinese (this is the very `FLEET_LOCALE` that already renders the
    /// `~/.claude` guidance) had no way
    /// to say so to a page it serves — every visitor got English until they
    /// found the switch themselves.
    ///
    /// Deliberately *not* validated against a list of languages here: which
    /// bundles exist is the frontend's fact, not core's, so core reports what
    /// the host said and each client keeps its own default when it has no
    /// bundle for it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub locale_default: Option<String>,
    /// Whether this host wants simplified mode (the sidebar without Projects,
    /// Review and usage) as the *default* for a client that has never toggled
    /// it, from [`SIMPLIFIED_ENV`]. `None` = this host has no opinion.
    ///
    /// A presentation default like `locale_default`, not a capability: a user
    /// who flips the switch in Settings overrides it for that browser. It
    /// exists because the browser build's settings live in one browser's
    /// localStorage, so a deployment that wants the lean layout for everyone
    /// who opens it has no per-browser click that can say so.
    ///
    /// Skipped when absent so an older client (and every `{ terminal: … }`
    /// literal in the two frontends) keeps type-checking.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub simplified_default: Option<bool>,
}

/// This host's feature set, as served to every client.
pub fn host_features() -> HostFeatures {
    HostFeatures {
        terminal: terminal_enabled(),
        locale_default: locale_default(),
        simplified_default: simplified_default(),
    }
}

/// Env var giving simplified mode's host default; see
/// [`HostFeatures::simplified_default`].
/// `1/true/yes/on` = on, `0/false/no/off` = off, unset = no opinion.
pub const SIMPLIFIED_ENV: &str = "FLEET_SIMPLIFIED_MODE";

/// Tri-state read of a boolean env var: `Some(true)` / `Some(false)` for an
/// explicit value, `None` for unset (and for garbage, which must not be read as
/// either answer — a typo'd value means "the deployment did not say").
pub fn env_tristate(raw: Option<&str>) -> Option<bool> {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "true" | "yes" | "on") => Some(true),
        Some("0" | "false" | "no" | "off") => Some(false),
        _ => None,
    }
}

/// This host's simplified-mode default, from [`SIMPLIFIED_ENV`]. Cached like
/// [`locale_default`], for the same reason; changing it means restarting the
/// process (for the cloud container: setting the env in muvee and redeploying).
pub fn simplified_default() -> Option<bool> {
    static CACHED: OnceLock<Option<bool>> = OnceLock::new();
    *CACHED.get_or_init(|| env_tristate(std::env::var(SIMPLIFIED_ENV).ok().as_deref()))
}

/// Env var that enables the terminal surface.
pub const TERMINAL_ENV: &str = "FLEET_TERMINAL";

/// The values that count as "on". Anything else — including an empty string,
/// `0`, and a typo'd `ture` — is off, because a flag whose blast radius is a
/// shell on the user's machine must not be enabled by an accident.
pub fn env_truthy(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Whether the terminal surface (interactive shells) is enabled on this host.
///
/// Every client asks this same function — via the Tauri command, the
/// `/host_features` route, or the relay's `host_features` method — so what the
/// UI shows and what [`crate::proc_runner`] actually allows cannot drift.
pub fn terminal_enabled() -> bool {
    if let Some(forced) = test_override() {
        return forced;
    }
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| env_truthy(std::env::var(TERMINAL_ENV).ok().as_deref()))
}

/// Env var naming this host's language. The same one `fleet bootstrap` /
/// `fleet webui`'s startup heal read to render the `~/.claude` guidance, so a
/// host says "I am a Chinese host" once and both surfaces follow.
pub const LOCALE_ENV: &str = "FLEET_LOCALE";

/// Reduce a locale to its language subtag: `"zh-CN"` / `"zh_CN"` / `" ZH "` all
/// become `"zh"`. `None` for unset, empty, or anything that is not letters —
/// garbage must not be reported as an opinion, for the same reason
/// [`env_truthy`] refuses to read a typo as "on".
pub fn locale_subtag(raw: Option<&str>) -> Option<String> {
    let tag = raw?
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(tag)
}

/// This host's language default, from [`LOCALE_ENV`].
///
/// Cached like [`terminal_enabled`] and for the same reason: it is a launch
/// property of the process, and the desktop / browser build / phone reading one
/// host must not disagree about it mid-run.
pub fn locale_default() -> Option<String> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| locale_subtag(std::env::var(LOCALE_ENV).ok().as_deref()))
        .clone()
}

#[cfg(test)]
mod test_override {
    use std::sync::atomic::{AtomicU8, Ordering};

    /// 0 = read the env, 1 = force on, 2 = force off.
    static FORCED: AtomicU8 = AtomicU8::new(0);

    pub(super) fn get() -> Option<bool> {
        match FORCED.load(Ordering::Relaxed) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    /// Pins the flag for the rest of the test binary's life, so a gate test
    /// does not depend on the developer's shell (`cargo test` inherits it) and
    /// does not fight `OnceLock`'s one-shot cache.
    pub fn force(enabled: Option<bool>) {
        FORCED.store(
            match enabled {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            },
            Ordering::Relaxed,
        );
    }
}

#[cfg(test)]
pub(crate) use test_override::force as force_terminal_for_tests;

#[cfg(test)]
fn test_override() -> Option<bool> {
    test_override::get()
}

#[cfg(not(test))]
fn test_override() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Simplified mode is tri-state, not truthy: "the deployment said off" and
    // "the deployment said nothing" are different answers.
    #[test]
    fn simplified_env_distinguishes_off_from_unset() {
        for on in ["1", "true", "TRUE", "yes", "On", " 1 "] {
            assert_eq!(env_tristate(Some(on)), Some(true), "{on:?} should be on");
        }
        for off in ["0", "false", "NO", "off", " 0 "] {
            assert_eq!(env_tristate(Some(off)), Some(false), "{off:?} should be off");
        }
        for silent in ["", " ", "ture", "2", "maybe"] {
            assert_eq!(env_tristate(Some(silent)), None, "{silent:?} says nothing");
        }
        assert_eq!(env_tristate(None), None, "unset says nothing");
    }

    #[test]
    fn only_explicit_affirmatives_enable_a_flag() {
        for on in ["1", "true", "TRUE", "yes", "On", " 1 "] {
            assert!(env_truthy(Some(on)), "{on:?} should enable");
        }
        for off in ["", " ", "0", "false", "no", "off", "ture", "2"] {
            assert!(!env_truthy(Some(off)), "{off:?} should not enable");
        }
        assert!(
            !env_truthy(None),
            "an unset var is off — that is the default"
        );
    }

    // The host names a *locale*; the clients index bundles by language. Region
    // and case are noise in between, and must not turn "zh-CN" into a language
    // nobody ships a bundle for.
    #[test]
    fn locale_is_reduced_to_its_language_subtag() {
        for raw in ["zh", "zh-CN", "zh_CN", " ZH ", "zh-Hans-CN"] {
            assert_eq!(locale_subtag(Some(raw)).as_deref(), Some("zh"), "{raw:?}");
        }
        assert_eq!(locale_subtag(Some("en_US.UTF-8")).as_deref(), Some("en"));
    }

    #[test]
    fn a_locale_that_is_not_a_language_says_nothing() {
        for silent in ["", "   ", "-", "_CN", "1", "zh2", "C.UTF-8"] {
            assert_eq!(locale_subtag(Some(silent)), None, "{silent:?} says nothing");
        }
        assert_eq!(locale_subtag(None), None, "unset says nothing");
    }
}
