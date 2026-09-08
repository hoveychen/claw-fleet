import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import { playDecisionAlert } from "../audio";
import { flattenPending, reconcilePlan, suppressedIds } from "../decisionReconcile";
import { normalizeForSpeech } from "../decisionText";
import { useDecisionStore } from "../store";
import type {
  A2uiRenderRequest,
  ElicitationRequest,
  FleetAskRequest,
  GuardRequest,
  PendingDecisions,
  PermissionPromptRequest,
  PlanApprovalRequest,
} from "../types";

// Split a `question` field on a line containing only `---` (per Fleet
// Interaction Mode's "Speech Summary Divider" convention). Returns
// [preDivider, postDivider]. If no divider is found, returns [body, ""].
function splitOnDivider(body: string): [string, string] {
  const match = body.match(/^\s*---\s*$/m);
  if (!match || match.index === undefined) return [body.trim(), ""];
  const before = body.slice(0, match.index).trim();
  const after = body.slice(match.index + match[0].length).trim();
  return [before, after];
}

/**
 * How often the panel re-asks the backend what is still outstanding.
 *
 * Bounds how long a lost `*-request` / `*-dismissed` / `decision-parked` emit
 * can keep the panel wrong. 10s is short enough that a card feels live (an
 * agent blocks for 600s by default, so this is 1.6% of its wait) and cheap
 * enough to run forever: one HTTP round trip in the browser build, one
 * in-process directory scan on the desktop.
 */
const RECONCILE_EVERY_MS = 10_000;

// Pull the last sentence ending with ? or ？ from a markdown blob.
function lastQuestionSentence(text: string): string {
  const match = normalizeForSpeech(text).match(/([^。！？?!\n]{1,80}[？?])\s*$/);
  return match ? match[1].trim() : "";
}

/**
 * The fields any channel's request might carry that an announcement reads.
 *
 * Deliberately loose: every real request type is structurally assignable to it,
 * so one table can announce all six without a discriminated union whose only
 * job would be to pick a speech format.
 */
type Announcable = {
  id: string;
  workspaceName?: string;
  aiTitle?: string | null;
  toolName?: string | null;
  commandSummary?: string | null;
  parked?: boolean;
  questions?: { question?: string }[];
};

/** Workspace, then the card's own two-sentence speech summary. */
function spokenForQuestions(r: Announcable): string {
  const body = r.questions?.[0]?.question ?? "";
  const [intro, after] = splitOnDivider(body);
  const followup = after ? lastQuestionSentence(after) : "";
  return [r.workspaceName, normalizeForSpeech(intro), followup]
    .filter((s): s is string => !!s && s.length > 0)
    .join("。");
}

/** For cards whose body Fleet cannot read (A2UI surfaces, plan approvals). */
function spokenForTitle(r: Announcable): string {
  return [r.workspaceName, r.aiTitle ?? ""]
    .filter((s): s is string => !!s && s.length > 0)
    .join("。");
}

/** For the two "an agent is blocked on a yes/no" channels. */
function spokenForTool(r: Announcable): string {
  return [r.workspaceName, r.aiTitle, r.toolName || r.commandSummary]
    .filter((s): s is string => !!s && s.length > 0)
    .join(" ");
}

/**
 * Which chime a bucket gets, and how its announcement reads.
 *
 * Shared by the live listeners and the reconcile poll, because a card the poll
 * recovered is exactly the one the user most needs to hear about: the push
 * channel dropped it, so the panel appearing is the *only* other signal, and it
 * is worthless if he is looking at another window. Keyed by the bucket name in
 * `PendingDecisions` so the poll can announce without knowing each shape.
 */
const ANNOUNCERS: Record<
  keyof PendingDecisions,
  { chime: "guard" | "elicitation"; speak: (r: Announcable) => string }
> = {
  guard: { chime: "guard", speak: spokenForTool },
  elicitation: { chime: "elicitation", speak: spokenForQuestions },
  fleetAsk: { chime: "elicitation", speak: spokenForQuestions },
  a2uiRender: { chime: "elicitation", speak: spokenForTitle },
  planApproval: { chime: "elicitation", speak: spokenForTitle },
  permissionPrompt: { chime: "guard", speak: spokenForTool },
};

/**
 * Subscribe to backend decision events and push them into the decision store.
 *
 * Must be mounted at the App root (unconditionally) so events are never
 * dropped while the DecisionPanel itself is unmounted (no pending
 * decisions). Backend emits are one-shot — if no listener is
 * attached at emit time, the event is gone.
 */
export function useDecisionEvents() {
  const addGuardRequest = useDecisionStore((s) => s.addGuardRequest);
  const addElicitationRequest = useDecisionStore((s) => s.addElicitationRequest);
  const addFleetAskRequest = useDecisionStore((s) => s.addFleetAskRequest);
  const addA2uiRenderRequest = useDecisionStore((s) => s.addA2uiRenderRequest);
  const addPlanApprovalRequest = useDecisionStore((s) => s.addPlanApprovalRequest);
  const addPermissionPromptRequest = useDecisionStore((s) => s.addPermissionPromptRequest);
  const dismiss = useDecisionStore((s) => s.dismiss);
  const markParked = useDecisionStore((s) => s.markParked);

  // Dedup: re-emitted payloads (e.g. after remount / reconnect) shouldn't
  // double-chime.
  const announcedIds = useRef<Set<string>>(new Set());

  // Continuous reconciliation against the backend's pending set.
  //
  // Every card arrives as a *one-shot* emit: the watcher broadcasts each new
  // request id once and never again, and neither Tauri events nor SSE are
  // buffered for a listener that was not attached at emit time. So a single
  // missed frame is not a delayed card — it is a card nobody ever sees. Ways to
  // miss one, all observed: a cold app restart while a `fleet mcp` child is
  // still blocking on its poll; in the browser build, an `EventSource` that
  // dropped and is mid-reconnect, a proxy that reaped the idle stream, a laptop
  // that slept, or a socket the server can still write to while the page behind
  // it is gone. On Boss's `fleet webui` box on 2026-09-08 that cost two cards
  // in a row: raised at 21:54 and 21:57, invisible for the full 600s wait,
  // recovered only by reloading the page half an hour later.
  //
  // Fixing the transport does not fix this class — the next transport gets its
  // own hiccups. What fixes it is not depending on delivery: ask the backend
  // what is still outstanding, on a timer, and make the store match. The
  // `add*` actions dedup by id, so re-seeding is free, and `list_pending_decisions`
  // is a local read (six directory listings) served by one route.
  useEffect(() => {
    let cancelled = false;

    const reconcile = (why: string) => {
      // Ids present *before* the fetch. A card that shows up while the request
      // is in flight is not stale — the backend read its directory before that
      // card existed — so pruning is confined to this set.
      const before = new Set(useDecisionStore.getState().decisions.map((d) => d.id));
      invoke<PendingDecisions>("list_pending_decisions")
        .then((p) => {
          if (cancelled || !p) return;
          // Cards this client just dealt with. The poll is faster than the
          // answer round trip, so without this a just-answered card comes
          // straight back on screen.
          const skip = suppressedIds();
          const add = <T extends Announcable>(
            bucket: keyof PendingDecisions,
            r: T,
            action: (r: T) => void,
          ) => {
            if (skip.has(r.id)) return;
            // Chime for a card this poll is the *first* to see, and only when
            // the page was already open: the panel appearing is no signal at
            // all to someone looking at another window, and a recovered card is
            // precisely the one whose push was dropped. Silent on mount (a card
            // that predates the page is not news) and silent for a parked card,
            // which is an old question being re-listed — same two exemptions
            // the live listeners make.
            if (why !== "mount" && !r.parked && !announcedIds.current.has(r.id)) {
              announcedIds.current.add(r.id);
              const a = ANNOUNCERS[bucket];
              playDecisionAlert(a.chime, a.speak(r));
            }
            action(r);
          };
          p.guard?.forEach((r) => add("guard", r, addGuardRequest));
          p.elicitation?.forEach((r) => add("elicitation", r, addElicitationRequest));
          p.fleetAsk?.forEach((r) => add("fleetAsk", r, addFleetAskRequest));
          p.a2uiRender?.forEach((r) => add("a2uiRender", r, addA2uiRenderRequest));
          p.planApproval?.forEach((r) => add("planApproval", r, addPlanApprovalRequest));
          p.permissionPrompt?.forEach((r) => add("permissionPrompt", r, addPermissionPromptRequest));

          // The other two directions, both just as losable as a `*-request`
          // emit: a missed `*-dismissed` strands a card the backend already
          // cleaned up (answering it gets "no pending request"), and a missed
          // `decision-parked` leaves a timed-out card showing a running
          // countdown — the opposite of the truth. `markParked` is in place, so
          // whatever the user had already typed survives.
          const plan = reconcilePlan(
            useDecisionStore.getState().decisions,
            before,
            flattenPending(p),
          );
          if (plan.drop.length > 0) {
            console.log(
              `[decision] reconcile (${why}): dropping ${plan.drop.join(", ")} — no longer pending on the backend`,
            );
          }
          plan.drop.forEach((id) => dismiss(id));
          plan.park.forEach((id) => markParked(id));
        })
        .catch((e) => {
          // Do NOT prune on a failed fetch: "the request errored" and "nothing
          // is pending" are the same empty answer, and acting on the first
          // would clear the panel every time the network blinks.
          console.warn(`[decision] reconcile (${why}) list_pending_decisions failed:`, e);
        });
    };

    reconcile("mount");
    const timer = window.setInterval(() => reconcile("interval"), RECONCILE_EVERY_MS);
    // Background tabs get their timers throttled to about once a minute, so the
    // first thing a returning user would otherwise see is a stale panel.
    const onVisible = () => {
      if (document.visibilityState === "visible") reconcile("visible");
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [
    addGuardRequest,
    addElicitationRequest,
    addFleetAskRequest,
    addA2uiRenderRequest,
    addPlanApprovalRequest,
    addPermissionPromptRequest,
    dismiss,
    markParked,
  ]);

  useEffect(() => {
    const unlisten = listen<GuardRequest>("guard-request", (e) => {
      const r = e.payload;
      if (!announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        playDecisionAlert("guard", spokenForTool(r));
      }
      addGuardRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addGuardRequest]);

  useEffect(() => {
    const unlisten = listen<ElicitationRequest>("elicitation-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        playDecisionAlert("elicitation", spokenForQuestions(r));
      }
      addElicitationRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addElicitationRequest]);

  useEffect(() => {
    const unlisten = listen<FleetAskRequest>("fleet-ask-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        // Reuse the elicitation chime — `fleet__ask` is the same
        // "agent needs your input" feel as AskUserQuestion.
        playDecisionAlert("elicitation", spokenForQuestions(r));
      }
      addFleetAskRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addFleetAskRequest]);

  useEffect(() => {
    const unlisten = listen<A2uiRenderRequest>("a2ui-render-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        // A2UI surfaces are opaque to Fleet — speak the workspace + title
        // only; the actual UI is announced by `@a2ui/react` accessibility.
        playDecisionAlert("elicitation", spokenForTitle(r));
      }
      addA2uiRenderRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addA2uiRenderRequest]);

  useEffect(() => {
    const unlisten = listen<PlanApprovalRequest>("plan-approval-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        playDecisionAlert("elicitation", spokenForTitle(r));
      }
      addPlanApprovalRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addPlanApprovalRequest]);


  useEffect(() => {
    const unlisten = listen<PermissionPromptRequest>("permission-prompt-request", (e) => {
      const r = e.payload;
      if (!announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        // Same urgency as guard: the headless agent is blocked until answered.
        playDecisionAlert("guard", spokenForTool(r));
      }
      addPermissionPromptRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addPermissionPromptRequest]);

  // Park events — a card the panel is already showing just timed out. It does
  // NOT go away: the backend interrupted the session's turn and is holding the
  // question until it gets an answer, which then resumes the session. Flip the
  // card in place so it can badge itself, keeping whatever the user had already
  // filled in.
  useEffect(() => {
    const unlisten = listen<string>("decision-parked", (e) => {
      const id = e.payload;
      if (!id) return;
      console.log(`[decision] decision-parked id=${id} — wait timed out, session interrupted; card stays until answered`);
      markParked(id);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [markParked]);

  // Dismiss events — fire when another client (another desktop) answers
  // a pending decision, or when `fleet guard` times out and cleans up the
  // request file. The backend polling loop in local_backend.rs / remote.rs
  // emits these by diffing the known id set against the current pending set.
  // A parked card is NOT dismissed: the backend keeps it in the pending set.
  useEffect(() => {
    const unlistens = [
      "guard-dismissed",
      "elicitation-dismissed",
      "fleet-ask-dismissed",
      "a2ui-render-dismissed",
      "plan-approval-dismissed",
      "permission-prompt-dismissed",
    ].map(
      (evt) => listen<string>(evt, (e) => {
        const id = e.payload;
        if (id) {
          console.log(
            `[decision] ${evt} id=${id} — panel being removed (hook CLI cleaned up request file, or peer client answered)`,
          );
          announcedIds.current.delete(id);
          dismiss(id);
        }
      }),
    );
    return () => {
      Promise.all(unlistens).then((fns) => fns.forEach((fn) => fn()));
    };
  }, [dismiss]);
}
