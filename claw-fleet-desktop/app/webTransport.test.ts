// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { localCommand } from "./webTransport";
import { LIVE_COMPOSITES, LIVE_ROUTES } from "./mock/liveProxy";

/**
 * `@tauri-apps/plugin-store`'s JS half expects specific shapes back from its
 * commands, not merely "something". Returning the wrong shape doesn't throw —
 * `initStorage()` simply never settles, `boot()` stops before
 * `ReactDOM.render`, and the page stays blank with no console error. That is
 * exactly how it failed the first time, so the contract is pinned here.
 *
 * The shapes are the ones `mock/tauri-mock.ts` documents ("must match expected
 * return types"), which is the only written record of them.
 */
/**
 * This jsdom build exposes `window` but no `localStorage` (verified: it comes
 * back `undefined` even with a real `location.href`), so the suite brings its
 * own minimal one.
 */
function installLocalStorage() {
  const store = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
      setItem: (k: string, v: string) => void store.set(k, String(v)),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
    },
  });
}

describe("plugin:store contract", () => {
  beforeEach(() => {
    installLocalStorage();
  });

  it("load returns a numeric resource id, not null", () => {
    const { handled, value } = localCommand("plugin:store|load", {});
    expect(handled).toBe(true);
    expect(typeof value).toBe("number");
  });

  it("get returns a [value, exists] tuple", () => {
    localCommand("plugin:store|set", { key: "lang", value: "zh" });

    const hit = localCommand("plugin:store|get", { key: "lang" });
    expect(hit.value).toEqual(["zh", true]);

    // A missing key must report existence as false rather than be absent.
    const miss = localCommand("plugin:store|get", { key: "nope" });
    expect(miss.value).toEqual([null, false]);
  });

  it("set then delete round-trips through localStorage", () => {
    localCommand("plugin:store|set", { key: "viewMode", value: "gallery" });
    expect(localCommand("plugin:store|get", { key: "viewMode" }).value).toEqual([
      "gallery",
      true,
    ]);

    localCommand("plugin:store|delete", { key: "viewMode" });
    expect(localCommand("plugin:store|get", { key: "viewMode" }).value).toEqual([
      null,
      false,
    ]);
  });

  it("collection and scalar queries return their own types", () => {
    // Each of these is destructured or compared by the plugin; a blanket null
    // would break the caller in a different way for each one.
    expect(localCommand("plugin:store|entries", {}).value).toEqual([]);
    expect(localCommand("plugin:store|keys", {}).value).toEqual([]);
    expect(localCommand("plugin:store|values", {}).value).toEqual([]);
    expect(localCommand("plugin:store|length", {}).value).toBe(0);
    expect(localCommand("plugin:store|has", { key: "x" }).value).toBe(false);
  });

  it("keeps its own namespace so mock fixtures cannot bleed in", () => {
    window.localStorage.setItem("mock-store:lang", "en");
    localCommand("plugin:store|set", { key: "lang", value: "zh" });
    expect(window.localStorage.getItem("mock-store:lang")).toBe("en");
    expect(localCommand("plugin:store|get", { key: "lang" }).value).toEqual(["zh", true]);
  });
});

describe("host facts", () => {
  it("reports the browser build honestly instead of impersonating a platform", () => {
    expect(localCommand("get_platform", {}).value).toBe("web");
  });

  it("leaves unknown commands unhandled so the caller can record the gap", () => {
    expect(localCommand("no_such_command", {}).handled).toBe(false);
  });

  /**
   * The three host-settings pairs go to the host over HTTP; answering them
   * locally would be inventing a value for something that lives on the server
   * (and the writes would land nowhere). They must stay out of `localCommand`
   * so `LIVE_ROUTES` gets them.
   */
  it.each([
    "get_permissions_config",
    "set_permissions_config",
    "get_decision_panel_config",
    "set_decision_panel_config",
    "get_auto_resume_config",
    "set_auto_resume_config",
  ])("%s is left to the probe rather than answered locally", (cmd) => {
    expect(localCommand(cmd, {}).handled).toBe(false);
  });

  it("reports no pending app update instead of rejecting the boot check", () => {
    // `App.tsx` reads `has_update` — snake_case, unlike most of the surface.
    expect(localCommand("check_app_version", {}).value).toEqual({
      has_update: false,
      latest_version: "",
      release_url: "",
    });
  });
});

/**
 * A blanket `null` for unmapped commands is only safe for callers that ignore
 * the result. Anything a component reads `.length` off during render must come
 * back as a list, or React throws mid-render and the whole tree unmounts to an
 * empty root — with `boot()` still reporting success, which is exactly how this
 * first showed up.
 *
 * `ConnectionDialog` does `conns.length === 0` on the value of
 * `list_saved_connections`, so these two are load-bearing.
 */
describe("list-shaped commands never answer null", () => {
  it.each([
    "list_saved_connections",
    "list_ssh_profiles",
    // `store.ts` assigns this straight into the `alerts` array slot.
    "get_waiting_alerts",
    // `audio.ts` maps over the voices; `AccountInfo` over the tools.
    "get_tts_voices",
    "detect_ai_tools",
  ])("%s returns an array", (cmd) => {
    const { handled, value } = localCommand(cmd, {});
    expect(handled).toBe(true);
    expect(Array.isArray(value)).toBe(true);
  });

  it("generate_mascot_quips keeps both quip lists", () => {
    expect(localCommand("generate_mascot_quips", {}).value).toEqual({
      busy: [],
      idle: [],
    });
  });
});

/**
 * `getCurrentWindow()` is called on the boot path (theme sync) and by the
 * custom titlebar. Every query in the family has to come back with the type
 * its caller destructures — an unhandled one is a rejected promise inside the
 * titlebar's effect, and a null where a size is expected throws on `.width`.
 */
describe("plugin:window family", () => {
  it("answers the boolean queries as booleans", () => {
    for (const op of ["is_visible", "is_focused", "is_maximized", "is_minimized"]) {
      const { handled, value } = localCommand(`plugin:window|${op}`, {});
      expect(handled).toBe(true);
      expect(typeof value).toBe("boolean");
    }
  });

  it("answers a size as a {width,height} pair", () => {
    const { value } = localCommand("plugin:window|inner_size", {});
    expect(value).toHaveProperty("width");
    expect(value).toHaveProperty("height");
  });

  it("answers every other window op rather than reporting a gap", () => {
    for (const op of ["set_theme", "minimize", "toggle_maximize", "start_dragging", "close"]) {
      expect(localCommand(`plugin:window|${op}`, {}).handled).toBe(true);
    }
  });
});

/**
 * The completeness guard: every command the frontend can invoke has to be
 * *classified* — routed to the probe, composed from other routes, answered
 * locally, or listed below as a known gap. Without this, adding a command to
 * the frontend silently adds a browser-build gap that only a manual click-
 * through of every view would find.
 */
const KNOWN_WEB_GAPS = [
  // Reach a workspace file through `memory::` instead of the Backend trait, so
  // there is no HTTP shape to mirror.
  "get_claude_md_content",
  "promote_memory",
  // Write to the caller's own machine, which is a tab. Still invoked from the
  // desktop branch of the same components, so still scanned — but the browser
  // build no longer *offers* them: the AccountInfo panel hides both install
  // steps, since "put this on the machine you are sitting at" has no meaning
  // when the machine that matters is the one serving the page.
  "install_fleet_cli",
  "install_fleet_skill",
  "apply_mcp_injector",
  // Write to a destination the *user* picks on the caller's filesystem, which a
  // tab cannot offer. Both are reached only from the desktop branch now: the
  // browser build downloads the artifact instead (`downloadWikiExport` /
  // `downloadFleetSkill`), which is the browser's version of the same intent.
  "export_wiki_doc",
  // Same shape: writes to a path the user picks on the caller's filesystem.
  // The browser build downloads the blob instead (`downloadArtifact`).
  "export_artifact",
  // Opens the blob with the host shell's default application. Never reached in
  // a tab: the button sits behind `artifact_local_path`, which answers null
  // here (see above), so the browser build offers the download instead. A
  // local no-op would be worse than a gap — the call site reports success on a
  // resolved promise, and nothing would have opened.
  "open_artifact_external",
  // Emit onto the desktop's app-event bus, or have no RemoteBackend override.
  "test_decision_frontend_only",
  "test_fleet_ask_end_to_end",
  "test_fleet_ask_via_claude_cli",
  // SSH connection management — a tab only ever talks to the server that
  // served it, so there is nothing to connect, disconnect or install.
  "connect_remote",
  "disconnect_remote",
  "delete_connection",
  "install_rca_remote",
  // Same shape: opens an ssh connection FROM the caller's machine to install
  // rca there. A tab has no ssh client and no keys.
  "install_rca_on_host",
  "update_rca_remote",
];

/**
 * Handled, but their whole body *is* a side effect on the real window
 * (`location.reload()` / `print()` / `open()`), so the coverage sweep below
 * asserts them from this list instead of calling them and making jsdom log a
 * "Not implemented" for each run.
 */
const SIDE_EFFECT_ONLY = ["restart_app", "print_webview", "open_settings_window"];

/** Same scan as `liveProxy.test.ts`, over the whole app. */
function invokedCommands(dir: string, out = new Set<string>()): Set<string> {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === "generated") continue;
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) {
      invokedCommands(p, out);
    } else if (/\.tsx?$/.test(entry)) {
      for (const m of readFileSync(p, "utf8").matchAll(/invoke(?:<[^>]*>)?\(\s*"([a-z0-9_]+)"/g)) {
        out.add(m[1]);
      }
    }
  }
  return out;
}

/**
 * The two halves of the attachment handshake. On the desktop they are a pair:
 * `stage_pasted_attachment` writes bytes to the host's temp dir and
 * `upload_elicitation_attachment` decides where they finally live (the store
 * for a paste, the picked path left alone for a file).
 *
 * The browser build collapses that: there is no host temp dir a tab can write
 * to, so the staging POST already lands in the store (see the
 * `stage_pasted_attachment` route), and everything reaching the second command
 * is *already* a path on the host that serves this page. Which makes it the
 * same situation `LocalBackend` is in — its `upload_attachment` returns a
 * picked path untouched because the agent runs on that very machine.
 */
describe("attachment resolve is a pass-through in the browser build", () => {
  it("hands back the path it was given", () => {
    const p = "/home/u/.fleet/user-attachments/ab12/shot.png";
    const { handled, value } = localCommand("upload_elicitation_attachment", {
      sourcePath: p,
      fromClipboard: true,
    });
    expect(handled).toBe(true);
    expect(value).toBe(p);
  });

  it("is a path for a picked file too, not only a paste", () => {
    expect(
      localCommand("upload_elicitation_attachment", {
        sourcePath: "/srv/repo/notes.md",
        fromClipboard: false,
      }).value,
    ).toBe("/srv/repo/notes.md");
  });

  /**
   * A rejection would land in the composer's catch and show the red banner; an
   * empty string would silently splice `- ` into the prompt's `Context files:`
   * block. Neither is right, so an argument-less call must still be *handled* —
   * this only guards the completeness sweep below from a lucky pass.
   */
  it("stays handled with no source path rather than inventing one", () => {
    const { handled, value } = localCommand("upload_elicitation_attachment", {});
    expect(handled).toBe(true);
    expect(value).toBe("");
  });
});

describe("browser-build command coverage", () => {
  it("classifies every command the frontend invokes", () => {
    const invoked = [...invokedCommands(join(__dirname))];
    expect(invoked.length).toBeGreaterThan(150);

    const unclassified = invoked.filter(
      (cmd) =>
        !(cmd in LIVE_ROUTES) &&
        !(cmd in LIVE_COMPOSITES) &&
        // `start/stop_watching_session` are intercepted inside `liveInvoke`
        // (they become pollers), ahead of the route table.
        cmd !== "start_watching_session" &&
        cmd !== "stop_watching_session" &&
        !SIDE_EFFECT_ONLY.includes(cmd) &&
        !localCommand(cmd, {}).handled &&
        !KNOWN_WEB_GAPS.includes(cmd),
    );
    expect(unclassified).toEqual([]);
  });

  it("keeps the gap list free of entries that are now covered", () => {
    const covered = KNOWN_WEB_GAPS.filter(
      (cmd) => cmd in LIVE_ROUTES || cmd in LIVE_COMPOSITES || localCommand(cmd, {}).handled,
    );
    expect(covered).toEqual([]);
  });
});
