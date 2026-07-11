import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkflowTree } from "../types";

/**
 * Poll a session's Claude Code Workflow runs (journal + reconstructed DAG),
 * keyed by `jsonlPath`. Re-polls every 4s while any run still has a running
 * agent so the DAG node dots flip running → done live; stops once everything
 * is done. Returns `[]` when disabled or before the first fetch resolves.
 *
 * Backend goes through the Backend trait (`get_workflow_trees`), so this works
 * for both LocalBackend and RemoteBackend.
 */
export function useWorkflowTrees(
  jsonlPath: string | null | undefined,
  enabled: boolean,
  paused = false,
): WorkflowTree[] {
  const [trees, setTrees] = useState<WorkflowTree[]>([]);

  useEffect(() => {
    if (!jsonlPath || !enabled) {
      setTrees([]);
      return;
    }
    // Backgrounded (hidden tab): hold the last DAG rather than clearing it, so
    // the workflow tab doesn't vanish and reappear across a tab switch. Coming
    // back re-arms the effect, which re-polls immediately.
    if (paused) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const next = await invoke<WorkflowTree[]>("get_workflow_trees", {
          jsonlPath,
        });
        if (cancelled) return;
        setTrees(next);
        const anyRunning = next.some((t) =>
          t.agents.some((a) => a.status === "running"),
        );
        if (anyRunning) timer = setTimeout(poll, 4000);
      } catch {
        // best-effort: leave whatever we have
      }
    };
    poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [jsonlPath, enabled, paused]);

  return trees;
}
