//! 给通知的点击目标盖上「这条来自哪个 channel」。
//!
//! 一部手机可以同时配对**多台**桌面端(每台一个 channel),而 Web Push 到达时
//! service worker 手里只有 payload:它不知道这条通知是哪台机器发的。桌面端自己
//! 产出的 url 只带卡的 id(如 `/#d=guard:g1`),而卡 id 只在单机内唯一 —— 两台
//! 同时有卡时,点开落到哪一张是不确定的。
//!
//! 补这个缺口的地方只能是 relay:它是唯一在扇出时**确切知道** channel 的一方
//! (`Push::notify(channel, …)`),而且这样不需要桌面端配合改任何东西。
//!
//! 盖的是 channel id 的前缀而不是完整 id:前缀足够在一部手机配对过的那几台之间
//! 区分(手机拿自己的 secret 派生 channel token 再 sha256 就能比对),而通知
//! payload 会落在系统通知中心里,少带一点身份信息就少一点暴露面。

/// 盖进 url 的 channel id 前缀长度(十六进制字符数)。
///
/// 4 字节 = 32 位。它要区分的只是「这部手机配对过的那几台」——量级是个位数,
/// 32 位远远够;而它不是任何安全边界(channel id 本来就是 relay 的路由键,
/// 手机自己也能算出来),所以不必更长。
const CHANNEL_MARK_LEN: usize = 8;

/// 通知点击目标的 fragment 参数名。手机端按它反查是哪一台设备。
pub const CHANNEL_PARAM: &str = "ch";

/// 把 channel 标记盖进点击目标。
///
/// * `url` 为 `None` 时返回 `None` —— 没有目标就没有「落到哪一张」的问题。
/// * url 里已经有 fragment 时作为 fragment 参数追加(`/#d=guard:g1&ch=…`);
///   没有则起一个(`/#ch=…`)。移动端的路由本来就全在 fragment 里(见
///   `mobile-web/src/decisionDeepLink.ts`),所以不碰 query —— query 会进服务端
///   日志,而 fragment 不会。
/// * 已经带过 `ch=` 的 url 原样返回:重复盖会让手机端读到两个互相矛盾的来源。
pub fn stamp_channel(url: Option<&str>, channel: &str) -> Option<String> {
    let url = url?;
    if url.contains(&format!("{CHANNEL_PARAM}=")) {
        return Some(url.to_string());
    }
    let mark = &channel[..channel.len().min(CHANNEL_MARK_LEN)];
    let sep = if url.contains('#') { '&' } else { '#' };
    Some(format!("{url}{sep}{CHANNEL_PARAM}={mark}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNEL: &str = "105e300f7fde4b1c9a0d8e7f6a5b4c3d2e1f00112233445566778899aabbccdd";

    #[test]
    fn stamps_a_fragment_target() {
        assert_eq!(
            stamp_channel(Some("/#d=guard:g1"), CHANNEL).unwrap(),
            "/#d=guard:g1&ch=105e300f"
        );
    }

    #[test]
    fn starts_a_fragment_when_the_target_has_none() {
        assert_eq!(stamp_channel(Some("/"), CHANNEL).unwrap(), "/#ch=105e300f");
    }

    /// 没有目标就没有歧义要解决。
    #[test]
    fn leaves_a_targetless_notification_alone() {
        assert_eq!(stamp_channel(None, CHANNEL), None);
    }

    /// 重复盖会让手机端读到两个互相矛盾的来源。
    #[test]
    fn is_idempotent() {
        let once = stamp_channel(Some("/#d=guard:g1"), CHANNEL).unwrap();
        assert_eq!(stamp_channel(Some(&once), CHANNEL).unwrap(), once);
    }

    /// 两个 channel 必须盖出不同的标记 —— 否则整件事白做。
    #[test]
    fn different_channels_stamp_differently() {
        let a = stamp_channel(Some("/#d=guard:g1"), CHANNEL).unwrap();
        let b = stamp_channel(
            Some("/#d=guard:g1"),
            "16378d5f072a4e8b1c2d3e4f5a6b7c8d9e0f11223344556677889900aabbccdd",
        )
        .unwrap();
        assert_ne!(a, b);
    }

    /// 短 id(测试里手写的、或将来换了摘要算法)不该 panic 在切片上。
    #[test]
    fn tolerates_a_short_channel_id() {
        assert_eq!(stamp_channel(Some("/"), "abc").unwrap(), "/#ch=abc");
    }
}
