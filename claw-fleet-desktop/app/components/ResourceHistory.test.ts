import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import type { ResourceEvent, ResourceEventKind } from "../generated/types";
import zh from "../locales/zh.json";
import en from "../locales/en.json";
import { eventKey, kindLabel, mergeHistory } from "./ResourceHistory";

describe("kindLabel", () => {
  const kinds: ResourceEventKind[] = [
    "enqueued", "granted", "released", "kicked", "cancelled", "timed_out",
    "aborted", "reaped", "transferred", "capacity_set", "resource_removed",
  ];
  it("labels every event kind, with a key in both locales", () => {
    const keys: string[] = [];
    const t = ((key: string, def?: string) => {
      keys.push(key);
      return def ?? key;
    }) as unknown as TFunction;
    for (const k of kinds) expect(kindLabel(k, t)).toBeTruthy();
    for (const key of keys) {
      const leaf = key.replace(/^resources\./, "");
      expect((zh as { resources: Record<string, string> }).resources[leaf], `zh ${key}`).toBeTruthy();
      expect((en as { resources: Record<string, string> }).resources[leaf], `en ${key}`).toBeTruthy();
    }
  });
});

const e = (tsMs: number, kind: ResourceEvent["kind"] = "granted", requestId = `r${tsMs}`): ResourceEvent =>
  ({ tsMs, kind, requestId }) as ResourceEvent;

describe("mergeHistory", () => {
  it("puts the fresh newest page on top and keeps older loaded pages below it", () => {
    const shown = [e(5), e(4), e(3), e(2), e(1)];
    const head = [e(7), e(6), e(5), e(4)];
    expect(mergeHistory(head, shown).map((x) => x.tsMs)).toEqual([7, 6, 5, 4, 3, 2, 1]);
  });
  it("never shows one event twice, even when several share a millisecond", () => {
    const shown = [e(5, "released", "a"), e(5, "granted", "b")];
    const head = [e(5, "released", "a"), e(5, "granted", "b"), e(4)];
    const merged = mergeHistory(head, shown);
    expect(new Set(merged.map(eventKey)).size).toBe(merged.length);
    expect(merged).toHaveLength(3);
  });
  it("is just the page on first load", () => {
    expect(mergeHistory([e(2), e(1)], [])).toHaveLength(2);
  });
});
