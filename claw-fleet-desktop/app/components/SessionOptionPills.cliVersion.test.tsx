// @vitest-environment jsdom
//
// A model newer than the installed CLI: the model menu names the CLI version it
// needs and how to upgrade, and the pill keeps flagging it after the pick.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import "../i18n";
import { SessionOptionPills } from "./SessionOptionPills";
import { __resetModelCatalogCache } from "../useModelCatalog";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const LADDER = ["low", "medium", "high", "xhigh", "max"];
function catalog(cliVersion: string | null) {
  const below = (min: string) => cliVersion !== null && cliVersion < min;
  return [
    {
      name: "claude",
      available: true,
      cliVersion,
      upgradeCommand: cliVersion ? "claude update" : null,
      models: [
        {
          id: "claude-opus-5-5",
          label: "Opus 5.5",
          harness: "claude",
          tier: "premium",
          efforts: LADDER,
          defaultEffort: null,
          minCliVersion: "2.1.280",
          needsCliUpgrade: below("2.1.280"),
        },
        {
          id: "claude-sonnet-5-5",
          label: "Sonnet 5.5",
          harness: "claude",
          tier: "standard",
          efforts: LADDER,
          defaultEffort: null,
          minCliVersion: "2.1.284",
          needsCliUpgrade: below("2.1.284"),
        },
      ],
    },
    { name: "codex", available: false, cliVersion: null, upgradeCommand: null, models: [] },
    { name: "dsh", available: false, cliVersion: null, upgradeCommand: null, models: [] },
  ];
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  __resetModelCatalogCache();
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

async function render(cliVersion: string | null, model = "") {
  invoke.mockImplementation(async (cmd: string) =>
    cmd === "model_catalog" ? catalog(cliVersion) : null,
  );
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(
      <SessionOptionPills
        tool="claude"
        model={model}
        effort=""
        permissionMode=""
        onModelChange={() => {}}
        onEffortChange={() => {}}
        onPermissionModeChange={() => {}}
      />,
    );
  });
}

function modelPill(): HTMLButtonElement {
  return container!.querySelector<HTMLButtonElement>('[data-testid="model-pill"]')!;
}

async function openModelMenu() {
  await act(async () => modelPill().dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

describe("SessionOptionPills — CLI version floor", () => {
  it("marks models the installed CLI is too old for and names the upgrade", async () => {
    await render("2.1.280");
    await openModelMenu();
    const note = container!.querySelector('[data-testid="model-cli-outdated"]');
    expect(note?.textContent).toContain("2.1.280");
    expect(note?.textContent).toContain("claude update");

    const row = (label: string) =>
      [...container!.querySelectorAll('button[role="menuitem"]')].find((b) =>
        b.textContent?.startsWith(label),
      )!;
    expect(row("Sonnet 5.5").textContent).toContain("2.1.284");
    // Opus 5.5's floor is exactly 2.1.280, so it is not flagged.
    expect(row("Opus 5.5").textContent).toBe("Opus 5.5");
  });

  it("keeps flagging the picked model on the pill", async () => {
    await render("2.1.280", "claude-sonnet-5-5");
    expect(modelPill().textContent).toContain("⚠");
    expect(modelPill().title).toContain("2.1.284");
    expect(modelPill().title).toContain("claude update");
  });

  it("says nothing when the CLI is current or its version is unknown", async () => {
    await render("2.1.284", "claude-sonnet-5-5");
    expect(modelPill().textContent).not.toContain("⚠");
    await openModelMenu();
    expect(container!.querySelector('[data-testid="model-cli-outdated"]')).toBeNull();
  });

  it("stays quiet when the version could not be read", async () => {
    await render(null, "claude-sonnet-5-5");
    expect(modelPill().textContent).not.toContain("⚠");
  });
});
