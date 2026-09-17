import { describe, expect, it } from "vitest";
import { parsePairingLink } from "./pairingLink";

const SECRET = "b8c0de1f2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5";

describe("parsePairingLink", () => {
  it("self-hosted relay: origin is the address this device should connect to", () => {
    expect(parsePairingLink(`https://relay.corp.example.com/#k=${SECRET}`)).toEqual({
      secret: SECRET,
      relayBase: "https://relay.corp.example.com",
    });
  });

  it("LAN relay (http + port) also accepted", () => {
    expect(parsePairingLink(`http://192.168.1.9:18080/#k=${SECRET}`)).toEqual({
      secret: SECRET,
      relayBase: "http://192.168.1.9:18080",
    });
  });

  it("desktop's &lang= parameter doesn't affect parsing", () => {
    expect(parsePairingLink(`https://fleet-relay.muveeai.com/#k=${SECRET}&lang=zh`)).toEqual({
      secret: SECRET,
      relayBase: "https://fleet-relay.muveeai.com",
    });
  });

  it("explicit &relay= overrides origin (HarmonyOS shell format)", () => {
    expect(
      parsePairingLink(
        `https://fleet.local/index.html#k=${SECRET}&relay=${encodeURIComponent("https://r.example.com")}`,
      ),
    ).toEqual({ secret: SECRET, relayBase: "https://r.example.com" });
  });

  // Most likely user paste mishaps: leading/trailing whitespace, partial paste,
  // pasted something else entirely.
  it("leading/trailing whitespace stripped", () => {
    expect(parsePairingLink(`  https://r.example.com/#k=${SECRET}\n`)).toEqual({
      secret: SECRET,
      relayBase: "https://r.example.com",
    });
  });

  it("link without #k= is not a pairing link", () => {
    expect(parsePairingLink("https://fleet-relay.muveeai.com/")).toBeNull();
  });

  it("empty input is not a pairing link", () => {
    expect(parsePairingLink("   ")).toBeNull();
  });

  // Relay's auth frame requires >= 16; reject short keys immediately rather than
  // failing to connect after pairing.
  it("short key rejected", () => {
    expect(parsePairingLink("https://r.example.com/#k=deadbeef")).toBeNull();
  });

  // `?k=` deliberately not accepted: query strings go in relay access logs, but
  // relay must never see the secret key.
  it("k= in query string not accepted (secret only in fragment)", () => {
    expect(parsePairingLink(`https://r.example.com/?k=${SECRET}`)).toBeNull();
  });
});
