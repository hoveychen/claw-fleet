//! Stamp notification click targets with "which channel this came from".
//!
//! One phone can pair with **multiple** desktops at once (one channel each).
//! When a Web Push arrives, the service worker only has the payload: it doesn't
//! know which desktop sent it. Each desktop generates URLs with just the card's
//! id (e.g. `/#d=guard:g1`), and card ids are only unique per device — when two
//! desktops both have a card, it's ambiguous which one you're clicking.
//!
//! Only the relay can fill this gap: it's the only party that **knows which
//! channel** when fanning out (`Push::notify(channel, …)`), and doing this doesn't
//! require any desktop changes.
//!
//! We stamp the channel id's prefix, not the full id: a prefix is enough to
//! distinguish between the few devices a phone has paired with (the phone can
//! derive its own channel token from its secret and sha256 to compare), and
//! keeping the notification payload smaller in the system notification center
//! reduces the exposed surface area.

/// Channel id prefix length to stamp into the URL (hex characters).
///
/// 4 bytes = 32 bits. We only need to distinguish between the few devices a
/// phone has paired with — single-digit scale, so 32 bits is more than enough.
/// It's not a security boundary (channel ids are relay routing keys anyway and
/// the phone can compute them too), so no need for a longer prefix.
const CHANNEL_MARK_LEN: usize = 8;

/// Fragment parameter name for the notification click target. Mobile uses it to
/// reverse-lookup which device the notification came from.
pub const CHANNEL_PARAM: &str = "ch";

/// Stamp the channel mark into the click target.
///
/// * When `url` is `None`, returns `None` — with no target, there's no
///   "which card" ambiguity to solve.
/// * If the url already has a fragment, appends the mark as a fragment parameter
///   (`/#d=guard:g1&ch=…`); otherwise creates one (`/#ch=…`). Mobile routing
///   already lives entirely in the fragment (see `mobile-web/src/decisionDeepLink.ts`),
///   so we avoid the query string — query gets logged server-side, fragment doesn't.
/// * If the url already has `ch=`, returns it unchanged: duplicate stamping would
///   give mobile two conflicting sources.
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

    /// No target means no ambiguity to resolve.
    #[test]
    fn leaves_a_targetless_notification_alone() {
        assert_eq!(stamp_channel(None, CHANNEL), None);
    }

    /// Duplicate stamping would make mobile read conflicting sources.
    #[test]
    fn is_idempotent() {
        let once = stamp_channel(Some("/#d=guard:g1"), CHANNEL).unwrap();
        assert_eq!(stamp_channel(Some(&once), CHANNEL).unwrap(), once);
    }

    /// Two channels must stamp different marks — otherwise this whole thing is pointless.
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

    /// Short channel ids (hand-written in tests or from future hash algorithms)
    /// shouldn't panic when slicing.
    #[test]
    fn tolerates_a_short_channel_id() {
        assert_eq!(stamp_channel(Some("/"), "abc").unwrap(), "/#ch=abc");
    }
}
