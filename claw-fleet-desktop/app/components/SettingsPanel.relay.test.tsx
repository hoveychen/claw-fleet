// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import i18n from "../i18n";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: () => Promise.resolve(true),
  requestPermission: () => Promise.resolve("granted"),
}));

import { setWebApiPrefix } from "../hostEnv";
import { SettingsPanel } from "./SettingsPanel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// The panel's dynamic `import("@tauri-apps/api/event")` reaches the real
// module; give its `listen` / unlisten the internals they touch.
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
  transformCallback: () => 0,
  invoke: () => Promise.resolve(0),
};
(window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};

// This jsdom build runs without localStorage (no --localstorage-file); the
// panel keeps its tristates and local prefs there.
const store = new Map<string, string>();
Object.defineProperty(window, "localStorage", {
  configurable: true,
  value: {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  },
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;

/** Host values deliberately set away from the panel's built-in defaults. */
const HOST: Record<string, unknown> = {
  get_auto_resume_config: {
    enabled: false,
    maxWaitHours: 5,
    retryServerErrors: false,
    maxServerErrorRetries: 3,
  },
  get_plan_revive_config: { enabled: false },
  get_resume_triggers_config: {
    finishContinue: false,
    codexStallWatchdog: false,
    planGate: false,
    handoffSuccessor: false,
  },
};

function answer(overrides: Record<string, unknown> = {}, denied: string[] = []) {
  const table = { ...HOST, ...overrides };
  invoke.mockImplementation(async (cmd: string) => {
    if (!denied.includes(cmd) && cmd in table) return table[cmd];
    // What the relay transport surfaces for a 403 / unmapped command.
    throw new Error(`denied: ${cmd}`);
  });
}

async function render() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(<SettingsPanel onClose={() => {}} />);
  });
  // Let every mount-time read settle.
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
  return container;
}

/** The settings row whose label reads `label`. */
function row(el: HTMLElement, key: string): HTMLElement {
  const label = i18n.t(key);
  const span = Array.from(el.querySelectorAll("span")).find((s) => s.textContent === label);
  if (!span) throw new Error(`no row labelled ${label}`);
  // Start above the label: its own `row_label` class would match itself.
  const r = span.parentElement?.closest("[class*='row']:not([class*='row_label'])");
  if (!r) throw new Error(`no row element for ${label}`);
  return r as HTMLElement;
}

beforeEach(() => {
  invoke.mockReset();
  window.localStorage.clear();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  setWebApiPrefix("");
});

describe("SettingsPanel over the relay tunnel", () => {
  it("shows the host's real values read-only", async () => {
    setWebApiPrefix("/desktop/h");
    answer();
    const el = await render();

    expect(el.querySelector('[data-testid="settings-relay-readonly"]')).not.toBeNull();
    const fieldset = el.querySelector("fieldset")!;
    expect(fieldset.disabled).toBe(true);

    for (const key of [
      "settings.auto_resume",
      "settings.retry_server_errors",
      "settings.plan_revive",
      "settings.finish_continue",
      "settings.plan_gate",
      "settings.handoff_successor",
      "settings.codex_stall_watchdog",
    ]) {
      const input = row(el, key).querySelector<HTMLInputElement>("input[type=checkbox]");
      expect(input, key).not.toBeNull();
      expect(input!.checked, key).toBe(false);
      expect(input!.closest("fieldset[disabled]"), key).not.toBeNull();
    }
  });

  it("does not try to save when a disabled switch is clicked", async () => {
    setWebApiPrefix("/desktop/h");
    answer();
    const el = await render();
    const input = row(el, "settings.auto_resume").querySelector<HTMLInputElement>("input")!;
    await act(async () => {
      input.click();
    });
    expect(input.checked).toBe(false);
    expect(invoke.mock.calls.map((c) => c[0])).not.toContain("set_auto_resume_config");
    // The mount-time notification-mode push is a write too.
    expect(invoke.mock.calls.map((c) => c[0])).not.toContain("set_notification_mode");
  });

  it("renders a denied read as unknown, never as the default", async () => {
    setWebApiPrefix("/desktop/h");
    answer({}, ["get_auto_resume_config", "get_resume_triggers_config"]);
    const el = await render();

    for (const key of ["settings.auto_resume", "settings.retry_server_errors", "settings.plan_gate"]) {
      const r = row(el, key);
      expect(r.querySelector("input[type=checkbox]"), key).toBeNull();
      expect(r.querySelector('[data-testid="settings-unknown"]'), key).not.toBeNull();
    }
    // A read that did succeed still shows its value.
    const revive = row(el, "settings.plan_revive").querySelector<HTMLInputElement>("input");
    expect(revive?.checked).toBe(false);
    // The max-wait field hangs off auto-resume being on; unknown is not on.
    expect(el.textContent).not.toContain(i18n.t("settings.auto_resume_max_wait"));
  });
});

describe("SettingsPanel on the desktop", () => {
  it("keeps controls live and saves on change", async () => {
    answer({ set_auto_resume_config: null });
    const el = await render();

    expect(el.querySelector('[data-testid="settings-relay-readonly"]')).toBeNull();
    expect(el.querySelector("fieldset")!.disabled).toBe(false);
    const input = row(el, "settings.auto_resume").querySelector<HTMLInputElement>("input")!;
    await act(async () => {
      input.click();
    });
    expect(invoke.mock.calls.map((c) => c[0])).toContain("set_auto_resume_config");
  });
});
