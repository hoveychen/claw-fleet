import { useMemo } from "react";
import type { Components } from "react-markdown";
import type { PathLinkContext } from "../markdown/pathLinks";
import { pathAwareMarkdownComponents, safeMarkdownComponents } from "../markdown/safeLinks";
import { useSessionsStore, useUIStore, type FileNavRequest } from "../store";

export type OpenFilePayload = Omit<FileNavRequest, "nonce">;

/**
 * Build the context that makes path-shaped inline code clickable, for surfaces
 * that know a session but not its workspace — decision cards carry a
 * `sessionId` and a `workspaceName`, but no path, so the workspace root is
 * recovered from the sessions store.
 *
 * Returns undefined when the session isn't known yet, which leaves paths inert
 * rather than resolving them against the wrong root.
 *
 * Subscribes to the *resolved workspace path*, not to `sessions`: the backend
 * rescans every 2s while any session is alive and `setSessions` always installs
 * a fresh array, so depending on the array itself handed out a new context —
 * and with it a new `components` object for ReactMarkdown, which re-parses its
 * whole body on every render — about 30 times a minute. On a 1.5 MB review doc
 * that pinned a core for as long as the card stayed open.
 */
export function usePathLinks(sessionId: string | null | undefined): PathLinkContext | undefined {
  const workspacePath = useSessionsStore(
    (s) => s.sessions.find((x) => x.id === sessionId)?.workspacePath,
  );
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const unresolvedPaths = useUIStore((s) => s.unresolvedPaths);

  return useMemo(() => {
    if (!sessionId) return undefined;
    if (!workspacePath) return undefined;
    return {
      workspaceRoot: workspacePath,
      unresolved: unresolvedPaths,
      openInFiles: (absPath, line, tried) => {
        requestFileNav({ workspacePath, absPath, line, tried });
      },
    };
  }, [sessionId, workspacePath, requestFileNav, unresolvedPaths]);
}

/**
 * Markdown components for a decision surface: same as `safeMarkdownComponents`,
 * plus clickable path chips once the session's workspace is known. Memoised so
 * ReactMarkdown isn't handed a fresh components object on every render.
 */
export function usePathMarkdown(sessionId: string | null | undefined): Components {
  const paths = usePathLinks(sessionId);
  return useMemo(
    () => (paths ? pathAwareMarkdownComponents(paths) : safeMarkdownComponents),
    [paths],
  );
}
