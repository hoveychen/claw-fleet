import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * Every Tauri plugin API a window can reach must be granted by that window's
 * capability — checked per window, against the real ACL manifest.
 *
 * This exists because the failure mode is silent. 「用系统应用打开」on the 产出
 * page did nothing on macOS for as long as it shipped: `openPath` needs
 * `opener:allow-open-path`, `opener:default` does not include it (it is
 * `allow-open-url` + `allow-reveal-item-in-dir` + `allow-default-urls`), so the
 * ACL rejected the command and the unawaited promise swallowed the rejection.
 * The same bug had already happened once before, in the decision-float window
 * missing `opener:default` — see the comment on `openExternal` in
 * `markdown/safeLinks.tsx`. Nothing but a manual click-through of every view in
 * every window would have caught either one.
 *
 * Everything here is derived rather than hand-listed, so it stays true as
 * plugins and capabilities change:
 *
 *  - window → entry:   `vite.config.ts`'s rollup input, then the `<script>` in
 *                      each html. The input keys are the window labels.
 *  - entry → files:    the import graph, static and dynamic.
 *  - API → command:    each plugin's shipped `dist-js/index.js`, which spells
 *                      `invoke('plugin:opener|open_path')` inside the exported
 *                      function.
 *  - permission → command: `gen/schemas/acl-manifests.json`, which tauri-build
 *                      generates from the actual crates — including how each
 *                      `default` set expands.
 *
 * Deliberately *not* covered: `@tauri-apps/api/window` and friends. Their APIs
 * are class methods rather than the flat exported functions the dist-js scan
 * reads, and `core:default` already grants the bulk of them. Adding a new
 * `core:window:*` call still needs a manual capability check.
 */

const ROOT = resolve(__dirname, "..");
const APP = __dirname;

// ── window → entry file ──────────────────────────────────────────────────────

function windowEntries(): Map<string, string> {
  const vite = readFileSync(join(ROOT, "vite.config.ts"), "utf8");
  const input = /input:\s*\{([\s\S]*?)\}/.exec(vite);
  if (!input) throw new Error("vite.config.ts: rollupOptions.input not found");

  const out = new Map<string, string>();
  for (const m of input[1].matchAll(/["']?([\w-]+)["']?:\s*resolve\(__dirname,\s*"([^"]+)"\)/g)) {
    const [, label, htmlName] = m;
    const html = readFileSync(join(ROOT, htmlName), "utf8");
    const script = /<script[^>]+src="([^"]+)"/.exec(html);
    if (!script) throw new Error(`${htmlName}: no module script`);
    // `/app/main.tsx` is served from the crate root.
    out.set(label, join(ROOT, script[1].replace(/^\//, "")));
  }
  return out;
}

// ── entry → reachable files ──────────────────────────────────────────────────

const EXTS = [".ts", ".tsx", ".js", ".jsx"];

function resolveImport(fromFile: string, spec: string): string | null {
  if (!spec.startsWith(".")) return null;
  const base = resolve(dirname(fromFile), spec);
  for (const cand of [base, ...EXTS.map((e) => base + e), ...EXTS.map((e) => join(base, "index" + e))]) {
    if (existsSync(cand) && statSync(cand).isFile()) return cand;
  }
  return null;
}

/**
 * Every `from "…"` and `import("…")` specifier in a source file, minus
 * `import type` / `export type`, which TypeScript erases — the module never
 * reaches the bundle, so following those edges would over-report. `localImages`
 * pulling a type out of `ExplorerPane` is exactly that: no `CopyButton` in the
 * preview window's bundle, despite the path through the source graph.
 */
function specifiers(src: string): string[] {
  const values = src.replace(/\b(?:import|export)\s+type\s+[^;]*?from\s*["'][^"']+["']/g, "");
  return [...values.matchAll(/(?:from|import)\s*\(?\s*["']([^"']+)["']/g)].map((m) => m[1]);
}

function reachableFrom(entry: string): Set<string> {
  const seen = new Set<string>();
  const queue = [entry];
  while (queue.length) {
    const file = queue.pop()!;
    if (seen.has(file)) continue;
    seen.add(file);
    const src = readFileSync(file, "utf8");
    for (const spec of specifiers(src)) {
      const next = resolveImport(file, spec);
      if (next && !seen.has(next)) queue.push(next);
    }
  }
  return seen;
}

// ── reachable files → plugin APIs used ───────────────────────────────────────

/**
 * `{ save }`, `{ open as openDialog }`, and the `await import(...)` destructure
 * all name the *plugin's* export, which is what the dist-js scan keys on.
 */
function pluginApisIn(src: string): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  const add = (plugin: string, names: string) => {
    const set = out.get(plugin) ?? new Set<string>();
    for (const part of names.split(",")) {
      const name = part.trim().split(/\s+as\s+/)[0].trim();
      if (name && !name.startsWith("type ")) set.add(name);
    }
    out.set(plugin, set);
  };

  for (const m of src.matchAll(/import\s*\{([^}]*)\}\s*from\s*["']@tauri-apps\/plugin-([\w-]+)["']/g)) {
    add(m[2], m[1]);
  }
  for (const m of src.matchAll(/\{([^}]*)\}\s*=\s*await\s+import\(\s*["']@tauri-apps\/plugin-([\w-]+)["']\s*\)/g)) {
    add(m[2], m[1]);
  }
  // `import("…").then(({ revealItemInDir }) => …)`
  for (const m of src.matchAll(
    /import\(\s*["']@tauri-apps\/plugin-([\w-]+)["']\s*\)\s*\.then\(\s*\(?\s*\{([^}]*)\}/g,
  )) {
    add(m[1], m[2]);
  }
  return out;
}

// ── plugin API → the command it invokes ──────────────────────────────────────

/** Scan a plugin's shipped bundle for `async function api() { invoke('plugin:p|cmd') }`. */
function apiCommands(plugin: string): Map<string, Set<string>> {
  const dist = join(ROOT, "node_modules", "@tauri-apps", `plugin-${plugin}`, "dist-js", "index.js");
  const src = readFileSync(dist, "utf8");
  const out = new Map<string, Set<string>>();
  let current: string | null = null;
  for (const line of src.split("\n")) {
    const fn = /^\s*(?:async\s+)?function\s+([A-Za-z0-9_$]+)\s*\(/.exec(line);
    if (fn) current = fn[1];
    const inv = /invoke\(\s*["']plugin:([\w-]+)\|([\w]+)["']/.exec(line);
    if (inv && current) {
      const set = out.get(current) ?? new Set<string>();
      set.add(`${inv[1]}:${inv[2]}`);
      out.set(current, set);
    }
  }
  return out;
}

// ── capability → allowed commands ────────────────────────────────────────────

interface Manifest {
  default_permission?: { permissions: string[] };
  permissions: Record<string, { commands: { allow: string[] } }>;
  permission_sets?: Record<string, { permissions: string[] }>;
}

const MANIFESTS: Record<string, Manifest> = JSON.parse(
  readFileSync(join(ROOT, "gen", "schemas", "acl-manifests.json"), "utf8"),
);

/** Expand one `plugin:identifier` entry into the commands it allows. */
function commandsFor(plugin: string, id: string, seen = new Set<string>()): Set<string> {
  const out = new Set<string>();
  const key = `${plugin}:${id}`;
  if (seen.has(key)) return out;
  seen.add(key);

  const manifest = MANIFESTS[plugin];
  if (!manifest) return out;

  const direct = manifest.permissions?.[id];
  if (direct) {
    for (const c of direct.commands.allow) out.add(`${plugin}:${c}`);
    return out;
  }
  const set =
    id === "default" ? manifest.default_permission : manifest.permission_sets?.[id];
  for (const child of set?.permissions ?? []) {
    for (const c of commandsFor(plugin, child, seen)) out.add(c);
  }
  return out;
}

interface Capability {
  windows?: string[];
  permissions: (string | { identifier: string; allow?: unknown[] })[];
}

function capabilities(): Capability[] {
  const dir = join(ROOT, "capabilities");
  return readdirSync(dir)
    .filter((n) => n.endsWith(".json"))
    .map((n) => JSON.parse(readFileSync(join(dir, n), "utf8")) as Capability);
}

/** Commands a window may invoke, plus which of them carry a non-empty scope. */
function grantsForWindow(label: string): { allowed: Set<string>; scoped: Set<string> } {
  const allowed = new Set<string>();
  const scoped = new Set<string>();
  for (const cap of capabilities()) {
    if (!cap.windows?.includes(label)) continue;
    for (const entry of cap.permissions) {
      const id = typeof entry === "string" ? entry : entry.identifier;
      const [plugin, rest] = id.includes(":")
        ? [id.slice(0, id.lastIndexOf(":")), id.slice(id.lastIndexOf(":") + 1)]
        : ["core", id];
      const cmds = commandsFor(plugin, rest);
      for (const c of cmds) {
        allowed.add(c);
        if (typeof entry === "object" && Array.isArray(entry.allow) && entry.allow.length > 0) {
          scoped.add(c);
        }
      }
    }
  }
  return { allowed, scoped };
}

// ── the assertions ───────────────────────────────────────────────────────────

/**
 * `open_path` is the one command whose allow-permission is not enough on its
 * own: `scope.rs` answers `fs_scope.is_allowed(path) && allowed.any(..)`, so an
 * empty scope forbids every path. Listed here rather than inferred because the
 * plugin does not describe it in the manifest.
 */
const NEEDS_SCOPE = new Set(["opener:open_path"]);

interface Use {
  window: string;
  file: string;
  plugin: string;
  api: string;
  command: string;
}

function uses(): Use[] {
  const commandCache = new Map<string, Map<string, Set<string>>>();
  const out: Use[] = [];
  for (const [label, entry] of windowEntries()) {
    for (const file of reachableFrom(entry)) {
      if (/\.test\.tsx?$/.test(file)) continue;
      for (const [plugin, apis] of pluginApisIn(readFileSync(file, "utf8"))) {
        if (!commandCache.has(plugin)) commandCache.set(plugin, apiCommands(plugin));
        const table = commandCache.get(plugin)!;
        for (const api of apis) {
          for (const command of table.get(api) ?? []) {
            out.push({ window: label, file: file.slice(APP.length + 1), plugin, api, command });
          }
        }
      }
    }
  }
  return out;
}

describe("capability coverage", () => {
  it("finds the windows, their entries and the plugin APIs they reach", () => {
    // A silent parse failure anywhere above would make the real assertion pass
    // by checking nothing, which is the one way this guard could rot unnoticed.
    expect([...windowEntries().keys()].sort()).toEqual([
      "decision-float",
      "main",
      "preview",
      "settings",
    ]);
    expect(uses().length).toBeGreaterThan(10);
  });

  it("grants every plugin command each window can reach", () => {
    const grants = new Map<string, ReturnType<typeof grantsForWindow>>();
    const missing = uses().filter((u) => {
      if (!grants.has(u.window)) grants.set(u.window, grantsForWindow(u.window));
      const { allowed, scoped } = grants.get(u.window)!;
      if (!allowed.has(u.command)) return true;
      return NEEDS_SCOPE.has(u.command) && !scoped.has(u.command);
    });

    expect(
      missing.map((u) => `${u.window}: ${u.file} calls ${u.api}() → ${u.command} (not granted)`),
    ).toEqual([]);
  });
});
