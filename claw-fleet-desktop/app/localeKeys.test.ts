import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

import enJson from "./locales/en.json";
import zhJson from "./locales/zh.json";

/**
 * Every `t("key")` call must have its key present in both en.json and zh.json.
 *
 * This guard exists because missing a key fails silently. When i18next can't find
 * a key, it falls back to the second argument, which we always write in Chinese —
 * so the English UI shows Chinese text directly. The build, type checks, and
 * existing tests all pass. The Mobile ("移动端") panel's relay-address button
 * shipped with Chinese "Edit" ("编辑") for a long time; only when someone actually
 * switched the UI to English and navigated there was it visible. Keys with no
 * fallback at all are worse: both languages show the key itself.
 *
 * The reverse direction is gated the same way: when zh.json lacks a key, the
 * Chinese UI works by accident via inline fallback, but that text lives nowhere
 * in the locale file — translation, review, and reuse all bypass it. The next
 * person to delete the fallback won't know they deleted the only Chinese copy.
 *
 * The scan inspects **source code**, not locale files, so it stays true as code
 * changes: add a `t()` call and forget to add the key, and this test turns red
 * and names the key.
 */

const APP_DIR = resolve(__dirname);

/** Locale JSON is nested (`{schedule: {title: …}}`), but `t()` calls use dot-separated paths. */
function flatten(obj: unknown, prefix = ""): Set<string> {
  const out = new Set<string>();
  if (typeof obj !== "object" || obj === null) return out;
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "object" && v !== null) {
      for (const nested of flatten(v, key)) out.add(nested);
    } else {
      out.add(key);
    }
  }
  return out;
}

/**
 * Call sites of `t("key")` / `t("key", "fallback")`.
 *
 * The negative lookbehind `(?<![A-Za-z0-9_$.])` is necessary, not optional:
 * without it, the `t(` inside `createElement("td")` would also match, so HTML
 * tag names like `td`, `tr`, `iframe` would be reported as missing translation
 * keys (the first version did this, producing 42 false positives).
 */
const T_CALL = /(?<![A-Za-z0-9_$.])t\(\s*"([A-Za-z0-9_.]+)"/g;

/** Prefix whitelist for keys that may be generated dynamically. Currently empty, meaning no dynamic keys are permitted. */
const DYNAMIC_KEY_PREFIXES: string[] = [];

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === "locales" || name === "mock") continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      out.push(...sourceFiles(full));
      continue;
    }
    if (!/\.(ts|tsx)$/.test(name)) continue;
    if (/\.test\.tsx?$/.test(name)) continue;
    out.push(full);
  }
  return out;
}

describe("locale key coverage", () => {
  const en = flatten(enJson);
  const zh = flatten(zhJson);

  /** key → file that uses it (relative to app/), deduplicated by key but keeping the first occurrence for reference. */
  const used = new Map<string, string>();
  for (const file of sourceFiles(APP_DIR)) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(T_CALL)) {
      const key = m[1];
      if (DYNAMIC_KEY_PREFIXES.some((p) => key.startsWith(p))) continue;
      if (!used.has(key)) used.set(key, relative(APP_DIR, file));
    }
  }

  it("scans a reasonable number of t() calls (regex is not entirely broken)", () => {
    // This check is not filler: if the lookbehind or file traversal logic above breaks,
    // `used` would silently become empty, and the two assertions below would both "pass".
    expect(used.size).toBeGreaterThan(200);
  });

  it("every key is present in en.json", () => {
    const missing = [...used.entries()]
      .filter(([k]) => !en.has(k))
      .map(([k, f]) => `${k}  (${f})`)
      .sort();
    expect(missing, `en.json 缺这些键 —— 英文 UI 会显示中文 fallback 或 key 本身`).toEqual([]);
  });

  it("every key is present in zh.json", () => {
    const missing = [...used.entries()]
      .filter(([k]) => !zh.has(k))
      .map(([k, f]) => `${k}  (${f})`)
      .sort();
    expect(missing, `zh.json 缺这些键 —— 那份中文只活在内联 fallback 里`).toEqual([]);
  });
});
