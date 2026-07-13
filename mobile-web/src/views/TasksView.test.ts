import { describe, expect, it } from "vitest";
import { CHAT_HIDDEN, CHAT_ONLY, matchesWorkspaceFilter } from "./TasksView";
import type { SessionInfo } from "../types";

/**
 * The tasks list mixes chat sessions with project ones. The chat workspace path
 * comes from the desktop over the relay (`chat_workspace`), so the phone can be
 * asked to filter by it before — or without ever — learning it.
 */
function session(workspacePath: string): SessionInfo {
  return { id: "s", workspacePath, workspaceName: "n" } as unknown as SessionInfo;
}

describe("matchesWorkspaceFilter", () => {
  const CHAT = "/Users/foo/.fleet/chat";
  const chat = session(CHAT);
  const repo = session("/Users/foo/repo");

  it("passes everything when no workspace is picked, chat included", () => {
    expect(matchesWorkspaceFilter(chat, "", CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(repo, "", CHAT)).toBe(true);
  });

  it("keeps only chat sessions under CHAT_ONLY", () => {
    expect(matchesWorkspaceFilter(chat, CHAT_ONLY, CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(repo, CHAT_ONLY, CHAT)).toBe(false);
  });

  it("drops chat sessions under CHAT_HIDDEN", () => {
    expect(matchesWorkspaceFilter(chat, CHAT_HIDDEN, CHAT)).toBe(false);
    expect(matchesWorkspaceFilter(repo, CHAT_HIDDEN, CHAT)).toBe(true);
  });

  it("still matches a plain workspace path", () => {
    expect(matchesWorkspaceFilter(repo, "/Users/foo/repo", CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(chat, "/Users/foo/repo", CHAT)).toBe(false);
  });

  it("degrades to 'all' when the desktop never sent a chat path", () => {
    // An older desktop doesn't know the `chat_workspace` method. Showing every
    // session beats blanking the list.
    for (const s of [chat, repo]) {
      expect(matchesWorkspaceFilter(s, CHAT_ONLY, null)).toBe(true);
      expect(matchesWorkspaceFilter(s, CHAT_HIDDEN, null)).toBe(true);
    }
  });
});
