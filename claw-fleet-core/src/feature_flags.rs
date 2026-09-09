//! Startup feature flags — capabilities Fleet keeps **off** until the user
//! opts in with an environment variable, read once at process start.
//!
//! Currently one flag: the 终端 surface (`FLEET_TERMINAL`). "Open me a shell
//! here" is the one capability in the app whose blast radius is the whole
//! machine — the desktop page, the browser build served by `fleet serve`, and
//! the phone all reach the same [`crate::proc_runner`] pty host — so it ships
//! disabled and a user who wants it exports `FLEET_TERMINAL=1` before
//! launching. Running *named* commands (the clone dialog, the 仓库 page's 命令
//! panel) is a different, narrower capability and stays on.
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
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HostFeatures {
    /// The 终端 page / phone terminal — interactive shells on this machine.
    pub terminal: bool,
    /// Whether this host wants 精简模式 (Tasks + Artifacts only) as the
    /// *default* for a client that has never been told otherwise. `None` = this
    /// host has no opinion, so the client keeps its own default (off).
    ///
    /// Unlike `terminal` this is a **presentation** default, not a capability:
    /// nothing is gated by it, and a user who flips the switch in Settings
    /// overrides it for that browser. It exists because the browser build's
    /// settings live in that one browser's localStorage (`webTransport.ts`
    /// bridges `plugin:store` to it), so a deployment that wants the lean
    /// layout for *everyone who opens it* has no per-browser click that can
    /// say so — only the host can.
    ///
    /// Skipped when absent so an older client (and every `{ terminal: … }`
    /// literal already in the two frontends) keeps type-checking against the
    /// generated binding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub simplified_default: Option<bool>,
}

/// This host's feature set, as served to every client.
pub fn host_features() -> HostFeatures {
    HostFeatures {
        terminal: terminal_enabled(),
        simplified_default: simplified_default(),
    }
}

/// Env var that enables the 终端 surface.
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

/// Whether the 终端 surface (interactive shells) is enabled on this host.
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

/// Env var that sets 精简模式's default for every client of this host.
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

/// 精简模式's host-level default, from [`SIMPLIFIED_ENV`].
///
/// Cached like [`terminal_enabled`] and for the same reason: it is a launch
/// property of the process, and the desktop / browser build / phone reading one
/// host must not disagree about it mid-run. Changing it means restarting the
/// process (for the cloud container: setting the env in muvee and redeploying).
pub fn simplified_default() -> Option<bool> {
    static CACHED: OnceLock<Option<bool>> = OnceLock::new();
    *CACHED.get_or_init(|| env_tristate(std::env::var(SIMPLIFIED_ENV).ok().as_deref()))
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

    #[test]
    fn only_explicit_affirmatives_enable_a_flag() {
        for on in ["1", "true", "TRUE", "yes", "On", " 1 "] {
            assert!(env_truthy(Some(on)), "{on:?} should enable");
        }
        for off in ["", " ", "0", "false", "no", "off", "ture", "2"] {
            assert!(!env_truthy(Some(off)), "{off:?} should not enable");
        }
        assert!(!env_truthy(None), "an unset var is off — that is the default");
    }

    // 精简模式 is tri-state, not truthy: "the deployment said off" and "the
    // deployment said nothing" are different answers, because only the latter
    // leaves a browser's own stored choice / default in charge.
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
}
