// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: "main" }),
}));

import { usePathLinks, usePathMarkdown } from "./usePathLinks";
import { useSessionsStore } from "../store";
import type { SessionInfo } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  act(() => useSessionsStore.getState().setSessions([]));
});

function session(id: string, workspacePath: string): SessionInfo {
  return { id, workspacePath, status: "idle", tokenSpeed: 0 } as unknown as SessionInfo;
}

/** Render a probe that records what the hooks hand back on every render. */
function probe(sessionId: string) {
  const ctxSeen: unknown[] = [];
  const componentsSeen: unknown[] = [];
  function Probe() {
    ctxSeen.push(usePathLinks(sessionId));
    componentsSeen.push(usePathMarkdown(sessionId));
    return null;
  }
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Probe />));
  return { ctxSeen, componentsSeen };
}

describe("usePathLinks identity", () => {
  it("survives a sessions-updated push that changed nothing it reads", () => {
    // The backend rescans every 2s while any session is alive and `setSessions`
    // always installs a fresh array, so a hook that depends on the whole array
    // hands out a new object ~30×/min. Downstream that means `ReactMarkdown`
    // re-parses its entire body that often — the decision card melted on a
    // 1.5 MB TASKS.md exactly this way.
    act(() =>
      useSessionsStore.getState().setSessions([session("s1", "/Users/x/proj")]),
    );
    const { ctxSeen, componentsSeen } = probe("s1");
    const ctxBefore = ctxSeen[ctxSeen.length - 1];
    const componentsBefore = componentsSeen[componentsSeen.length - 1];
    expect(ctxBefore).toBeDefined();

    // A new array, same workspace for s1 — exactly what a no-op rescan emits.
    act(() =>
      useSessionsStore.getState().setSessions([session("s1", "/Users/x/proj")]),
    );

    expect(ctxSeen[ctxSeen.length - 1]).toBe(ctxBefore);
    expect(componentsSeen[componentsSeen.length - 1]).toBe(componentsBefore);
  });

  it("still picks up a real workspace change", () => {
    act(() =>
      useSessionsStore.getState().setSessions([session("s1", "/Users/x/proj")]),
    );
    const { ctxSeen } = probe("s1");
    const before = ctxSeen[ctxSeen.length - 1] as { workspaceRoot: string };
    expect(before.workspaceRoot).toBe("/Users/x/proj");

    act(() =>
      useSessionsStore.getState().setSessions([session("s1", "/Users/x/other")]),
    );
    const after = ctxSeen[ctxSeen.length - 1] as { workspaceRoot: string };
    expect(after).not.toBe(before);
    expect(after.workspaceRoot).toBe("/Users/x/other");
  });
});
