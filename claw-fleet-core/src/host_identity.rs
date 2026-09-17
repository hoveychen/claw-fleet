//! Who is this host — the minimal information a phone's device book needs to name a paired device.
//!
//! Previously, the phone's device book (`mobile-web/src/devices.ts`) could only call a newly paired
//! desktop "Device 1", "Device 2", etc. The pairing QR code carries only the key and relay address;
//! hostname has never crossed this boundary. Once there are two or more devices on file, that sequence
//! number is pure noise — the user wants to pick "the Mac at home", not "Device 2".
//!
//! This is intentionally separate from [`crate::feature_flags::HostFeatures`]: that side is a
//! **capability gate** (clients decide whether a surface exists), this side is **identity**
//! (display-only, gates nothing). Mixing them into one struct would pull the "fail closed if you
//! can't get it" rule onto a field that should never fail.
//!
//! The counterpart is `deviceLabel.ts` on the phone: that side guesses what the phone is from its UA,
//! this side is self-reported by the host. Both sides are display-only and both allow "unknown".

use serde::{Deserialize, Serialize};

/// The identity of a Fleet host, sent as display-only information to each client.
///
/// Each field may be absent: in a container, `hostname` might be random hex, and on some systems
/// `os_version` cannot be retrieved. When absent, clients fall back to the platform name ("macOS")
/// rather than make one up.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    /// Hostname with mDNS/LAN suffix stripped (`.local` / `.lan`). `None` if unavailable.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hostname: Option<String>,
    /// Platform key, from `std::env::consts::OS` (`macos` / `windows` / `linux`, …).
    /// Clients use it to pick an icon, so it is machine-readable lowercase, not a display name.
    pub platform: String,
    /// OS version (`15.2`, `11`, …). Display-only; `None` if unavailable.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub os_version: Option<String>,
}

/// Strip mDNS/LAN suffix from hostname. On macOS, `hostname` is often
/// `Hoveys-MacBook-Pro.local`, but that suffix is noise in the device list.
///
/// Only strip the **final** segment, and only these two known suffixes — a machine actually named
/// `build.local.example` should not be truncated to `build`.
pub fn trim_host_suffix(raw: &str) -> String {
    let name = raw.trim();
    for suffix in [".local", ".lan"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            if !stripped.is_empty() {
                return stripped.to_string();
            }
        }
    }
    name.to_string()
}

/// On macOS, ask `scutil` for the user-configured machine name.
///
/// `gethostname()` (i.e., `sysinfo::System::host_name()`) returns the **transient hostname** on
/// macOS: unless the system admin has pinned `HostName` via `scutil --set HostName`, the system
/// will overwrite it with names from DHCP/reverse DNS. On 2026-09-16, tested on the user's MacBook
/// over a phone hotspot: the remote end sent back "Private Wi-Fi Address" as the hostname, so
/// `hostname` became `de:e8:92:d6:ca:71`, and the Mac's name in the phone's device book instantly
/// changed to that MAC address string.
///
/// `ComputerName` is the name the user set in System Settings > General > About This Mac; it
/// doesn't change with the network. `LocalHostName` is its ASCII-fied version, second choice.
/// Only fall back to `gethostname()` if both are unavailable.
#[cfg(target_os = "macos")]
fn scutil_name() -> Option<String> {
    for key in ["ComputerName", "LocalHostName"] {
        let out = std::process::Command::new("/usr/sbin/scutil").arg("--get").arg(key).output().ok();
        if let Some(out) = out {
            if out.status.success() {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn scutil_name() -> Option<String> {
    None
}

/// Is this "hostname" really just a MAC address (`de:e8:92:d6:ca:71`)?
///
/// That is what DHCP/reverse DNS hands back over a phone hotspot with "Private Wi-Fi Address" on
/// — see [`scutil_name`]. macOS is covered by asking `scutil` first, but Linux and Windows have no
/// equivalent, so reject the shape everywhere: reporting no name at all leaves the client on its
/// own default ("Device 2"), which beats naming the machine after a MAC address.
fn looks_like_mac_address(name: &str) -> bool {
    let sep = if name.contains(':') { ':' } else { '-' };
    let parts: Vec<&str> = name.split(sep).collect();
    parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The identity of this machine. Every client calls this one function (the desktop Tauri side
/// doesn't need it yet — the desktop shows itself, not "which one to pick"), so the relay method
/// and `/host_identity` route will never drift.
pub fn host_identity() -> HostIdentity {
    let hostname = scutil_name()
        .or_else(sysinfo::System::host_name)
        .map(|h| trim_host_suffix(&h))
        .filter(|h| !h.is_empty() && !looks_like_mac_address(h));
    HostIdentity {
        hostname,
        platform: std::env::consts::OS.to_string(),
        os_version: sysinfo::System::os_version().filter(|v| !v.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_only_known_lan_suffixes() {
        assert_eq!(trim_host_suffix("Hoveys-MacBook-Pro.local"), "Hoveys-MacBook-Pro");
        assert_eq!(trim_host_suffix("nas.lan"), "nas");
        assert_eq!(trim_host_suffix("  box  "), "box");
        // Not a suffix if it appears in the middle — `.local` is part of the name
        assert_eq!(trim_host_suffix("build.local.example"), "build.local.example");
        // Malformed name with only the suffix stays as-is, not truncated to empty string
        assert_eq!(trim_host_suffix(".local"), ".local");
    }

    #[test]
    fn recognizes_a_mac_address_masquerading_as_a_hostname() {
        assert!(looks_like_mac_address("de:e8:92:d6:ca:71"));
        assert!(looks_like_mac_address("DE-E8-92-D6-CA-71"));
        // Real names that merely contain hex or separators stay names
        assert!(!looks_like_mac_address("Harrys-MacBook-Pro"));
        assert!(!looks_like_mac_address("ab:cd:ef"));
        assert!(!looks_like_mac_address("de:e8:92:d6:ca:71:99"));
        assert!(!looks_like_mac_address("nas"));
    }

    #[test]
    fn identity_always_reports_a_platform() {
        let id = host_identity();
        assert!(!id.platform.is_empty());
        assert_eq!(id.platform, std::env::consts::OS);
        // Hostname is either absent, or non-empty and without `.local` suffix
        if let Some(h) = id.hostname {
            assert!(!h.is_empty());
            assert!(!h.ends_with(".local"));
        }
    }

    /// On macOS, identity name must come from `ComputerName`/`LocalHostName`, not from the
    /// network-drifting `gethostname()` — the latter becomes a MAC address string over hotspot.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_prefers_scutil_over_transient_hostname() {
        let Some(scutil) = scutil_name() else {
            return; // Machines without a configured ComputerName (very rare) are not required to pass
        };
        let id = host_identity();
        assert_eq!(id.hostname.as_deref(), Some(trim_host_suffix(&scutil).as_str()));
    }
}
