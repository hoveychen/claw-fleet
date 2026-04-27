//! OAuth listener (Route D): localhost callback server + code → token
//! exchange.  Will own a `tiny_http` listener on
//! [`claw_fleet_core::feishu::FEISHU_OAUTH_PORT`].
//!
//! Skeleton: empty.  See `design/feishu-integration.md#oauth-flow`.
