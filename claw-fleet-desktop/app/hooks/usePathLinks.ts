import { useMemo } from "react";
import type { Components } from "react-markdown";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { PathLinkContext } from "../markdown/pathLinks";
import { pathAwareMarkdownComponents, safeMarkdownComponents } from "../markdown/safeLinks";
import { useConnectionStore, useSessionsStore, useUIStore, type FileNavRequest } from "../store";

/** Cross-window request to open a file in the 文件 page. Raised by the
 *  decision-float window, which has no explorer of its own. */
export const OPEN_FILE_EVENT = "open-file-in-files";
export type OpenFilePayload = Omit<FileNavRequest, "nonce">;

/** The decision-float window mirrors the main window's stores but renders only
 *  a card — so a path click there has to hop to the main window. */
function isFloatWindow(): boolean {
  try {
    return getCurrentWindow().label === "decision-float";
  } catch {
    return false;
  }
}

/**
 * Build the context that makes path-shaped inline code clickable, for surfaces
 * that know a session but not its workspace — decision cards carry a
 * `sessionId` and a `workspaceName`, but no path, so the workspace root is
 * recovered from the sessions store (which the float window mirrors too).
 *
 * Returns undefined when the session isn't known yet, which leaves paths inert
 * rather than resolving them against the wrong root.
 */
export function usePathLinks(sessionId: string | null | undefined): PathLinkContext | undefined {
  const sessions = useSessionsStore((s) => s.sessions);
  const connection = useConnectionStore((s) => s.connection);
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const float = useMemo(isFloatWindow, []);

  return useMemo(() => {
    if (!sessionId) return undefined;
    const workspacePath = sessions.find((s) => s.id === sessionId)?.workspacePath;
    if (!workspacePath) return undefined;
    return {
      workspaceRoot: workspacePath,
      isLocal: connection?.type !== "remote",
      openInFiles: (absPath, line) => {
        const req: OpenFilePayload = { workspacePath, absPath, line };
        if (float) {
          void emit(OPEN_FILE_EVENT, req);
          void invoke("show_main_window");
        } else {
          requestFileNav(req);
        }
      },
    };
  }, [sessionId, sessions, connection?.type, requestFileNav, float]);
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
