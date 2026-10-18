// @vitest-environment jsdom
//
// First-run "What should get done?" card: the project candidates come from the
// registry merged with scanned sessions, the most recent one is preselected,
// and submitting hands back the words plus the picked directory.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn(async (_cmd: string) => null as unknown));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import "../i18n";
import { useSessionsStore } from "../store";
import { MAX_PROJECT_CANDIDATES, OnboardingLaunch } from "./OnboardingLaunch";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

async function mount(onLaunch: (prompt: string, workspace: string | null) => void) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(<OnboardingLaunch onLaunch={onLaunch} launching={false} />);
  });
}

function chips(): HTMLButtonElement[] {
  return [...container!.querySelectorAll<HTMLButtonElement>("[role=radio]")];
}

function type(text: string) {
  const input = container!.querySelector("input")!;
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  act(() => {
    setter.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function start() {
  act(() => {
    container!.querySelector<HTMLButtonElement>("[data-testid=onboarding-launch]")!.click();
  });
}

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "list_projects") return [{ path: "/w/old", name: "old", createdAtMs: 1 }];
    if (cmd === "chat_workspace") return "/w/.chat";
    return null;
  });
  useSessionsStore.setState({
    sessions: [
      { workspacePath: "/w/fresh", workspaceName: "fresh", isSubagent: false, procAlive: true, lastActivityMs: 100 },
      { workspacePath: "/tmp/scratch", workspaceName: "scratch", isSubagent: false, procAlive: false, lastActivityMs: 200 },
    ] as never,
  });
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  useSessionsStore.setState({ sessions: [] });
});

describe("first-run launch card", () => {
  it("offers recent directories, most recent first, never temp dirs", async () => {
    await mount(() => {});
    const names = chips().map((c) => c.title);
    // Two projects plus the "choose later" chip (which has no title).
    expect(names).toEqual(["/w/fresh", "/w/old", ""]);
    expect(chips()[0].getAttribute("aria-checked")).toBe("true");
  });

  it("launches with the preselected project", async () => {
    const onLaunch = vi.fn();
    await mount(onLaunch);
    type("  fix the flaky test  ");
    start();
    expect(onLaunch).toHaveBeenCalledWith("fix the flaky test", "/w/fresh");
  });

  it("launches with no project when 'choose later' is picked", async () => {
    const onLaunch = vi.fn();
    await mount(onLaunch);
    act(() => chips()[2].click());
    start();
    expect(onLaunch).toHaveBeenCalledWith("", null);
  });

  it("caps the candidates", async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "list_projects"
        ? Array.from({ length: 10 }, (_, i) => ({ path: `/w/p${i}`, name: `p${i}`, createdAtMs: i }))
        : null,
    );
    await mount(() => {});
    expect(chips()).toHaveLength(MAX_PROJECT_CANDIDATES + 1);
  });
});
