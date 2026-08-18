import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { LIVE_ROUTES } from "./liveProxy";

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
      for (const m of src.matchAll(/invoke(?:<[^>]*>)?\(\s*"([a-z0-9_]+)"/g)) {
        out.add(m[1]);
      }
    }
  }
  return out;
}

describe("live proxy route table", () => {
  const invoked = collectInvokedCommands(join(__dirname, ".."));

  it("finds the frontend's invoke() call sites at all (guards the scanner)", () => {
    expect(invoked.has("list_sessions")).toBe(true);
    expect(invoked.has("get_messages_tail")).toBe(true);
    expect(invoked.size).toBeGreaterThan(50);
  });

  it("maps only commands the frontend really invokes", () => {
    const unknown = Object.keys(LIVE_ROUTES).filter((cmd) => !invoked.has(cmd));
    expect(unknown).toEqual([]);
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
