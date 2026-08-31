import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * `@tauri-apps/plugin-opener`'s `openPath` is the one opener call that Tauri's
 * ACL can veto, and the veto is invisible: the command rejects, and a call site
 * that does not await the promise shows nothing at all. That is exactly what
 * shipped — 「用系统应用打开」on the 产出 detail page did nothing on macOS,
 * because `capabilities/*.json` only grants `opener:default`, whose permission
 * set (tauri-plugin-opener 2.5.3, `permissions/default.toml`) is
 * `allow-open-url` + `allow-reveal-item-in-dir` + `allow-default-urls` — no
 * `allow-open-path`.
 *
 * `reveal_item_in_dir` has no scope check at all in the plugin, which is why
 * 「在访达中显示」kept working next to a dead button.
 *
 * Granting `opener:allow-open-path` alone is still not enough: `scope.rs`
 * answers `fs_scope.is_allowed(path) && allowed.iter().any(...)`, and with an
 * empty scope the `any()` is false, so the command keeps returning
 * `ForbiddenPath`. Hence the second half of this assertion.
 *
 * Prefer a backend command (`open_artifact_external`) that resolves the path
 * host-side and calls the Rust `app.opener().open_path()`, which is not
 * scope-checked — the frontend then never hands an arbitrary path across.
 */
const APP_DIR = join(__dirname);
const CAPABILITIES_DIR = join(__dirname, "..", "capabilities");

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name.startsWith(".")) continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...sourceFiles(full));
    else if (/\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name)) out.push(full);
  }
  return out;
}

/** Files that import `openPath` from the opener plugin and call it. */
function pluginOpenPathCallers(): string[] {
  return sourceFiles(APP_DIR).filter((f) => {
    const src = readFileSync(f, "utf8");
    return (
      /from\s+"@tauri-apps\/plugin-opener"/.test(src) &&
      /\bopenPath\s*\(/.test(src)
    );
  });
}

/** Permission entries across every capability file, flattened. */
function permissionEntries(): (string | { identifier?: string; allow?: unknown[] })[] {
  return readdirSync(CAPABILITIES_DIR)
    .filter((n) => n.endsWith(".json"))
    .flatMap((n) => {
      const cap = JSON.parse(readFileSync(join(CAPABILITIES_DIR, n), "utf8"));
      return (cap.permissions ?? []) as (string | { identifier?: string })[];
    });
}

describe("opener capability", () => {
  it("only uses the plugin's openPath when the ACL actually grants a scoped allow-open-path", () => {
    const callers = pluginOpenPathCallers();
    if (callers.length === 0) return;

    const scoped = permissionEntries().some(
      (p) =>
        typeof p === "object" &&
        p !== null &&
        p.identifier === "opener:allow-open-path" &&
        Array.isArray(p.allow) &&
        p.allow.length > 0,
    );

    expect(
      scoped,
      `${callers.join(", ")} call the opener plugin's openPath, but no capability grants ` +
        `opener:allow-open-path with a non-empty allow scope — the command will be rejected ` +
        `and the rejection will be invisible in the UI.`,
    ).toBe(true);
  });
});
