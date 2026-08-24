import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { LIVE_ROUTES } from "./liveProxy";

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

  it("builds the tail request the store issues", () => {
    expect(LIVE_ROUTES.get_messages_tail({ jsonlPath: "dsh://s-1", tail: 150 })).toEqual({
      method: "GET",
      path: "/messages",
      query: { path: "dsh://s-1", tail: "150" },
    });
  });
});
