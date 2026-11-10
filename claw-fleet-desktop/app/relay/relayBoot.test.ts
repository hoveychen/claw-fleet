import { describe, expect, it } from "vitest";
import { preselectDevice, relayDevicesFromBook } from "./relayBoot";

describe("relayDevicesFromBook", () => {
  it("keeps relay pairings, skips http hosts and malformed records", () => {
    const raw = JSON.stringify({
      devices: [
        { kind: "relay", id: "a", label: "MacBook", secret: "s1", relayBase: null },
        { id: "legacy", label: "", secret: "s2" }, // pre-`kind` record
        { kind: "http", id: "h", baseUrl: "https://x", token: null },
        { kind: "relay", id: "nosecret" },
        "junk",
      ],
      activeId: "a",
    });
    expect(relayDevicesFromBook(raw)).toEqual([
      { id: "a", label: "MacBook", secret: "s1", relayBase: null },
      { id: "legacy", label: "legacy", secret: "s2", relayBase: null },
    ]);
  });

  it("treats missing or broken storage as no devices", () => {
    expect(relayDevicesFromBook(null)).toEqual([]);
    expect(relayDevicesFromBook("{")).toEqual([]);
    expect(relayDevicesFromBook('{"devices":3}')).toEqual([]);
  });
});

describe("preselectDevice", () => {
  const a = { id: "a", label: "A", secret: "1", relayBase: null };
  const b = { id: "b", label: "B", secret: "2", relayBase: null };

  it("uses the only device without asking", () => {
    expect(preselectDevice([a], null, true)).toBe(a);
  });

  it("uses the remembered choice among several, unless a pick is forced", () => {
    expect(preselectDevice([a, b], "b", false)).toBe(b);
    expect(preselectDevice([a, b], "b", true)).toBeNull();
    expect(preselectDevice([a, b], "gone", false)).toBeNull();
    expect(preselectDevice([a, b], null, false)).toBeNull();
  });
});
