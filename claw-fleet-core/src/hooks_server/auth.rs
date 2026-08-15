//! Request authorization for `fleet serve`.
//!
//! Two token tiers back the Fleet Cloud lean deployment:
//!
//! - **admin token** — the existing single `fleet serve --token`. Full access
//!   to every route. Used by the desktop app, RemoteBackend, mobile relay and
//!   any first-party caller.
//! - **scoped (public) token** — optional, supplied via `FLEET_PUBLIC_TOKEN`.
//!   This is what an *external* customer's integrating service presents. It may
//!   reach ONLY the curated public surface ([`crate::routes::is_public`]);
//!   every other path (proc exec, settings, guidance, file browse, credential
//!   surfaces …) is denied so provider credentials and host internals stay
//!   invisible to the customer.
//!
//! Keeping the decision in one pure function makes the security boundary unit
//! testable — see the tests below.

/// Outcome of an authorization check for a single request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// Full-access admin token — any route.
    Admin,
    /// Valid scoped token AND the path is on the public whitelist.
    Scoped,
    /// No/unknown token, or a scoped token reaching a non-public path.
    Denied,
}

impl AuthOutcome {
    pub fn is_allowed(self) -> bool {
        !matches!(self, AuthOutcome::Denied)
    }
}

/// Decide whether a request is authorized.
///
/// `presented` is the bare token the caller supplied (from the
/// `Authorization: Bearer <t>` header or the `?token=<t>` query param, already
/// stripped of the `Bearer ` prefix). `public_token` is `None` when scoped
/// access is not configured (or configured empty), which disables the whole
/// scoped tier.
pub fn authorize(
    path: &str,
    presented: Option<&str>,
    admin_token: &str,
    public_token: Option<&str>,
) -> AuthOutcome {
    let presented = match presented {
        Some(t) => t,
        None => return AuthOutcome::Denied,
    };

    // Admin wins regardless of path. Guard against an empty admin token
    // accidentally matching an empty `?token=`.
    if !admin_token.is_empty() && presented == admin_token {
        return AuthOutcome::Admin;
    }

    // Scoped tier: only when a non-empty public token is configured, the
    // presented token matches it, AND the path is on the public whitelist.
    if let Some(pt) = public_token {
        if !pt.is_empty() && presented == pt && crate::routes::is_public(path) {
            return AuthOutcome::Scoped;
        }
    }

    AuthOutcome::Denied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes;

    const ADMIN: &str = "admin-secret";
    const PUBLIC: &str = "cust-scoped-secret";

    #[test]
    fn admin_token_reaches_any_path_including_internal() {
        // A public path and a dangerous internal one both succeed for admin.
        assert_eq!(
            authorize(routes::SPAWN_SESSION, Some(ADMIN), ADMIN, Some(PUBLIC)),
            AuthOutcome::Admin
        );
        assert_eq!(
            authorize(routes::PROC_RUN, Some(ADMIN), ADMIN, Some(PUBLIC)),
            AuthOutcome::Admin
        );
        assert_eq!(
            authorize(routes::APPLY_GUARD_HOOK, Some(ADMIN), ADMIN, Some(PUBLIC)),
            AuthOutcome::Admin
        );
    }

    #[test]
    fn scoped_token_reaches_public_paths() {
        // v2: the scoped surface is the /v1/* tree (+ the health check).
        for p in [
            routes::HEALTH,
            "/v1/responses",
            "/v1/responses/resp_abc",
            "/v1/responses/resp_abc/cancel",
            "/v1/responses/resp_abc/files",
            "/v1/files/file_xyz/content",
        ] {
            assert_eq!(
                authorize(p, Some(PUBLIC), ADMIN, Some(PUBLIC)),
                AuthOutcome::Scoped,
                "expected scoped access to public path {p}"
            );
        }
    }

    #[test]
    fn scoped_token_denied_on_internal_paths() {
        // The whole point: a customer token must NOT reach command exec,
        // settings, guidance, filesystem browse or credential surfaces.
        for p in [
            routes::PROC_RUN,
            // Spawns a proc on the host just like PROC_RUN, so it must sit on
            // the same side of the boundary — and registering a browse path
            // widens what the explorer will read.
            routes::GIT_CLONE_STREAM,
            routes::BROWSE_PATHS_ADD,
            routes::APPLY_GUARD_HOOK,
            routes::EXPLORER_FILE,
            // Same read power as EXPLORER_FILE with no workspace gate at all.
            routes::EXPLORER_EXTERNAL_FILE,
            routes::BROWSE_DIR,
            routes::LLM_CONFIG,
            routes::SOURCES_CONFIG,
            routes::MEMORIES,
            routes::SKILL_CONTENT,
            routes::REMOTE_WORKSPACES,
        ] {
            assert_eq!(
                authorize(p, Some(PUBLIC), ADMIN, Some(PUBLIC)),
                AuthOutcome::Denied,
                "scoped token must be denied on internal path {p}"
            );
        }
    }

    #[test]
    fn scoped_token_denied_on_v1_replaced_raw_routes() {
        // v2 P6 confinement: the raw internal routes the v1 lean whitelist used
        // to expose are now admin-only. A scoped token hitting /spawn_session
        // (etc.) is Denied → the audit holes (unconstrained workspacePath, raw
        // SessionInfo leak) are unreachable by construction.
        for p in [
            routes::SPAWN_SESSION,
            routes::RESUME_SESSION,
            routes::SESSIONS,
            routes::TAIL,
            routes::MESSAGES,
            routes::LIVE_THINKING,
            routes::ENQUEUE_MESSAGE,
            routes::INTERRUPT,
            routes::STOP,
            routes::GUARD_RESPOND,
            routes::FLEET_ASK_RESPOND,
            routes::USER_ATTACHMENT,
            routes::USAGE_SUMMARIES,
            "/events",
        ] {
            assert!(
                !routes::is_public(p),
                "v1 raw route {p} must be off the v2 scoped whitelist"
            );
            assert_eq!(
                authorize(p, Some(PUBLIC), ADMIN, Some(PUBLIC)),
                AuthOutcome::Denied,
                "scoped token must be denied on v1-replaced raw route {p}"
            );
        }
    }

    #[test]
    fn scoped_token_cannot_reach_credential_surfaces() {
        // Invariant guard for the lean cred-isolation model: a scoped customer
        // token must NEVER reach a route that could read provider credentials
        // out of the container. If someone later adds one of these to
        // routes::is_public, this test fails on purpose.
        for p in [
            routes::EXPLORER_FILE,          // reads an arbitrary path
            routes::EXPLORER_EXTERNAL_FILE, // reads an arbitrary path, ungated
            routes::SCRATCHPAD_FILE,        // reads an arbitrary scratchpad path
            routes::PROC_RUN,               // arbitrary command exec
            routes::PROC_OUTPUT,            // output of arbitrary command
            routes::BROWSE_DIR,             // directory listing
            routes::SOURCES_CONFIG,         // provider source config
            routes::SOURCES_CLAUDE_ACCOUNT, // provider account detail
            "/sources/claude/account",      // SOURCES_PREFIX arm → account/creds info
            "/sources/codex/account",
        ] {
            assert!(
                !routes::is_public(p),
                "credential-adjacent route {p} must not be on the public whitelist"
            );
            assert_eq!(
                authorize(p, Some(PUBLIC), ADMIN, Some(PUBLIC)),
                AuthOutcome::Denied,
                "scoped token must be denied on credential-adjacent route {p}"
            );
        }
    }

    #[test]
    fn no_token_is_denied() {
        assert_eq!(
            authorize(routes::SPAWN_SESSION, None, ADMIN, Some(PUBLIC)),
            AuthOutcome::Denied
        );
    }

    #[test]
    fn wrong_token_is_denied() {
        assert_eq!(
            authorize(routes::SPAWN_SESSION, Some("nope"), ADMIN, Some(PUBLIC)),
            AuthOutcome::Denied
        );
    }

    #[test]
    fn scoped_tier_disabled_when_no_public_token() {
        // Without a configured public token, only the admin token works —
        // the scoped secret string is meaningless.
        assert_eq!(
            authorize(routes::SPAWN_SESSION, Some(PUBLIC), ADMIN, None),
            AuthOutcome::Denied
        );
        assert_eq!(
            authorize(routes::SPAWN_SESSION, Some(ADMIN), ADMIN, None),
            AuthOutcome::Admin
        );
    }

    #[test]
    fn empty_public_token_does_not_enable_scoped_tier() {
        // An empty presented token must not match an empty configured one.
        assert_eq!(
            authorize(routes::SPAWN_SESSION, Some(""), ADMIN, Some("")),
            AuthOutcome::Denied
        );
    }

    #[test]
    fn empty_admin_token_never_matches() {
        assert_eq!(
            authorize(routes::SPAWN_SESSION, Some(""), "", Some(PUBLIC)),
            AuthOutcome::Denied
        );
    }
}
