//! 这台主机是谁 —— 手机端给一台配对设备起名字用的那点信息。
//!
//! 手机的设备簿(`mobile-web/src/devices.ts`)此前只能把新配对的桌面端叫「设备
//! 1」「设备 2」:配对二维码里只有密钥和 relay 地址,主机名从来没有跨过这条线。
//! 一旦在册两台以上,那个序号就是纯噪音 —— 用户要选的是「家里那台 Mac」,而不是
//! 「设备 2」。
//!
//! 这里刻意与 [`crate::feature_flags::HostFeatures`] 分开:那边是**能力开关**
//! (客户端据此决定某个面存不存在),这里是**身份**(纯展示,不 gate 任何东西)。
//! 塞进同一个结构会让「拿不到就 fail closed」那条规则跨到一个不该 fail 的字段上。
//!
//! 对称物是手机侧的 `deviceLabel.ts`:那边由 UA 猜手机是什么,这边由主机自报。
//! 两边都只做展示,都允许「不知道」。

use serde::{Deserialize, Serialize};

/// 一台 Fleet 主机的身份,作为纯展示信息发给每个客户端。
///
/// 每一项都可缺席:容器里 `hostname` 可能是一串随机十六进制,某些系统上
/// `os_version` 拿不到。缺席时客户端退回平台名(「macOS」),而不是编一个。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    /// 主机名,已去掉 mDNS/局域网后缀(`.local` / `.lan`)。拿不到就是 `None`。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hostname: Option<String>,
    /// 平台键,取 `std::env::consts::OS`(`macos` / `windows` / `linux` …)。
    /// 客户端拿它挑图标,所以是机器可读的小写串而不是展示名。
    pub platform: String,
    /// 系统版本(`15.2`、`11`…)。展示用,拿不到就是 `None`。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub os_version: Option<String>,
}

/// 去掉主机名的 mDNS/局域网后缀。macOS 上 `hostname` 常是
/// `Hoveys-MacBook-Pro.local`,那个后缀在设备列表里只是噪音。
///
/// 只削**末尾**那一段,且只削这两个已知后缀 —— 一台真叫 `build.local.example`
/// 的机器不该被截成 `build`。
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

/// macOS 上问 `scutil` 要用户自己设的机器名。
///
/// `gethostname()`(也就是 `sysinfo::System::host_name()`)在 macOS 上返回的是
/// **临时主机名**:只要 `HostName` 没被 `scutil --set HostName` 钉死,系统就会拿
/// DHCP / 反向 DNS 回来的名字覆盖它。2026-09-16 在老板的 MacBook 上实测,走手机
/// 热点时对端把「私有 Wi-Fi 地址」当主机名回了过来,于是 `hostname` 变成
/// `de:e8:92:d6:ca:71`,手机设备簿里那台 Mac 的名字当场跟着变成了这串 MAC。
///
/// `ComputerName` 是用户在「系统设置 → 通用 → 关于本机」里起的名字,不随网络变;
/// `LocalHostName` 是它的 ASCII 化版本,作为第二选择。两个都拿不到才退回
/// `gethostname()`。
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

/// 本机身份。每个客户端问的都是这一个函数(Tauri 侧暂不需要 —— 桌面端展示的是
/// 它自己,没有「哪一台」要选),所以 relay 方法与 `/host_identity` 路由不会漂移。
pub fn host_identity() -> HostIdentity {
    let hostname = scutil_name()
        .or_else(sysinfo::System::host_name)
        .map(|h| trim_host_suffix(&h))
        .filter(|h| !h.is_empty());
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
        // 不是后缀就不动 —— 中间出现的 `.local` 是名字的一部分
        assert_eq!(trim_host_suffix("build.local.example"), "build.local.example");
        // 只剩后缀的怪名字保持原样,而不是被削成空字符串
        assert_eq!(trim_host_suffix(".local"), ".local");
    }

    #[test]
    fn identity_always_reports_a_platform() {
        let id = host_identity();
        assert!(!id.platform.is_empty());
        assert_eq!(id.platform, std::env::consts::OS);
        // 主机名要么缺席,要么非空且不带 .local 后缀
        if let Some(h) = id.hostname {
            assert!(!h.is_empty());
            assert!(!h.ends_with(".local"));
        }
    }

    /// macOS 上身份名必须来自 `ComputerName`/`LocalHostName`,而不是随网络漂移的
    /// `gethostname()` —— 后者在热点下会变成一串 MAC 地址。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_prefers_scutil_over_transient_hostname() {
        let Some(scutil) = scutil_name() else {
            return; // 没配 ComputerName 的机器(极少见)不强求
        };
        let id = host_identity();
        assert_eq!(id.hostname.as_deref(), Some(trim_host_suffix(&scutil).as_str()));
    }
}
