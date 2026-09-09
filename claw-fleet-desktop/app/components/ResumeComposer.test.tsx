// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

// The model / effort pills now read Fleet's model catalog (`models.toml`)
// through the `model_catalog` command instead of a hardcoded TS list, so the
// mock has to answer it. Everything else still resolves to null.
const CODEX_MODELS = [
  {
    id: "gpt-5.6-sol",
    label: "GPT-5.6 Sol",
    harness: "codex",
    tier: "premium",
    efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    defaultEffort: "medium",
  },
];
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) =>
    cmd === "model_catalog"
      ? [
          { name: "claude", available: true, models: [] },
          { name: "codex", available: true, models: CODEX_MODELS },
          { name: "dsh", available: false, models: [] },
        ]
      : null,
  ),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import "../i18n";
import { ResumeComposer } from "./ResumeComposer";
import { __resetModelCatalogCache } from "../useModelCatalog";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  // The catalog is cached per module, so one test's fetch would otherwise be
  // reused (or, worse, its empty failure result) by the next.
  __resetModelCatalogCache();
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
    // `minimal` is NOT on any Codex ladder — the old hardcoded table invented
    // it. The real ladder for Sol runs low..max plus `ultra`.
    expect(container.textContent).not.toContain("minimal");
    expect(container.textContent).toContain("ultra");
    expect(container.querySelector('button[title="Permission"]')).toBeNull();
  });
});
