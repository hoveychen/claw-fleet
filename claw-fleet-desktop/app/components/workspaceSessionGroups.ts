import type { SessionInfo } from "../types";
import { repoRootPath } from "./NewSessionForm";

export interface WorkspaceSessionGroup {
  path: string;
  name: string;
  latestActivityMs: number;
  sessions: SessionInfo[];
}

function activityMs(session: SessionInfo): number {
  return session.agentLastActivityMs ?? session.lastActivityMs;
}

export interface GroupSessionsOptions {
  /** Keep `sessions` in the order given instead of re-sorting by activity —
   *  both within a section and across sections (a section takes the position of
   *  its first member). The rail passes this while its sort freeze is engaged;
   *  without it the freeze would be undone right here, since the frozen list is
   *  re-sorted the moment it is grouped. */
  preserveOrder?: boolean;
}

/**
 * Turn the task rail's flat session result into repository-sized sections.
 * Fleet worktrees belong to their durable repository root, matching the new
 * session launcher's workspace picker. Recently active repositories stay near
 * the top; alphabetical order keeps equal timestamps deterministic.
 */
export function groupSessionsByWorkspace(
  sessions: SessionInfo[],
  { preserveOrder = false }: GroupSessionsOptions = {},
): WorkspaceSessionGroup[] {
  const groups = new Map<string, WorkspaceSessionGroup>();

  for (const session of sessions) {
    const path = repoRootPath(session.workspacePath);
    const lastMs = activityMs(session);
    const existing = groups.get(path);
    if (existing) {
      existing.sessions.push(session);
      existing.latestActivityMs = Math.max(existing.latestActivityMs, lastMs);
      continue;
    }

    groups.set(path, {
      path,
      name: session.workspaceName,
      latestActivityMs: lastMs,
      sessions: [session],
    });
  }

  // Insertion order already mirrors the caller's order, so preserving it is
  // simply skipping both sorts.
  if (preserveOrder) return [...groups.values()];

  for (const group of groups.values()) {
    group.sessions.sort((a, b) => activityMs(b) - activityMs(a));
  }

  return [...groups.values()].sort(
    (a, b) =>
      b.latestActivityMs - a.latestActivityMs || a.name.localeCompare(b.name),
  );
}
