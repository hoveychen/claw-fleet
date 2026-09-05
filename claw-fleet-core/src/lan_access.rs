//! "How does the phone in my hand reach this port" — the two things a
//! terminal needs to answer that: the machine's LAN address, and a QR code
//! small enough to scan off a scrollback buffer.
//!
//! Used by `fleet webui --lan`. Kept out of [`crate::mobile_relay`] on purpose:
//! that module's QR encodes a *pairing* URL for the cloud relay, this one
//! encodes a plain same-origin URL for a server on the local network. They
//! share only the encoder.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// The IPv4 address another device on the same network would use to reach this
/// machine, or `None` when there is no such address (offline, loopback only).
///
/// Found by asking the routing table rather than enumerating interfaces: a UDP
/// socket "connected" to an off-link address picks the interface the kernel
/// would actually route through, and its local address is that interface's IP.
/// Connecting a UDP socket sends no packets, so this neither needs the network
/// to be reachable nor tells 8.8.8.8 anything — it is a local routing lookup
/// wearing a socket. The alternative (getifaddrs / `ipconfig` parsing) means
/// platform-specific code plus a guess at which of several interfaces is the
/// one a phone can see.
///
/// The address is deliberately *not* cached: laptops change networks, and a
/// stale IP printed with confidence is worse than no IP at all.
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    // Any routable off-link address works; nothing is sent to it.
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("8.8.8.8:53").ok()?;
    match probe.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// A QR code for `text`, drawn with half-block characters (two QR rows per
/// text line) so a realistic URL still fits in an 80-column terminal.
///
/// Rendered light-on-dark — i.e. the *quiet* modules are the printed blocks —
/// because a terminal is usually dark and phone cameras need the code's dark
/// modules to be the darker of the two. On a light terminal the polarity is
/// wrong and the scan fails; that is the same trade every terminal QR tool
/// makes, and inverting is one flag away if it ever matters.
pub fn qr_terminal(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| format!("qr encode: {e}"))?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Light)
        .light_color(qrcode::render::unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_is_a_square_block_of_half_block_chars() {
        let art = qr_terminal("http://192.168.1.5:4571/m/").expect("encodes");
        let lines: Vec<&str> = art.lines().collect();
        assert!(lines.len() > 10, "unexpectedly short QR: {}", lines.len());
        // Every line is the same width — a ragged render means the caller's
        // terminal sees a broken code, not a QR.
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == width));
        // Half-block rendering: two QR rows per line, so the drawing is about
        // half as tall as it is wide.
        assert!(
            width >= lines.len(),
            "expected a wide-ish block, got {width}x{}",
            lines.len()
        );
        assert!(art.chars().any(|c| c == '█' || c == '▀' || c == '▄'));
    }

    #[test]
    fn dark_modules_render_as_terminal_background() {
        // The whole point of the inverted polarity: on a dark terminal the QR's
        // *dark* modules must be the unprinted ones. Checked at the top-left
        // finder pattern, whose first two rows at x=0 are both dark — with a
        // 4-module quiet zone that is line 2 (two module rows per line),
        // column 4, and it has to be blank. If this flips, every scan fails
        // while the code still "looks like a QR" in the terminal.
        let art = qr_terminal("http://192.168.1.5:4571/m/").expect("encodes");
        let line = art.lines().nth(2).expect("at least 3 lines");
        let chars: Vec<char> = line.chars().collect();
        assert_eq!(chars[3], '█', "quiet zone must be printed (light) blocks");
        assert_eq!(chars[4], ' ', "finder pattern corner must be background");
    }

    #[test]
    fn qr_rejects_input_too_large_to_encode() {
        // Well past the largest QR version's capacity — the encoder must say
        // so rather than panic, since the URL comes from user config.
        let huge = "x".repeat(8000);
        assert!(qr_terminal(&huge).is_err());
    }

    #[test]
    fn lan_ipv4_is_never_loopback() {
        // CI boxes and offline laptops both exist, so the address itself is
        // not assertable — but whatever comes back must be usable by another
        // host, which loopback and 0.0.0.0 are not.
        if let Some(ip) = lan_ipv4() {
            assert!(!ip.is_loopback());
            assert!(!ip.is_unspecified());
        }
    }
}
