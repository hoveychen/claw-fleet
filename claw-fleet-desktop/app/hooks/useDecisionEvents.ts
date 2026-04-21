import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { playDecisionAlert } from "../audio";
import { useDecisionStore } from "../store";
import type { ElicitationRequest, GuardRequest, PlanApprovalRequest } from "../types";

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
  const plain = text.replace(/[`*_#>\[\]()]/g, " ");
  const match = plain.match(/([^。！？?!\n]{1,80}[？?])\s*$/);
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
  const addPlanApprovalRequest = useDecisionStore((s) => s.addPlanApprovalRequest);

  // Dedup: re-emitted payloads (e.g. after remount / reconnect) shouldn't
  // double-chime.
  const announcedIds = useRef<Set<string>>(new Set());

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
      if (!silent && !announcedIds.current.has(r.id)) {
        announcedIds.current.add(r.id);
        const body = r.questions[0]?.question ?? "";
        const [intro, after] = splitOnDivider(body);
        const followup = after ? lastQuestionSentence(after) : "";
        const spoken = [r.workspaceName, intro, followup]
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
    const unlisten = listen<PlanApprovalRequest>("plan-approval-request", (e) => {
      const r = e.payload;
      if (!silent && !announcedIds.current.has(r.id)) {
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
}
