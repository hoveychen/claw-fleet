// @vitest-environment jsdom
//
// The rca entry point in the new-session composer.
//
// This exists because of a real report: with rca fully shipped and the DEV gate
// removed, the composer looked *byte for byte* like it always had. The reason
// was that the only rca row in the directory dropdown is generated per
// rca-capable host — and on an install with none, that's zero rows. So the
// capability was invisible precisely to the person who hadn't set it up yet,
// which is the only person who needs to be told it exists.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const CHAT = "/Users/foo/.fleet/chat";

/** Swapped per test to stand for "this machine has rca hosts" vs "it has none". */
let sshHosts: unknown[] = [];
const openSettings = vi.fn(async () => undefined);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "chat_workspace") return CHAT;
    if (cmd === "get_sources_config") return [{ tool: "claude", enabled: true, installed: true }];
    if (cmd === "list_ssh_hosts") return sshHosts;
    if (cmd === "list_remote_workspaces") return { workspaces: [] };
    if (cmd === "open_settings_window") return openSettings();
    return null;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import i18n from "../i18n";
import { NewSessionForm } from "./NewSessionForm";
import { useComposerDraftStore } from "../composerDraft";
import { useSessionsStore } from "../store";
import type { SessionInfo } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const store = new Map<string, string>();
Object.defineProperty(window, "localStorage", {
  configurable: true,
  value: {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  },
});

// `openSettingsWindow` resolves the theme, which reads `window.matchMedia` —
// absent in this jsdom build. Without the stub it throws before reaching invoke
// and the rejection is swallowed, so the row looks inert for a reason that has
// nothing to do with the row.
Object.defineProperty(window, "matchMedia", {
  configurable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
    onchange: null,
  }),
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  window.localStorage.clear();
  useComposerDraftStore.setState({ drafts: {} });
  // Without a project workspace the composer boots into pure-chat mode and the
  // directory pill (which owns the rca rows) never renders at all.
  useSessionsStore.setState({
    sessions: [
      {
        id: "/Users/foo/repo",
        workspacePath: "/Users/foo/repo",
        workspaceName: "repo",
        lastActivityMs: 1,
        tokenSpeed: 0,
      } as unknown as SessionInfo,
    ],
  });
  sshHosts = [];
  openSettings.mockClear();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(<NewSessionForm onCreated={() => {}} onCancel={() => {}} />);
  });
  await act(async () => {
    await Promise.resolve();
  });
}

async function click(el: HTMLElement) {
  await act(async () => el.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

/** Open the directory dropdown — the rca rows live in its footer, not on the
 *  composer's face, which is itself part of why nobody found them. */
async function openWorkspaceMenu() {
  const pill = container!.querySelector<HTMLButtonElement>('[data-testid="workspace-pill"]');
  expect(pill, "workspace pill not rendered").toBeTruthy();
  await click(pill!);
}

function menuText(): string {
  // The menu portals to the body on some platforms, so read the whole document.
  return document.body.textContent ?? "";
}

describe("NewSessionForm rca entry", () => {
  /** The regression this file exists for. */
  it("offers a way in even when no host has rca yet", async () => {
    sshHosts = [{ id: "h1", label: "bt-container", host: "", port: 22, username: "" }];
    await mount();
    await openWorkspaceMenu();
    expect(menuText()).toContain(i18n.t("new_session.add_remote_host"));
  });

  /** …and that way in has to actually go somewhere. Landing on the General tab
   *  and making the user hunt for "Integration" is the same discoverability
   *  problem one screen later. */
  it("takes you to the settings page that can add one", async () => {
    sshHosts = [];
    await mount();
    await openWorkspaceMenu();
    const label = i18n.t("new_session.add_remote_host");
    const row = [...document.querySelectorAll<HTMLElement>('button[role="menuitem"]')].find(
      (n) => (n.textContent ?? "").includes(label),
    );
    expect(row, "the add-host row is not a clickable menu item").toBeTruthy();
    await click(row!);
    // The settings window is asked to open ON the integration tab — the seam is
    // the localStorage key the settings window consumes on mount.
    expect(window.localStorage.getItem("settings-open-tab")).toBe("integration");
    // The row fires the window open without awaiting it (the menu must not hang
    // on a window), so let the invoke's microtask land before asserting on it.
    await act(async () => {
      await Promise.resolve();
    });
    expect(openSettings).toHaveBeenCalled();
  });

  /** The existing behaviour must survive: a machine that HAS an rca host still
   *  gets the per-host browse rows, not the add-a-host prompt. */
  it("lists the hosts themselves once rca is installed on one", async () => {
    sshHosts = [
      { id: "h1", label: "gpu-box", host: "gpu.example", port: 22, username: "root", rcaPath: "/root/.fleet/bin/rca" },
    ];
    await mount();
    await openWorkspaceMenu();
    expect(menuText()).toContain(i18n.t("new_session.browse_on_host", { host: "gpu-box" }));
    expect(menuText()).not.toContain(i18n.t("new_session.add_remote_host"));
  });
});
