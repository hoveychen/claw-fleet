//! One place that answers "which decision cards are outstanding right now".
//!
//! Every client needs this same union, and each one used to build it itself:
//! `LocalBackend::list_pending_decisions` for the desktop, and six separate
//! `/…/pending` route handlers for `fleet serve` / `fleet webui` that the
//! browser build fanned out to and stitched back together in TypeScript. Three
//! copies of "channel dir + parked store, then fill in display fields" is three
//! places for a channel to be forgotten — the `permissionPrompt` bucket was
//! added to two of them and read as `undefined` in the third for a while.
//!
//! The union is a cheap read (six directory listings plus the parked store), so
//! it is fine to ask for it on a timer. That matters: the frontend reconciles
//! against it continuously now rather than trusting a one-shot push, which is
//! what makes a lost SSE frame self-healing instead of a card nobody ever sees.

use crate::parked::{self, ParkedKind};
use crate::session::SessionInfo;
use crate::ui_types::PendingDecisions;

/// Every request awaiting an answer, display fields resolved against
/// `sessions`.
///
/// `guard` and `permission_prompt` have no parked arm on purpose: both are
/// answered by a hook CLI that is still blocking on its own poll, so a timeout
/// there ends the request rather than outliving its producer.
pub fn collect(sessions: &[SessionInfo]) -> PendingDecisions {
    let mut pending = PendingDecisions {
        guard: crate::guard::list_pending_requests()
            .iter()
            .filter_map(|id| crate::guard::read_request(id))
            .collect(),
        elicitation: crate::elicitation::list_pending_requests()
            .iter()
            .filter_map(|id| crate::elicitation::read_request(id))
            // Cards whose wait timed out live in the parked store, not in the
            // channel's request dir — the producer that was blocking on them is
            // long gone. They keep showing up here, flagged `parked`, until the
            // user actually resolves them.
            .chain(parked::list_requests(ParkedKind::Elicitation))
            .collect(),
        fleet_ask: crate::mcp_ipc::list_pending_requests()
            .iter()
            .filter_map(|id| crate::mcp_ipc::read_request(id))
            .chain(parked::list_requests(ParkedKind::FleetAsk))
            .collect(),
        a2ui_render: crate::mcp_a2ui_ipc::list_pending_requests()
            .iter()
            .filter_map(|id| crate::mcp_a2ui_ipc::read_request(id))
            .chain(parked::list_requests(ParkedKind::A2uiRender))
            .collect(),
        plan_approval: crate::plan_approval::list_pending_requests()
            .iter()
            .filter_map(|id| crate::plan_approval::read_request(id))
            .chain(parked::list_requests(ParkedKind::PlanApproval))
            .collect(),
        permission_prompt: crate::permission_prompt_ipc::list_pending_requests()
            .iter()
            .filter_map(|id| crate::permission_prompt_ipc::read_request(id))
            .collect(),
    };
    crate::ui_types::resolve_pending_display(&mut pending, sessions);
    pending
}
