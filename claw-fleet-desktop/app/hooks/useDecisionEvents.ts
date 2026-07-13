import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import { playDecisionAlert } from "../audio";
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

// Pull the last sentence ending with ? or ？ from a markdown blob.
function lastQuestionSentence(text: string): string {
  const match = normalizeForSpeech(text).match(/([^。！？?!\n]{1,80}[？?])\s*$/);
  return match ? match[1].trim() : "";
}

/**
 * Subscribe to backend decision events and push them into the decision store.
 *
 * Must be mounted at the App root (unconditionally) so events are never
 * dropped while the DecisionPanel itself is unmounted (e.g. lite mode with
 * no pending decisions). Backend emits are one-shot — if no listener is
 * attached at emit time, the event is gone.
 *
 * `silent: true` skips `playDecisionAlert` — used by the decision-float
 * window so the main window stays the single source of audio.
 */
export function useDecisionEvents(options: { silent?: boolean } = {}) {
  const silent = options.silent ?? false;
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

  // Mount catch-up: the backend watcher emits each pending request exactly
  // once, and Tauri events are NOT buffered for listeners that attach later.
  // On a cold restart while a `fleet elicitation` / `fleet mcp` child process
  // is still blocking on its poll, that one-shot emit fires before this hook's
  // listeners are attached — so the decision panel never reappears and the
  // agent stays blocked until its (default 600s) timeout. Pull the current
  // pending set once on mount and seed the store directly; the add* actions
  // dedup by id, so this is safe even when a live event also arrives. Only the
  // main window pulls — the decision-float window (silent) mirrors the
  // snapshot handed to it by App.tsx.
  useEffect(() => {
    if (silent) return;
    let cancelled = false;
    invoke<PendingDecisions>("list_pending_decisions")
      .then((p) => {
        if (cancelled || !p) return;
        p.guard?.forEach((r) => addGuardRequest(r));
        p.elicitation?.forEach((r) => addElicitationRequest(r));
        p.fleetAsk?.forEach((r) => addFleetAskRequest(r));
        p.a2uiRender?.forEach((r) => addA2uiRenderRequest(r));
        p.planApproval?.forEach((r) => addPlanApprovalRequest(r));
        p.permissionPrompt?.forEach((r) => addPermissionPromptRequest(r));
      })
      .catch((e) => {
        console.warn("[decision] mount catch-up list_pending_decisions failed:", e);
      });
    return () => {
      cancelled = true;
    };
  }, [
    silent,
    addGuardRequest,
    addElicitationRequest,
    addFleetAskRequest,
    addA2uiRenderRequest,
    addPlanApprovalRequest,
    addPermissionPromptRequest,
  ]);

  useEffect(() => {
    const unlisten = listen<GuardRequest>("guard-request", (e) => {
      const r = e.payload;
      if (!silent && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        const spoken = [r.workspaceName, r.aiTitle, r.toolName || r.commandSummary]
          .filter((s): s is string => !!s && s.length > 0)
          .join(" ");
        playDecisionAlert("guard", spoken);
      }
      addGuardRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addGuardRequest, silent]);

  useEffect(() => {
    const unlisten = listen<ElicitationRequest>("elicitation-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!silent && !r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        const body = r.questions[0]?.question ?? "";
        const [intro, after] = splitOnDivider(body);
        const followup = after ? lastQuestionSentence(after) : "";
        const spoken = [r.workspaceName, normalizeForSpeech(intro), followup]
          .filter((s): s is string => !!s && s.length > 0)
          .join("。");
        playDecisionAlert("elicitation", spoken);
      }
      addElicitationRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addElicitationRequest, silent]);

  useEffect(() => {
    const unlisten = listen<FleetAskRequest>("fleet-ask-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!silent && !r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        const body = r.questions[0]?.question ?? "";
        const [intro, after] = splitOnDivider(body);
        const followup = after ? lastQuestionSentence(after) : "";
        const spoken = [r.workspaceName, normalizeForSpeech(intro), followup]
          .filter((s): s is string => !!s && s.length > 0)
          .join("。");
        // Reuse the elicitation chime — `fleet__ask` is the same
        // "agent needs your input" feel as AskUserQuestion.
        playDecisionAlert("elicitation", spoken);
      }
      addFleetAskRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addFleetAskRequest, silent]);

  useEffect(() => {
    const unlisten = listen<A2uiRenderRequest>("a2ui-render-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!silent && !r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        // A2UI surfaces are opaque to Fleet — speak the workspace + title
        // only; the actual UI is announced by `@a2ui/react` accessibility.
        const spoken = [r.workspaceName, r.aiTitle ?? ""]
          .filter((s): s is string => !!s && s.length > 0)
          .join("。");
        playDecisionAlert("elicitation", spoken);
      }
      addA2uiRenderRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addA2uiRenderRequest, silent]);

  useEffect(() => {
    const unlisten = listen<PlanApprovalRequest>("plan-approval-request", (e) => {
      const r = e.payload;
      // A parked card is an old question being re-listed, not a new ask — chiming
      // for it would re-announce the same question on every app restart.
      if (!silent && !r.parked && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        const spoken = [r.workspaceName, r.aiTitle ?? ""]
          .filter((s): s is string => !!s && s.length > 0)
          .join("。");
        playDecisionAlert("elicitation", spoken);
      }
      addPlanApprovalRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addPlanApprovalRequest, silent]);


  useEffect(() => {
    const unlisten = listen<PermissionPromptRequest>("permission-prompt-request", (e) => {
      const r = e.payload;
      if (!silent && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        // Same urgency as guard: the headless agent is blocked until answered.
        const spoken = [r.workspaceName, r.aiTitle, r.toolName]
          .filter((s): s is string => !!s && s.length > 0)
          .join(" ");
        playDecisionAlert("guard", spoken);
      }
      addPermissionPromptRequest(r);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addPermissionPromptRequest, silent]);

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
