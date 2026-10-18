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
use crate::task_model::DecisionTaskRef;
use crate::ui_types::PendingDecisions;

/// The display fields every decision request carries, so one function can
/// fill them for all six kinds — the live-emit paths and the pending snapshot
/// used to each hand-copy the same block.
pub trait DecisionCard {
    fn session_id(&self) -> &str;
    fn display_fields(&mut self) -> (&mut String, &mut Option<String>, &mut Option<DecisionTaskRef>);
}

macro_rules! impl_decision_card {
    ($($t:ty),* $(,)?) => {$(
        impl DecisionCard for $t {
            fn session_id(&self) -> &str {
                &self.session_id
            }
            fn display_fields(
                &mut self,
            ) -> (&mut String, &mut Option<String>, &mut Option<DecisionTaskRef>) {
                (&mut self.workspace_name, &mut self.ai_title, &mut self.task)
            }
        }
    )*};
}

impl_decision_card!(
    crate::guard::GuardRequest,
    crate::elicitation::ElicitationRequest,
    crate::mcp_ipc::FleetAskRequest,
    crate::mcp_a2ui_ipc::A2uiRenderRequest,
    crate::plan_approval::PlanApprovalRequest,
    crate::permission_prompt_ipc::PermissionPromptRequest,
);

/// Fill a request's workspace name, session title and task attribution from
/// the scanned `sessions`. Workspace and title are only filled when empty, so
/// a request that already carries them keeps its own; the task reference is
/// always re-derived (it is computed state, never authored by the producer).
/// No-op when the session is not in `sessions` (a cold tick).
pub fn decorate<T: DecisionCard + ?Sized>(req: &mut T, sessions: &[SessionInfo]) {
    let Some(s) = sessions.iter().find(|s| s.id == req.session_id()) else {
        return;
    };
    let ws = s.workspace_name.clone();
    // Prefer the human/agent title override — for Codex sessions it's the
    // only real title (ai_title is the raw first prompt).
    let title = s.title_override.clone().or_else(|| s.ai_title.clone());
    let task = crate::task_model::decision_task_ref(sessions, &s.id);
    let (w, a, t) = req.display_fields();
    if w.is_empty() {
        *w = ws;
    }
    if a.is_none() {
        *a = title;
    }
    if task.is_some() {
        *t = task;
    }
}

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
