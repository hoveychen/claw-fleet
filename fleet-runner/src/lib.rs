//! Customer-hosted Fleet Runner protocol boundary.

pub mod protocol;
pub mod spool;

pub const RUNNER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn supported_protocol_versions() -> &'static [&'static str] {
    &[protocol::PROTOCOL_VERSION]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_one_spike_protocol() {
        assert_eq!(supported_protocol_versions(), &["2026-07-18"]);
    }
}
