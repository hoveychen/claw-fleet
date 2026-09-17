import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * Every Chinese original used in `t("…")` must have an English entry in the `DICT` of `i18n.ts`.
 *
 * This gate exists because the failure mode of a missing entry is **silent**: if `t()` cannot
 * find the key, it returns it as-is, so the English UI displays Chinese directly, while the
 * build, type check, and existing tests all pass. On 2026-09-08, the artifacts page archive
 * went live this way — exactly 11 UI strings (breadcrumbs, search box, four errors) showed
 * Chinese in English mode with nothing stopping it. The desktop has `app/localeKeys.test.ts`
 * catching the same issue; mobile never had one.
 *
 * Scans **source code** rather than the dictionary, so it stays true as code changes: add
 * a `t()` call and forget to add its entry, and this test immediately fails with the missing
 * key and the file it is in.
 */

const SRC_DIR = resolve(__dirname);
const I18N_FILE = join(SRC_DIR, "i18n.ts");

/**
 * The `(?<![A-Za-z0-9_$.])` negative lookbehind is necessary, not pedantic: without it,
 * parentheses after identifiers ending in 't' like `format(".2f")` and `element.at("x")`
 * also match, treating their arguments as translation keys and reporting false positives.
 * The desktop's corresponding gate test hit the same issue (first version reported 42 false positives).
 */
const T_CALL = /(?<![A-Za-z0-9_$.])t\(\s*"((?:[^"\\]|\\.)*)"/g;

/**
 * Strings that look the same in both languages and therefore do not need dictionary entries.
 *
 * Each entry must be something that is **inherently English or a proper noun** — adding
 * `"Fleet": "Fleet"` to the dictionary is just noise. Chinese copy is never allowed in
 * this set: that is exactly what this gate is meant to catch.
 */
const SAME_IN_BOTH_LANGS = new Set([
  "Fleet", // product name
  "Codex app-server", // process name, displayed as-is when grouped by source on usage page
  "shell", // default command name in terminal view, used as-is in both languages
]);

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules") continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      out.push(...sourceFiles(full));
      continue;
    }
    if (!/\.tsx?$/.test(name)) continue;
    if (/\.test\.tsx?$/.test(name)) continue;
    out.push(full);
  }
  return out;
}

/** Keys registered in DICT. The literal form includes both quoted strings like `"中文":` and
 *  bare identifiers like `改名: "Rename"` — we must capture both, or bare keys get reported
 *  as missing. */
function dictKeys(): Set<string> {
  const src = readFileSync(I18N_FILE, "utf8");
  const start = src.indexOf("const DICT");
  if (start < 0) throw new Error("i18n.ts 里找不到 DICT —— 是不是改名了?");
  const body = src.slice(start);
  const out = new Set<string>();
  for (const m of body.matchAll(/^\s*"((?:[^"\\]|\\.)*)":/gm)) out.add(m[1]);
  for (const m of body.matchAll(/^\s*([^\s":,{}]+):\s*"/gm)) out.add(m[1]);
  return out;
}

describe("i18n key coverage", () => {
  /** key → file using it (relative to src/), deduplicated by key but keeping the first occurrence for locating it. */
  const used = new Map<string, string>();
  for (const file of sourceFiles(SRC_DIR)) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(T_CALL)) {
      if (!used.has(m[1])) used.set(m[1], relative(SRC_DIR, file));
    }
  }

  it("captures a substantial number of t() calls (regex is not completely broken)", () => {
    // This test is not just a sanity check: if the lookbehind above or the file traversal
    // breaks, `used` silently becomes an empty set, and the assertion below would pass.
    expect(used.size).toBeGreaterThan(300);
  });

  it("every t() key exists in DICT", () => {
    const keys = dictKeys();
    const missing = [...used.entries()]
      .filter(([k]) => !keys.has(k) && !SAME_IN_BOTH_LANGS.has(k))
      .map(([k, f]) => `${JSON.stringify(k)}  (${f})`)
      .sort();
    expect(
      missing,
      "DICT 缺这些条目 —— 英文 UI 上它们会原样显示中文",
    ).toEqual([]);
  });

  it("same-form-word allowlist must not contain Chinese", () => {
    // This list is an escape hatch, not a dumping ground: putting Chinese in it disables
    // this gate for that text.
    const cjk = [...SAME_IN_BOTH_LANGS].filter((s) => /[一-鿿]/.test(s));
    expect(cjk, "中文文案要进 DICT，不是进允许清单").toEqual([]);
  });
});
