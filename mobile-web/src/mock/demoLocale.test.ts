import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import * as data from "./data";
import { localizeMock } from "./demoLocale";

const CJK = /[㐀-鿿＀-￯　-〿]/;

function chineseStrings(value: unknown, out: string[] = [], seen = new Set<unknown>()): string[] {
  if (typeof value === "string") {
    if (CJK.test(value)) out.push(value);
  } else if (value && typeof value === "object" && !seen.has(value)) {
    seen.add(value);
    for (const v of Object.values(value)) chineseStrings(v, out, seen);
  }
  return out;
}

describe("demo fixtures in English", () => {
  // The guard for demoLocale's exact-match map: edit a fixture string and it
  // drops out of the map, which shows up here as a leftover Chinese string.
  it("leaves no Chinese in any fixture once localized", () => {
    expect(chineseStrings(localizeMock({ ...data }, "en"))).toEqual([]);
  });

  it("covers the canned side-question copy in mock/relay.ts", () => {
    const relaySrc = readFileSync(resolve(__dirname, "relay.ts"), "utf8");
    const block = relaySrc.slice(relaySrc.indexOf("const MOCK_EXPLAIN_QUESTION"), relaySrc.indexOf("const MOCK_SLOW_MS"));
    const literals = [...block.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => JSON.parse(`"${m[1]}"`) as string);
    const chinese = literals.filter((s) => CJK.test(s));
    expect(chinese.length).toBeGreaterThan(0);
    expect(chineseStrings(localizeMock(chinese, "en"))).toEqual([]);
  });

  it("is the identity outside English, so Chinese screenshots keep their copy", () => {
    const sessions = data.MOCK_SESSIONS;
    expect(localizeMock(sessions, "zh")).toBe(sessions);
  });
});
