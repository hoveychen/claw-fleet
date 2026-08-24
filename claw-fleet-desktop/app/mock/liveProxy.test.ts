// @vitest-environment jsdom
// `callProbe` resolves its URL against `window.location.origin`, so the
// composite test needs a DOM. The fs-based scanners work either way.
import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { LIVE_COMPOSITES, LIVE_ROUTES } from "./liveProxy";

/**
 * Commands the frontend reaches through a variable, not a literal — a ternary
 * (`op === "push" ? "git_push" : "git_pull"`) or a `cmd` parameter threaded
 * into a shared mutation helper. The scanner below only sees literals at the
 * `invoke(` call site, so these are listed here instead. Each is still checked
 * to appear *somewhere* in the app source, so a typo cannot hide in this list.
 */
const DYNAMICALLY_DISPATCHED = [
  "cancel_loop", // ScheduleView: task.kind === "loop" ? … : …
  "cancel_schedule",
  "git_push", // FilesView: op === "push" ? … : …
  "git_pull",
  "install_plugin", // PluginsView: runPluginMutation(cmd, args)
  "uninstall_plugin",
  "set_plugin_enabled",
  "test_decision_end_to_end", // SettingsPanel: kind → cmd chain
  "test_decision_via_claude_cli",
];

/**
 * A typo in a LIVE_ROUTES key is invisible at runtime: the command simply falls
 * through to the fixtures, and the harness quietly reports mock data as if it
 * were real. That is the exact failure this whole harness exists to prevent, so
 * pin every key to a command the frontend actually invokes.
 */
function collectInvokedCommands(dir: string, out = new Set<string>()): Set<string> {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === "generated") continue;
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) {
      collectInvokedCommands(p, out);
    } else if (/\.tsx?$/.test(entry)) {
      const src = readFileSync(p, "utf8");
      // The generic can hold an inline object type (`invoke<{ pid: number }>`),
      // so the parameter list must allow braces — `[^>]*`, not `[^>{}]*`.
      for (const m of src.matchAll(/invoke(?:<[^>]*>)?\(\s*"([a-z0-9_]+)"/g)) {
        out.add(m[1]);
      }
    }
  }
  return out;
}

/** Every string literal in the app source — the typo net for the list above. */
function collectStringLiterals(dir: string, out = new Set<string>()): Set<string> {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === "generated") continue;
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) {
      collectStringLiterals(p, out);
    } else if (/\.tsx?$/.test(entry)) {
      for (const m of readFileSync(p, "utf8").matchAll(/"([a-z0-9_]+)"/g)) {
        out.add(m[1]);
      }
    }
  }
  return out;
}

/**
 * Routes `hooks_server::serve` answers, resolved through the shared `routes`
 * module. Mirrors the extractor in `claw-fleet-core/tests/backend_drift_guard.rs`
 * (Check B), which does the same job for the Rust client.
 */
function servedRoutes(): { exact: Set<string>; prefixes: string[] } {
  const core = join(__dirname, "..", "..", "..", "claw-fleet-core", "src");
  const consts = new Map<string, string>();
  const routesRs = readFileSync(join(core, "routes.rs"), "utf8");
  for (const m of routesRs.matchAll(/pub const ([A-Z][A-Z0-9_]*): &str = "([^"]*)";/g)) {
    consts.set(m[1], m[2]);
  }
  const dir = join(core, "hooks_server");
  const src = readdirSync(dir)
    .filter((f) => f.endsWith(".rs"))
    .map((f) => readFileSync(join(dir, f), "utf8"))
    .join("\n");

  const exact = new Set<string>();
  const prefixes: string[] = [];
  // `"/path" =>` / `"/path" if …` / `"/path" |` match arms.
  for (const m of src.matchAll(/"(\/[^"]*)"\s*(?:=>|if\b|\|)/g)) {
    exact.add(m[1].split("?")[0]);
  }
  // The same arms after the migration to `routes::CONST`.
  for (const m of src.matchAll(/(?:crate::)?routes::([A-Z][A-Z0-9_]*)\s*(?:=>|if\b|\|)/g)) {
    const p = consts.get(m[1]);
    if (!p) continue;
    if (p.endsWith("/")) prefixes.push(p);
    else exact.add(p);
  }
  for (const m of src.matchAll(/starts_with\(\s*"(\/[^"]*)"/g)) prefixes.push(m[1]);
  for (const m of src.matchAll(/starts_with\(\s*(?:crate::)?routes::([A-Z][A-Z0-9_]*)/g)) {
    const p = consts.get(m[1]);
    if (p) prefixes.push(p);
  }
  return { exact, prefixes };
}

describe("live proxy route table", () => {
  const invoked = collectInvokedCommands(join(__dirname, ".."));

  it("finds the frontend's invoke() call sites at all (guards the scanner)", () => {
    expect(invoked.has("list_sessions")).toBe(true);
    expect(invoked.has("get_messages_tail")).toBe(true);
    expect(invoked.size).toBeGreaterThan(50);
  });

  it("maps only commands the frontend really invokes", () => {
    const known = new Set([...invoked, ...DYNAMICALLY_DISPATCHED]);
    const unknown = Object.keys(LIVE_ROUTES).filter((cmd) => !known.has(cmd));
    expect(unknown).toEqual([]);
  });

  it("the dynamic-dispatch escape hatch holds no typos", () => {
    const literals = collectStringLiterals(join(__dirname, ".."));
    // Each name must still exist verbatim in the source; and none of them may
    // be reachable as a literal `invoke(` (that would mean the list is stale).
    expect(DYNAMICALLY_DISPATCHED.filter((c) => !literals.has(c))).toEqual([]);
    expect(DYNAMICALLY_DISPATCHED.filter((c) => invoked.has(c))).toEqual([]);
  });

  /**
   * A path that no route serves 404s at runtime, and the frontend's own catch
   * turns that into an empty view rather than a visible error — indistinguishable
   * from "the host has no data". Three of these (`/daily_report/stats`,
   * `/mobile_relay/qr`, `/permission_prompt/respond`) were written by hand from
   * the route *name* rather than the const and were all wrong; this test is what
   * caught them.
   */
  it("only calls paths hooks_server actually serves", () => {
    const { exact, prefixes } = servedRoutes();
    expect(exact.size).toBeGreaterThan(80);
    const called = new Set<string>();
    for (const mapper of Object.values(LIVE_ROUTES)) {
      // Args are only read to build query/body strings, so a proxy of `{}` is
      // enough to get at the path (template paths interpolate to `undefined`,
      // which is why the source-prefix ones are matched by prefix below).
      called.add(mapper({}).path);
    }
    const unserved = [...called]
      .filter((p) => !exact.has(p) && !prefixes.some((pre) => p.startsWith(pre)))
      .sort();
    expect(unserved).toEqual([]);
  });

  /**
   * `wikiFileUrl` builds an `<iframe src>` by hand, so it never passes through
   * `LIVE_ROUTES` and the test above cannot see it. It still needs the server
   * to answer, and specifically at a *prefix*: the published `index.html`
   * reaches its bundle through relative refs, which the browser resolves
   * against the URL's directory. The existing `/wiki_file?slug=` query route
   * cannot serve that shape — one segment carrying the whole doc sends
   * `assets/style.css` somewhere else entirely.
   *
   * Asserted against the served prefix list rather than a substring of the
   * source: `expect(src).toContain("/wiki_asset")` would also pass for a route
   * named `/wiki_assets_typo`.
   */
  it("serves the wiki-asset prefix the web build's iframe src is built from", () => {
    const { prefixes } = servedRoutes();
    expect(prefixes).toContain("/wiki_asset/");
  });

  it("covers the session-data commands the detail view depends on", () => {
    for (const cmd of [
      "list_sessions",
      "get_messages",
      "get_messages_tail",
      "get_dsh_session_cost",
      "get_dsh_token_breakdown",
      "list_session_decisions",
    ]) {
      expect(Object.keys(LIVE_ROUTES)).toContain(cmd);
    }
  });

  /**
   * Body casing is per-struct, not per-surface. The one-off `struct Req` inside
   * a single `remote.rs` fn declares no `rename_all`, so its fields go out
   * snake_case — while the shared request types in `claw-fleet-core` are
   * camelCase. Sending the wrong one is a 400 with a body nobody reads
   * (`missing field user_title`), which is how the first four of these shipped.
   */
  it("keeps snake_case bodies snake_case", () => {
    expect(LIVE_ROUTES.apply_interaction_mode({}).body).toHaveProperty("user_title");
    expect(LIVE_ROUTES.apply_prd_mode({}).body).toHaveProperty("user_title");
    expect(LIVE_ROUTES.reconcile_codex_guidance({}).body).toHaveProperty("user_title");
    expect(LIVE_ROUTES.install_plugin({ pluginId: "p" }).body).toEqual({ plugin_id: "p" });
    expect(LIVE_ROUTES.uninstall_plugin({ pluginId: "p" }).body).toEqual({ plugin_id: "p" });
    expect(LIVE_ROUTES.set_plugin_enabled({ pluginId: "p", enabled: true }).body).toEqual({
      plugin_id: "p",
      enabled: true,
    });
    expect(LIVE_ROUTES.delete_skill({ skillPath: "/s" }).body).toEqual({ skill_path: "/s" });
  });

  /**
   * The three host-settings POSTs pass the config object straight through, so
   * the only thing that can be wrong is which IPC arg it comes from — and the
   * frontend is not consistent: `set_auto_resume_config` sends `{ config }`
   * while the other two send `{ cfg }`. Reading the wrong one posts `undefined`,
   * which serializes to no body at all and 400s on the server.
   */
  it("takes each host-settings body from the arg name the caller uses", () => {
    expect(LIVE_ROUTES.set_auto_resume_config({ config: { enabled: false } }).body).toEqual({
      enabled: false,
    });
    expect(LIVE_ROUTES.set_permissions_config({ cfg: { enabled: true } }).body).toEqual({
      enabled: true,
    });
    expect(
      LIVE_ROUTES.set_decision_panel_config({ cfg: { wait_seconds: 600 } }).body,
    ).toEqual({ wait_seconds: 600 });
  });

  it("keeps camelCase bodies camelCase", () => {
    // `SpawnSessionRequest` / `SetSessionMarkRequest` are `rename_all = "camelCase"`.
    expect(LIVE_ROUTES.spawn_new_claude_session({ workspacePath: "/w" }).body).toMatchObject({
      workspacePath: "/w",
    });
    expect(
      LIVE_ROUTES.set_session_mark({ sessionId: "s", workspacePath: "/w", mark: "star" }).body,
    ).toMatchObject({ sessionId: "s", workspacePath: "/w" });
  });

  /**
   * Routes that answer a JSON *envelope* while the command returns one field of
   * it. `RemoteBackend` unwraps these in Rust — e.g. `/chat_workspace` answers
   * `{"path": …}` and `fn chat_workspace` deserializes into a local `Resp` and
   * returns `resp.path` — so a mapper without `pick` hands the caller an object
   * where it declared a string.
   *
   * That is not a cosmetic mismatch. `useChatWorkspace` stores the value and
   * `NewSessionForm` puts it in `setWorkspace`; picking the chat pill then made
   * the whole page go blank, because something downstream does string work on
   * an object and React unmounts the tree mid-render.
   *
   * To re-derive the list after a `remote.rs` change: find every method in
   * `impl Backend for RemoteBackend` that declares a local `#[derive(Deserialize)]`
   * struct and returns one of its fields. As of this test that is
   * `analyze_guard_command`, `chat_workspace`, `get_claude_binary_override`,
   * `get_skill_autosync`, `mobile_relay_qr_svg`, `upload_attachment` — the last
   * two of those are the QR (already picked) and the attachment upload (not
   * routed in the browser build at all), and `get_skill_autosync` has no
   * frontend caller.
   */
  it("unwraps the envelope routes the same field RemoteBackend does", () => {
    expect(LIVE_ROUTES.chat_workspace({}).pick).toBe("path");
    expect(LIVE_ROUTES.get_claude_binary_override({}).pick).toBe("path");
    expect(LIVE_ROUTES.analyze_guard_command({}).pick).toBe("analysis");
    expect(LIVE_ROUTES.mobile_relay_qr_svg({}).pick).toBe("svg");
  });

  /**
   * A pasted screenshot has no path anywhere — not on the host, not in the
   * page. The desktop parks the bytes in `$TMPDIR/fleet-pasted` and lets a
   * second command move them into the store; the browser build has no host
   * temp dir to park in, so the one POST has to land them in the store
   * directly. Hence `from_clipboard=1`, which is what selects the store over
   * the temp dir on the route's side.
   *
   * The bytes go as the raw body, not inside `body`: JSON-encoding a 3 MB
   * screenshot as an array of integers is ~4x the bytes for a route that reads
   * the body verbatim.
   */
  it("posts pasted bytes into the persistent store, not as JSON", () => {
    const req = LIVE_ROUTES.stage_pasted_attachment({ bytes: [1, 2, 3], extension: "png" });
    expect(req.method).toBe("POST");
    expect(req.path).toBe("/elicitation/upload");
    expect(req.query?.from_clipboard).toBe("1");
    expect(String(req.query?.name)).toMatch(/\.png$/);
    expect(req.body).toBeUndefined();
    expect([...(req.rawBody as Uint8Array)]).toEqual([1, 2, 3]);
    // The route answers `{"path": …}`; the command returns the bare path.
    expect(req.pick).toBe("path");
  });

  it("builds the tail request the store issues", () => {
    expect(LIVE_ROUTES.get_messages_tail({ jsonlPath: "dsh://s-1", tail: 150 })).toEqual({
      method: "GET",
      path: "/messages",
      query: { path: "dsh://s-1", tail: "150" },
    });
  });
});

/**
 * `list_pending_decisions` is the frontend's *mount catch-up*: Tauri events are
 * not buffered, so a card raised before the page loaded is only ever seen
 * through this one call. `RemoteBackend` builds it by fanning out to all six
 * `/…/pending` endpoints and then filling in each request's display fields from
 * the session list — it is not one route, and mapping it to `/guard/pending`
 * alone left five of the six buckets permanently empty *and* handed the caller
 * a bare array where it reads `p.elicitation` / `p.fleetAsk` / ….
 *
 * `useDecisionEvents` guards with `p.guard?.forEach`, so the wrong shape did
 * not throw — it silently did nothing, which is why a click-through never
 * caught it.
 */
/**
 * Query encoding, which is not a detail: `hooks_server::parse_query` splits on
 * `&` / `=` and hands each raw value to `percent_decode_str`. Percent-decoding
 * does not touch `+`, so a value serialized the *form* way — which is what
 * `URLSearchParams` does — arrives with a literal plus where the space was.
 *
 * `RemoteBackend`, the surface this table mirrors, percent-encodes with
 * `NON_ALPHANUMERIC` and so sends `%20`. Anything named with a space (`/Users/x/
 * My Project`, `my shot.png`) is the whole failure mode: the route looks up a
 * path that does not exist and answers 404 or empty, and the UI shows it as
 * "no data".
 */
/**
 * The browser build's replacement for the desktop's "pick a destination, then
 * write there" export. `save()` answers null in a tab, so the old path returned
 * one line later having silently done nothing.
 *
 * Pinned here rather than in `WikiView` because the failure mode is the *path*:
 * a route nobody serves 404s, `exportDoc`'s caller shows that as a failed
 * export, and no test would have caught the typo — the same class the
 * `LIVE_ROUTES` path sweep above exists for, which does not see this literal
 * because it is not a mapped command.
 */
describe("downloadWikiExport", () => {
  it("calls a path hooks_server serves, with the slug percent-encoded", async () => {
    expect(servedRoutes().exact).toContain("/wiki_export");

    const seen: string[] = [];
    const realFetch = globalThis.fetch;
    const realCreate = URL.createObjectURL;
    const realRevoke = URL.revokeObjectURL;
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      seen.push(String(input));
      return new Response(new Blob(["# doc"]), { status: 200 });
    }) as typeof fetch;
    URL.createObjectURL = () => "blob:stub";
    URL.revokeObjectURL = () => {};
    try {
      const { downloadWikiExport } = await import("./liveProxy");
      await downloadWikiExport("arch/my notes", "v2", "notes.md");
    } finally {
      globalThis.fetch = realFetch;
      URL.createObjectURL = realCreate;
      URL.revokeObjectURL = realRevoke;
    }

    expect(seen).toHaveLength(1);
    // The whole pathname, not a substring: `toContain("/wiki_export")` is also
    // satisfied by a typo'd `/wiki_exportt`, which is exactly the mistake this
    // is here to catch (verified by mutating the literal).
    const url = new URL(seen[0]);
    expect(url.pathname.replace(/^\/__live/, "")).toBe("/wiki_export");
    expect(url.search).toContain("my%20notes");
    expect(url.search).toContain("version=v2");
  });
});

/**
 * The bundled Fleet SKILL.md, downloadable in the browser build.
 *
 * On the desktop `save_skill_file` writes a compile-time `include_str!`
 * constant to a path the user picks — the frontend never holds the text and a
 * tab has no path to write to, so this is the one gap in the export family that
 * needed a route rather than a re-routing. Pinned the same way as the wiki one:
 * the whole pathname, against what `hooks_server` really serves.
 */
describe("downloadFleetSkill", () => {
  it("calls a path hooks_server actually serves", async () => {
    expect(servedRoutes().exact).toContain("/fleet_skill");

    const seen: string[] = [];
    const realFetch = globalThis.fetch;
    const realCreate = URL.createObjectURL;
    const realRevoke = URL.revokeObjectURL;
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      seen.push(String(input));
      return new Response(new Blob(["# Fleet"]), { status: 200 });
    }) as typeof fetch;
    URL.createObjectURL = () => "blob:stub";
    URL.revokeObjectURL = () => {};
    try {
      const { downloadFleetSkill } = await import("./liveProxy");
      await downloadFleetSkill();
    } finally {
      globalThis.fetch = realFetch;
      URL.createObjectURL = realCreate;
      URL.revokeObjectURL = realRevoke;
    }

    expect(seen).toHaveLength(1);
    expect(new URL(seen[0]).pathname.replace(/^\/__live/, "")).toBe("/fleet_skill");
  });
});

describe("query encoding matches RemoteBackend", () => {
  it("sends a space as %20, not as +", async () => {
    const seen: string[] = [];
    const realFetch = globalThis.fetch;
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      seen.push(String(input));
      return new Response("[]", { status: 200 });
    }) as typeof fetch;
    try {
      const { liveInvoke } = await import("./liveProxy");
      await liveInvoke("get_messages", { jsonlPath: "/Users/x/My Project/a b.jsonl" });
    } finally {
      globalThis.fetch = realFetch;
    }
    expect(seen).toHaveLength(1);
    expect(seen[0]).toContain("My%20Project");
    expect(seen[0]).not.toContain("+");
  });
});

describe("list_pending_decisions composite", () => {
  // Paths carry the `/__live` dev-proxy prefix, so match on the suffix.
  const bucketFor = (path: string) => {
    if (path.endsWith("/guard/pending")) return [mk("g1", "guard-ws")];
    if (path.endsWith("/elicitation/pending")) return [mk("e1", "")];
    if (path.endsWith("/fleet-ask/pending")) return [mk("f1", "")];
    if (path.endsWith("/a2ui-render/pending")) return [mk("a1", "")];
    if (path.endsWith("/plan-approval/pending")) return [mk("p1", "")];
    if (path.endsWith("/permission-prompt/pending")) return [mk("m1", "")];
    if (path.endsWith("/sessions")) {
      return [{ id: "s1", workspaceName: "resolved-ws", aiTitle: "resolved-title" }];
    }
    return [];
  };
  function mk(id: string, workspaceName: string) {
    return { id, sessionId: "s1", workspaceName, aiTitle: null };
  }

  it("fans out to all six pending routes and resolves display fields", async () => {
    const seen: string[] = [];
    const realFetch = globalThis.fetch;
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const path = new URL(String(input), "http://localhost").pathname;
      seen.push(path);
      return new Response(JSON.stringify(bucketFor(path)), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }) as typeof fetch;
    try {
      const composite = LIVE_COMPOSITES.list_pending_decisions;
      expect(composite, "list_pending_decisions must be a composite").toBeTypeOf("function");
      const out = (await composite({})) as Record<string, Array<Record<string, unknown>>>;

      // All six buckets, under the camelCase names the hook destructures.
      for (const k of [
        "guard",
        "elicitation",
        "fleetAsk",
        "a2uiRender",
        "planApproval",
        "permissionPrompt",
      ]) {
        expect(out[k], `bucket ${k}`).toHaveLength(1);
      }
      expect(seen.some((p) => p.endsWith("/elicitation/pending"))).toBe(true);
      expect(seen.some((p) => p.endsWith("/permission-prompt/pending"))).toBe(true);

      // `resolve_pending_display`: fill an empty workspaceName / missing aiTitle
      // from the session, and leave a value that is already set alone.
      expect(out.elicitation[0].workspaceName).toBe("resolved-ws");
      expect(out.elicitation[0].aiTitle).toBe("resolved-title");
      expect(out.guard[0].workspaceName).toBe("guard-ws");
    } finally {
      globalThis.fetch = realFetch;
    }
  });

  it("is no longer mapped as a single route", () => {
    expect(LIVE_ROUTES.list_pending_decisions).toBeUndefined();
  });
});
