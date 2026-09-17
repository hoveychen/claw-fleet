import { describe, expect, it } from "vitest";
import {
  defaultRelayBaseFrom,
  parseRelayParam,
  relayBaseFor,
  relayWsUrl,
} from "./relayBase";

// Test cases for `resolveRelayBase` and `relayDisplayHost` still live in relay.test.ts—
// moving them with the functions would be too noisy, changes are just import paths.
// This file covers the new ones that grew out of multi-device.

const BAKED = "https://fleet-relay.muveeai.com";

describe("parseRelayParam", () => {
  it("reads the relay a pairing QR named", () => {
    expect(parseRelayParam("#k=abc&relay=https%3A%2F%2Frelay.corp.example.com")).toBe(
      "https://relay.corp.example.com",
    );
  });

  it("drops the path — a relay behind a prefix is unsupported everywhere in this client", () => {
    expect(parseRelayParam("#relay=https%3A%2F%2Fr.example.com%2Fprefix%2F")).toBe(
      "https://r.example.com",
    );
  });

  it("keeps an explicit port (the dev relay case)", () => {
    expect(parseRelayParam("#relay=http%3A%2F%2F127.0.0.1%3A18080")).toBe(
      "http://127.0.0.1:18080",
    );
  });

  it("returns null when no relay is named", () => {
    expect(parseRelayParam("#k=abc")).toBeNull();
    expect(parseRelayParam("")).toBeNull();
  });

  // QR codes are untrusted input, and this value becomes the base for every URL
  // the client makes afterward.
  it("refuses anything that is not an absolute http(s) URL", () => {
    expect(parseRelayParam("#relay=javascript%3Aalert(1)")).toBeNull();
    expect(parseRelayParam("#relay=file%3A%2F%2F%2Fetc%2Fpasswd")).toBeNull();
    expect(parseRelayParam("#relay=%2Fjust%2Fa%2Fpath")).toBeNull();
    expect(parseRelayParam("#relay=not%20a%20url")).toBeNull();
  });
});

describe("defaultRelayBaseFrom", () => {
  it("prefers the baked build constant over the page origin", () => {
    // HarmonyOS shell pages have a fake origin https://fleet.local; falling back
    // to it would make the app dial itself.
    expect(defaultRelayBaseFrom(BAKED, "https://fleet.local")).toBe(BAKED);
  });

  it("falls back to the origin — the PWA case, where the relay serves the page", () => {
    expect(defaultRelayBaseFrom(undefined, BAKED)).toBe(BAKED);
    expect(defaultRelayBaseFrom("", BAKED)).toBe(BAKED);
  });
});

describe("relayBaseFor", () => {
  it("uses the relay the device named", () => {
    expect(relayBaseFor("https://relay.corp.example.com")).toBe(
      "https://relay.corp.example.com",
    );
  });

  // Devices migrated from earlier versions and devices scanned without &relay= are both null — that is not
  // "unknown", but rather "use the build default" (in this test environment, origin is http://localhost).
  it("falls back to the build default when the device named none", () => {
    expect(relayBaseFor(null)).toBe("http://localhost");
    expect(relayBaseFor(undefined)).toBe("http://localhost");
  });
});

describe("relayWsUrl", () => {
  it("maps http(s) to ws(s) and appends /ws", () => {
    expect(relayWsUrl("https://r.example.com")).toBe("wss://r.example.com/ws");
    expect(relayWsUrl("http://127.0.0.1:18080")).toBe("ws://127.0.0.1:18080/ws");
  });

  it("tolerates a trailing slash", () => {
    expect(relayWsUrl("https://r.example.com/")).toBe("wss://r.example.com/ws");
  });
});
