// @vitest-environment jsdom
//
// The dsh half of the model / effort pills. dsh is the one agent whose lists are
// fetched rather than curated, so these cover the three things that can only go
// wrong at the component level: the catalogue reaching the menu at all, the
// effort ladder following the *selected* model, and the degrade when there is no
// catalogue to show.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import "../i18n";
import { SessionOptionPills } from "./SessionOptionPills";
import { MOCK_DSH_MODELS } from "../mock/data";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) =>
    cmd === "dsh_models" ? MOCK_DSH_MODELS : null,
  );
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

/** Render the pills for a dsh session with `model` already selected. */
async function render(model = "", effort = "") {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const onModelChange = vi.fn();
  await act(async () => {
    root!.render(
      <SessionOptionPills
        tool="dsh"
        model={model}
        effort={effort}
        permissionMode=""
        onModelChange={onModelChange}
        onEffortChange={() => {}}
        onPermissionModeChange={() => {}}
      />,
    );
  });
  return { onModelChange };
}

async function clickPill(title: string) {
  const pill = container!.querySelector<HTMLButtonElement>(`button[title="${title}"]`);
  expect(pill, `pill not found: ${title}`).toBeTruthy();
  await act(async () => pill!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

/** Click an open menu's row by its visible text. */
async function clickRow(text: string) {
  const row = [...container!.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]')].find(
    (b) => b.textContent === text,
  );
  expect(row, `menu row not found: ${text}`).toBeTruthy();
  await act(async () => row!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

function rows(): string[] {
  return [...container!.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]')].map(
    (b) => b.textContent ?? "",
  );
}

describe("SessionOptionPills — dsh", () => {
  it("fetches the catalogue and offers full provider/model specs, not bare ids", async () => {
    await render();
    expect(invoke).toHaveBeenCalledWith("dsh_models");

    await clickPill("Model");
    // The small provider is listed inline…
    expect(rows()).toContain("DeepSeek-V4-Pro");
    // …and the large one is folded behind its vendors rather than dumped flat.
    expect(rows()).toContain("anthropic (4)");
    expect(rows()).not.toContain("anthropic/claude-opus-5");

    // Claude's curated list must never leak into a dsh menu: a bare
    // `claude-opus-5` has no provider prefix, so dsh's `split_model` drops it
    // and the pick would silently do nothing.
    expect(rows()).not.toContain("Opus 5");
  });

  it("commits the three-segment spec dsh needs when picking inside a vendor folder", async () => {
    const { onModelChange } = await render();
    await clickPill("Model");
    await clickRow("anthropic (4)");
    // Opening a folder must not dismiss the popover, or the second level is
    // unreachable in one gesture.
    expect(rows()).toContain("anthropic/claude-opus-5");

    await clickRow("anthropic/claude-opus-5");
    expect(onModelChange).toHaveBeenCalledWith("openrouter/anthropic/claude-opus-5");
  });

  it("takes the effort ladder from the selected model, not a fixed table", async () => {
    // deepseek-official publishes off/low/high/max — note "max", which Codex's
    // scale does not have, and no "medium", which it does.
    await render("deepseek-official/deepseek-v4-pro");
    await clickPill("Effort");
    expect(rows()).toContain("max");
    expect(rows()).not.toContain("medium");
    // dsh names a default for this model, so the un-chosen row says which.
    expect(rows()[0]).toContain("high");
  });

  it("offers no effort at all for a model with no reasoning control", async () => {
    // `ai21/jamba-large-1.7` carries no `reasoning` block — 83 of the 276 real
    // openrouter models are like this. Showing the previous model's ladder would
    // invite a value dsh will not honour.
    await render("openrouter/ai21/jamba-large-1.7");
    await clickPill("Effort");
    expect(rows()).toEqual(["Default"]);
  });

  it("degrades to Default-only but says why, when the catalogue cannot be fetched", async () => {
    // No dsh on the host, the server failing to start, or a `fleet serve` too
    // old to know the route. The pill row must still render, and the menu must
    // no longer silently pretend "no options" — the reason and a retry show.
    invoke.mockImplementation(async () => {
      throw new Error("dsh is not installed");
    });
    await render();
    await clickPill("Model");
    expect(rows()).toEqual(["Default (CLI setting)"]);
    expect(container!.textContent).toContain("dsh is not installed");
    expect([...container!.querySelectorAll("button")].some((b) => b.textContent === "Retry")).toBe(
      true,
    );

    await clickPill("Model");
    await clickPill("Effort");
    expect(rows()).toEqual(["Default"]);
  });

  it("refetches the catalogue every time the model menu opens", async () => {
    // The launch race this exists for: the first fetch fires while `dsh web` is
    // still booting after an app relaunch. Opening the menu must retry instead
    // of settling for "no options" until the dialog is remounted.
    let calls = 0;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd !== "dsh_models") return null;
      calls += 1;
      if (calls <= 2) throw new Error("dsh web not up yet");
      return MOCK_DSH_MODELS;
    });
    await render();
    // Mount fetch + first open both land in the failure window…
    await clickPill("Model");
    expect(rows()).toEqual(["Default (CLI setting)"]);
    expect(container!.textContent).toContain("dsh web not up yet");
    // …close, reopen, and the same menu instance recovers without a remount.
    await clickPill("Model");
    await clickPill("Model");
    expect(invoke.mock.calls.filter((c) => c[0] === "dsh_models").length).toBe(3);
    expect(rows()).toContain("DeepSeek-V4-Pro");
    expect(container!.textContent).not.toContain("dsh web not up yet");
  });

  it("retry from the failure block recovers the catalogue without closing the menu", async () => {
    let calls = 0;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd !== "dsh_models") return null;
      calls += 1;
      if (calls < 3) throw new Error("dsh is not installed");
      return MOCK_DSH_MODELS;
    });
    await render();
    await clickPill("Model");
    expect(rows()).toEqual(["Default (CLI setting)"]);

    const retry = [...container!.querySelectorAll("button")].find((b) => b.textContent === "Retry");
    expect(retry).toBeTruthy();
    await act(async () => retry!.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(rows()).toContain("DeepSeek-V4-Pro");
    expect(container!.textContent).not.toContain("dsh is not installed");
  });

  it("surfaces per-provider failures from a partial catalogue while keeping the sound groups", async () => {
    // llm.models answers ok with the providers it could not reach in
    // `failures` — that is normal partial success, and it must be visible
    // rather than silently dropped, next to the models that did load.
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "dsh_models"
        ? { ...MOCK_DSH_MODELS, failures: [{ id: "openrouter", name: "OpenRouter", message: "network down" }] }
        : null,
    );
    await render();
    await clickPill("Model");
    expect(rows()).toContain("DeepSeek-V4-Pro");
    expect(container!.textContent).toContain("OpenRouter");
    expect(container!.textContent).toContain("network down");
  });

  it("hides the permission pill — dsh has no --permission-mode analogue", async () => {
    await render();
    expect(container!.querySelector('button[title="Permission"]')).toBeNull();
  });
});
