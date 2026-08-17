import { describe, expect, it } from "vitest";
import { extractSecretFromUrl } from "./secretStore";

const SECRET = "Ab3-_xYz09";
const RELAY = "https://fleet-relay.muveeai.com";

describe("extractSecretFromUrl", () => {
  it("reads the secret from a bare fragment (the PWA's window.location.hash)", () => {
    expect(extractSecretFromUrl(`#k=${SECRET}`)).toBe(SECRET);
  });

  it("reads it from a full pairing URL, as a deep link delivers it", () => {
    expect(extractSecretFromUrl(`${RELAY}/#k=${SECRET}`)).toBe(SECRET);
  });

  it("finds it after other fragment params", () => {
    expect(extractSecretFromUrl(`${RELAY}/#t=1&k=${SECRET}`)).toBe(SECRET);
  });

  // The relay is a blind forwarder and must never see the secret. A query
  // string IS sent to the server, so accepting `?k=` would leak it every time
  // the pairing link opens in a browser rather than the app.
  it("refuses a secret in the query string", () => {
    expect(extractSecretFromUrl(`${RELAY}/?k=${SECRET}`)).toBeNull();
  });

  it("refuses a query secret even when a fragment follows without one", () => {
    expect(extractSecretFromUrl(`${RELAY}/?k=${SECRET}#t=1`)).toBeNull();
  });

  it("returns null when there is no secret", () => {
    expect(extractSecretFromUrl(`${RELAY}/`)).toBeNull();
    expect(extractSecretFromUrl(`${RELAY}/#t=1`)).toBeNull();
    expect(extractSecretFromUrl("")).toBeNull();
  });

  it("stops at characters that cannot be part of a secret", () => {
    expect(extractSecretFromUrl(`#k=${SECRET}&next=/tasks`)).toBe(SECRET);
  });
});
