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
}

/// This host's feature set, as served to every client.
pub fn host_features() -> HostFeatures {
    HostFeatures {
        terminal: terminal_enabled(),
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
}
