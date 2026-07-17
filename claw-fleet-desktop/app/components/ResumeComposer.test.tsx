// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import "../i18n";
import { ResumeComposer } from "./ResumeComposer";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

async function openPill(title: string) {
  const pill = container!.querySelector<HTMLButtonElement>(`button[title="${title}"]`);
  expect(pill, `pill not found: ${title}`).toBeTruthy();
  await act(async () => pill!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

describe("ResumeComposer agent options", () => {
  it("uses Codex models and effort without Claude permissions for a Codex session", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root!.render(
        <ResumeComposer
          sessionId="codex-session"
          workspacePath="/workspace/fleet"
          agentSource="codex"
          onResumed={() => {}}
        />,
      );
    });

    await openPill("Model");
    expect(container.textContent).toContain("GPT-5.6 Sol");
    expect(container.textContent).not.toContain("Opus 4.8");

    await openPill("Model");
    await openPill("Effort");
    expect(container.textContent).toContain("minimal");
    expect(container.querySelector('button[title="Permission"]')).toBeNull();
  });
});
