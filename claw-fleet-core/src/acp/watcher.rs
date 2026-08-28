//! Pushing Fleet's decision cards to the ACP client, and writing answers back.
//!
//! # Why a watcher rather than a projection
//!
//! A decision card is produced by a *different process* — the `fleet guard`
//! hook, the MCP server, a plan-approval hook — which drops a request file in
//! `~/.fleet/` and blocks waiting for the answer. The Responses surface could
//! only surface those by including them in whatever poll happened next, because
//! HTTP gave it no way to speak first.
//!
//! ACP is bidirectional, so the agent can originate the question. This watcher
//! is what turns "a file appeared on disk" into "the client is being asked",
//! and the answer back into the response file that unblocks the waiting
//! process.
//!
//! # Threading
//!
//! One watcher thread per connection, polling the stores; each card it finds
//! gets its own thread, because asking the client blocks until a human answers
//! and several cards can legitimately be open at once. The watcher itself never
//! blocks on an answer.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use super::agent::AcpAgent;
use super::decisions;
use super::types::{
    CreateElicitationRequest, ElicitationAction, RequestPermissionRequest,
    RequestPermissionResponse,
};

/// How often the stores are checked.
///
/// Fast enough that a card does not feel stuck, cheap enough to run per
/// connection: each tick is a directory listing over a handful of small dirs.
const POLL_INTERVAL: Duration = Duration::from_millis(400);

/// How long a card waits for the human before giving up.
///
/// Matches Fleet's own parking behaviour: the asking process eventually times
/// out and the card is parked for later, so blocking here forever would leak a
/// thread per abandoned question.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Start the per-connection decision watcher. Returns immediately.
pub fn spawn(agent: Arc<AcpAgent>) {
    let _ = std::thread::Builder::new().name("acp-decisions".into()).spawn(move || {
        let mut seen: HashSet<String> = HashSet::new();
        loop {
            if agent.is_closed() {
                return;
            }
            // Say out loud that someone is watching the stores. `fleet guard`,
            // `fleet elicitation` and the `fleet__ask` MCP tool all check this
            // heartbeat before parking a card and refuse outright when it is
            // missing — so without it, an ACP-only head (a cloud container, a
            // phone over the websocket) never receives a single card: they are
            // rejected at the asking end, before this loop could route them.
            // `fleet serve` writes the same heartbeat for SSE and mobile-relay
            // clients; an ACP connection is a third surface with equal claim.
            crate::consumer_heartbeat::write_heartbeat();

            for card in pending_cards() {
                if seen.contains(&card.id) {
                    continue;
                }
                // Only cards belonging to a session this connection drives.
                let Some(acp_session) = agent.acp_session_for_internal(&card.session_id) else {
                    continue;
                };
                seen.insert(card.id.clone());
                let agent = agent.clone();
                let _ = std::thread::Builder::new()
                    .name("acp-decision".into())
                    .spawn(move || ask(&agent, &acp_session, card));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

/// A pending card, reduced to what the watcher needs to route it.
pub struct Card {
    pub id: String,
    pub session_id: String,
    pub kind: CardKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Guard,
    PermissionPrompt,
    Elicitation,
    FleetAsk,
    PlanApproval,
}

/// Every card currently waiting for a human, across all stores.
///
/// Includes the parked variants: a card whose asking process already timed out
/// is still worth showing, and answering it resumes the session rather than
/// unblocking a poll that is long gone.
fn pending_cards() -> Vec<Card> {
    let mut out = Vec::new();
    let mut push = |id: String, session_id: String, kind: CardKind| {
        if !session_id.is_empty() {
            out.push(Card { id, session_id, kind });
        }
    };

    for id in crate::guard::list_pending_requests() {
        if let Some(r) = crate::guard::read_request(&id) {
            push(id, r.session_id, CardKind::Guard);
        }
    }
    for id in crate::permission_prompt_ipc::list_pending_requests() {
        if let Some(r) = crate::permission_prompt_ipc::read_request(&id) {
            push(id, r.session_id, CardKind::PermissionPrompt);
        }
    }
    for id in crate::elicitation::list_pending_requests() {
        if let Some(r) = crate::elicitation::read_request(&id) {
            push(id, r.session_id, CardKind::Elicitation);
        }
    }
    for id in crate::mcp_ipc::list_pending_requests() {
        if let Some(r) = crate::mcp_ipc::read_request(&id) {
            push(id, r.session_id, CardKind::FleetAsk);
        }
    }
    for id in crate::plan_approval::list_pending_requests() {
        if let Some(r) = crate::plan_approval::read_request(&id) {
            push(id, r.session_id, CardKind::PlanApproval);
        }
    }
    // Parked cards: the producer gave up waiting, the question did not.
    for r in crate::parked::list_requests::<crate::elicitation::ElicitationRequest>(
        crate::parked::ParkedKind::Elicitation,
    ) {
        push(r.id.clone(), r.session_id.clone(), CardKind::Elicitation);
    }
    for r in crate::parked::list_requests::<crate::mcp_ipc::FleetAskRequest>(
        crate::parked::ParkedKind::FleetAsk,
    ) {
        push(r.id.clone(), r.session_id.clone(), CardKind::FleetAsk);
    }
    for r in crate::parked::list_requests::<crate::plan_approval::PlanApprovalRequest>(
        crate::parked::ParkedKind::PlanApproval,
    ) {
        push(r.id.clone(), r.session_id.clone(), CardKind::PlanApproval);
    }
    out
}

/// Ask the client about one card and write the answer back. Blocks.
fn ask(agent: &AcpAgent, acp_session: &str, card: Card) {
    match card.kind {
        CardKind::Guard => ask_guard(agent, acp_session, &card.id),
        CardKind::PermissionPrompt => ask_permission_prompt(agent, acp_session, &card.id),
        CardKind::Elicitation => ask_elicitation(agent, acp_session, &card.id),
        CardKind::FleetAsk => ask_fleet_ask(agent, acp_session, &card.id),
        CardKind::PlanApproval => ask_plan(agent, acp_session, &card.id),
    }
}

fn ask_guard(agent: &AcpAgent, acp_session: &str, id: &str) {
    let Some(req) = crate::guard::read_request(id) else { return };
    let mut ask = decisions::guard_to_permission(acp_session, &req);
    // The title has nowhere to live on ToolCallUpdate's required fields, so it
    // rides in the content the client renders above the choices.
    ask.tool_call.content =
        Some(vec![super::types::ToolCallContent::text(decisions::guard_title(&req))]);

    let Some(outcome) = request_permission(agent, ask) else { return };
    // Allow or Block; see `decisions::guard_to_permission` for why no
    // "remember" option is offered.
    let decision = if decisions::outcome_allows(&outcome) {
        crate::guard::GuardDecision::Allow
    } else {
        crate::guard::GuardDecision::Block
    };
    let _ = crate::guard::write_response(&crate::guard::GuardResponse {
        id: id.to_string(),
        decision,
        reason: None,
    });
}

fn ask_permission_prompt(agent: &AcpAgent, acp_session: &str, id: &str) {
    let Some(req) = crate::permission_prompt_ipc::read_request(id) else { return };
    let ask = decisions::permission_prompt_to_permission(acp_session, &req);
    let Some(outcome) = request_permission(agent, ask) else { return };
    let allow = decisions::outcome_allows(&outcome);
    let _ = crate::permission_prompt_ipc::write_response(
        &crate::permission_prompt_ipc::PermissionPromptResponse {
            id: id.to_string(),
            decision: if allow {
                crate::permission_prompt_ipc::PermissionPromptDecision::Allow
            } else {
                crate::permission_prompt_ipc::PermissionPromptDecision::Deny
            },
            // Fleet allows editing the tool input on approval; ACP's outcome
            // carries only an option id, so the input goes through unchanged.
            reason: None,
        },
    );
}

fn ask_elicitation(agent: &AcpAgent, acp_session: &str, id: &str) {
    let Some(req) = crate::elicitation::read_request(id).or_else(|| {
        crate::parked::list_requests::<crate::elicitation::ElicitationRequest>(
            crate::parked::ParkedKind::Elicitation,
        )
        .into_iter()
        .find(|r| r.id == id)
    }) else {
        return;
    };
    if !agent.client_supports_elicitation_form() {
        return leave_for_another_surface(agent, acp_session);
    }
    let ask = decisions::elicitation_to_form(acp_session, &req);
    let Some(action) = create_elicitation(agent, ask) else { return };
    let resp = decisions::form_answer_to_elicitation(id, &action);
    let _ = crate::parked::deliver(
        &resp.id,
        &resp,
        resp.declined,
        crate::elicitation::write_response,
    );
}

fn ask_fleet_ask(agent: &AcpAgent, acp_session: &str, id: &str) {
    let Some(req) = crate::mcp_ipc::read_request(id).or_else(|| {
        crate::parked::list_requests::<crate::mcp_ipc::FleetAskRequest>(
            crate::parked::ParkedKind::FleetAsk,
        )
        .into_iter()
        .find(|r| r.id == id)
    }) else {
        return;
    };

    // A card with `html`/`images` wants URL mode, but only if the client said
    // it supports it — the spec forbids using it otherwise.
    let rich = req.questions.iter().any(decisions::has_rich_preview);
    let caps = agent.client_capabilities();
    let ask = match decisions::choose_delivery(rich, &caps) {
        decisions::Delivery::Url => match decisions::preview_url(&req.id, 0) {
            Some(url) => decisions::fleet_ask_to_url(acp_session, &req, url),
            // No public base URL configured, so there is nothing to link to.
            None => decisions::fleet_ask_to_form(acp_session, &req),
        },
        decisions::Delivery::Form => decisions::fleet_ask_to_form(acp_session, &req),
        decisions::Delivery::Unsupported => return leave_for_another_surface(agent, acp_session),
    };
    let Some(action) = create_elicitation(agent, ask) else { return };
    let resp = decisions::form_answer_to_fleet_ask(id, &action);
    let _ =
        crate::parked::deliver(&resp.id, &resp, resp.cancelled, crate::mcp_ipc::write_response);
}

fn ask_plan(agent: &AcpAgent, acp_session: &str, id: &str) {
    let Some(req) = crate::plan_approval::read_request(id).or_else(|| {
        crate::parked::list_requests::<crate::plan_approval::PlanApprovalRequest>(
            crate::parked::ParkedKind::PlanApproval,
        )
        .into_iter()
        .find(|r| r.id == id)
    }) else {
        return;
    };
    if !agent.client_supports_elicitation_form() {
        return leave_for_another_surface(agent, acp_session);
    }
    let ask = decisions::plan_approval_to_form(acp_session, &req);
    let Some(action) = create_elicitation(agent, ask) else { return };
    let resp = decisions::form_answer_to_plan(id, &action);
    let _ = crate::parked::deliver(&resp.id, &resp, false, crate::plan_approval::write_response);
}

/// Tell the client a question exists that it cannot host, and leave the card
/// open.
///
/// The alternative — answering on the user's behalf — would either approve
/// something nobody saw or reject work for a reason that is not the user's.
/// Fleet's desktop and mobile surfaces can still answer it.
fn leave_for_another_surface(agent: &AcpAgent, acp_session: &str) {
    agent.notify_session(
        acp_session,
        super::types::SessionUpdate::agent_text(
            "A Fleet decision card is waiting, but this client did not advertise \
             elicitation support. Answer it in the Fleet desktop or mobile app.",
        ),
    );
}

/// Send `session/request_permission` and wait. `None` when the client never
/// answered or answered unintelligibly — the card is then left for another
/// surface (the desktop, the phone) to handle rather than being force-denied.
fn request_permission(
    agent: &AcpAgent,
    req: RequestPermissionRequest,
) -> Option<super::types::RequestPermissionOutcome> {
    let params = serde_json::to_value(&req).ok()?;
    let raw = agent.request_client("session/request_permission", params, ANSWER_TIMEOUT).ok()?;
    serde_json::from_value::<RequestPermissionResponse>(raw).ok().map(|r| r.outcome)
}

/// Send `elicitation/create` and wait. `None` on the same terms as
/// [`request_permission`].
fn create_elicitation(
    agent: &AcpAgent,
    req: CreateElicitationRequest,
) -> Option<ElicitationAction> {
    let params = serde_json::to_value(&req).ok()?;
    let raw = agent.request_client("elicitation/create", params, ANSWER_TIMEOUT).ok()?;
    serde_json::from_value::<ElicitationAction>(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fleet guard` / `fleet elicitation` / the `fleet__ask` MCP tool all
    /// refuse to park a card unless a consumer is heartbeating — otherwise
    /// they would block an agent on a question nobody will ever see.
    ///
    /// `fleet serve` writes that heartbeat only while an SSE client or the
    /// mobile relay is attached. An ACP connection is neither, so on a head
    /// whose only surface is ACP every card was rejected at the source:
    /// `fleet__ask` came back "Fleet consumer not running (status:
    /// file-unreadable)" and the agent fell back to plain text. Observed on a
    /// real phone-driven turn on 2026-08-27.
    ///
    /// This watcher genuinely polls every store, so while it runs it *is* a
    /// consumer and has to say so.
    #[test]
    fn a_running_watcher_announces_itself_as_a_decision_consumer() {
        let _guard = crate::session::fleet_home_lock();
        let home = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", home.path());

        // Nothing has beaten yet.
        assert!(
            !crate::consumer_heartbeat::consumer_status(Duration::from_secs(30)).is_alive(),
            "precondition: an isolated home starts with no consumer"
        );

        let agent = AcpAgent::new(
            Arc::new(crate::acp::jsonrpc::Peer::new(Box::new(SilentSink))),
            Arc::new(Vec::new()),
        );
        spawn(Arc::new(agent));

        let mut alive = false;
        for _ in 0..40 {
            if crate::consumer_heartbeat::consumer_status(Duration::from_secs(30)).is_alive() {
                alive = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(alive, "a live ACP connection must count as a decision-card consumer");

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
    }

    struct SilentSink;
    impl crate::acp::jsonrpc::Sink for SilentSink {
        fn send(&self, _frame: &str) -> bool {
            true
        }
    }

    #[test]
    fn cards_without_a_session_are_not_routed() {
        // A card with no session id cannot be attributed to a connection, and
        // pushing it to whichever client happened to be listening would show
        // one customer another's question.
        let mut out = Vec::new();
        let mut push = |id: String, session_id: String, kind: CardKind| {
            if !session_id.is_empty() {
                out.push(Card { id, session_id, kind });
            }
        };
        push("a".into(), String::new(), CardKind::Guard);
        push("b".into(), "s1".into(), CardKind::Guard);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "b");
    }

    #[test]
    fn a_permission_response_parses_from_the_wire_shape() {
        let raw = serde_json::json!({"outcome": {"outcome": "selected", "optionId": "allow"}});
        let parsed: RequestPermissionResponse = serde_json::from_value(raw).unwrap();
        assert!(decisions::outcome_allows(&parsed.outcome));
    }

    #[test]
    fn an_elicitation_response_parses_all_three_actions() {
        let accept: ElicitationAction =
            serde_json::from_value(serde_json::json!({"action": "accept", "content": {"a": "b"}}))
                .unwrap();
        assert!(!accept.is_refusal());
        assert_eq!(accept.content()["a"], "b");

        for action in ["decline", "cancel"] {
            let a: ElicitationAction =
                serde_json::from_value(serde_json::json!({"action": action})).unwrap();
            assert!(a.is_refusal(), "{action}");
        }
    }

    #[test]
    fn an_unintelligible_answer_is_not_read_as_a_decision() {
        // Neither a denial nor an approval: the card stays open for another
        // surface to answer.
        assert!(serde_json::from_value::<RequestPermissionResponse>(
            serde_json::json!({"outcome": "yes"})
        )
        .is_err());
        assert!(serde_json::from_value::<ElicitationAction>(serde_json::json!({"action": "maybe"}))
            .is_err());
    }
}
