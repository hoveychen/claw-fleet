// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const CHAT = "/Users/foo/.fleet/chat";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "chat_workspace") return CHAT;
    if (cmd === "get_sources_config") return [{ tool: "claude", enabled: true, installed: true }];
    return null;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import "../i18n";
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

// This jsdom build runs without localStorage (no --localstorage-file), and the
// form reads/writes the remembered last workspace through it.
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

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  window.localStorage.clear();
  useComposerDraftStore.setState({ drafts: {} });
  useSessionsStore.setState({ sessions: [] });
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function session(workspacePath: string): SessionInfo {
  return {
    id: workspacePath,
    workspacePath,
    workspaceName: workspacePath.split("/").pop() ?? workspacePath,
    lastActivityMs: 1,
    tokenSpeed: 0,
  } as unknown as SessionInfo;
}

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(<NewSessionForm onCreated={() => {}} onCancel={() => {}} />);
  });
  // Let the chat_workspace / sources round-trips land.
  await act(async () => {
    await Promise.resolve();
  });
}

function pill(): HTMLButtonElement {
  const el = container!.querySelector<HTMLButtonElement>('[data-testid="chat-mode-pill"]');
  expect(el, "chat-mode-pill not rendered").toBeTruthy();
  return el!;
}

/** The directory picker's label — it only renders outside chat mode, so this
 *  doubles as "the form is showing a launchable directory". */
function workspacePillLabel(): string {
  const el = container!.querySelector<HTMLElement>('[data-testid="workspace-pill"]');
  expect(el, "workspace pill not rendered").toBeTruthy();
  return (el!.textContent ?? "").trim();
}

async function click(el: HTMLElement) {
  await act(async () => el.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

/** The pure-chat pill is a toggle: whatever it can switch on it must switch back
 *  off. Both halves are exercised because the form's workspace seeding runs off
 *  the same field the pill writes, so either direction can be undone by it. */
describe("NewSessionForm chat-mode pill", () => {
  it("toggles off again on a host that has project workspaces", async () => {
    useSessionsStore.setState({ sessions: [session("/Users/foo/repo")] });
    await mount();
    await click(pill());
    expect(pill().getAttribute("aria-pressed")).toBe("true");
    await click(pill());
    expect(pill().getAttribute("aria-pressed")).toBe("false");
  });

  // The real desktop resolves `chat_workspace` (one invoke) well before the
  // session scan lands, so the form seeds chat first and learns about the repos
  // afterwards. With the last launched session being a chat one, the seeding
  // effect keeps that chat seed — which is how the form opens in chat mode on a
  // host that has plenty of repos.
  it("toggles off again when the form opened in chat mode after a chat launch", async () => {
    window.localStorage.setItem("fleet:last-new-session-workspace", CHAT);
    await mount();
    expect(pill().getAttribute("aria-pressed")).toBe("true");
    await act(async () => {
      useSessionsStore.setState({ sessions: [session("/Users/foo/repo")] });
    });
    expect(pill().getAttribute("aria-pressed")).toBe("true");
    await click(pill());
    expect(pill().getAttribute("aria-pressed")).toBe("false");
  });

  // The regression 老板 hit. Reopening the form (or restoring the draft tab)
  // mounts it with the chat path already in the shared draft, and
  // `chat_workspace` is one backend round-trip behind that first render. During
  // that window the form saw a workspace it couldn't yet recognise as chat and
  // filed it away as "the project to come back to" — so the off-switch handed
  // the chat path back to itself and the pill could never be switched off.
  it("toggles off again when the draft already held the chat path at mount", async () => {
    useSessionsStore.setState({ sessions: [session("/Users/foo/repo")] });
    // Chat was also the last thing launched, so the seeding effect keeps the
    // chat draft instead of treating it as a provisional pre-scan guess.
    window.localStorage.setItem("fleet:last-new-session-workspace", CHAT);
    useComposerDraftStore.setState({
      drafts: {
        new: {
          prompt: "",
          model: "",
          effort: "",
          permissionMode: "acceptEdits",
          attachments: [],
          workspace: CHAT,
          tool: "claude",
        },
      },
    });
    await mount();
    expect(pill().getAttribute("aria-pressed")).toBe("true");
    await click(pill());
    expect(pill().getAttribute("aria-pressed")).toBe("false");
    // And it lands on the actual repo, not on an empty picker with submit
    // disabled — leaving chat mode has to leave you somewhere launchable.
    expect(workspacePillLabel()).toContain("repo");
  });

  it("toggles off again on a host with no project workspaces yet", async () => {
    await mount();
    // Nothing else to launch into, so the form opens in chat mode already.
    expect(pill().getAttribute("aria-pressed")).toBe("true");
    await click(pill());
    expect(pill().getAttribute("aria-pressed")).toBe("false");
  });
});
