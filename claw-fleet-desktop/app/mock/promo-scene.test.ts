import { describe, expect, it } from "vitest";
import { primePromoStorage, promoSceneFromSearch } from "./promo-scene";

describe("promo scene bootstrap", () => {
  it("accepts only deterministic promo scene names", () => {
    expect(promoSceneFromSearch("?mock&promo=base")).toBe("base");
    expect(promoSceneFromSearch("?mock&promo=guard")).toBe("guard");
    expect(promoSceneFromSearch("?promo=ask&mock")).toBe("ask");
    expect(promoSceneFromSearch("?mock&promo=relay")).toBeNull();
    expect(promoSceneFromSearch("?mock")).toBeNull();
  });

  it("pre-seeds onboarding state for an uncluttered recording boot", () => {
    const values = new Map<string, string>();
    const storage = {
      get length() {
        return values.size;
      },
      clear: () => values.clear(),
      getItem: (key: string) => values.get(key) ?? null,
      key: (index: number) => [...values.keys()][index] ?? null,
      removeItem: (key: string) => values.delete(key),
      setItem: (key: string, value: string) => values.set(key, value),
    } as Storage;

    primePromoStorage(storage);

    expect(storage.getItem("mock-store:onboarding-dismissed")).toBe("true");
    expect(
      JSON.parse(storage.getItem("mock-store:onboarding-seen-features") ?? "[]"),
    ).toContain("global_ask");
  });
});
