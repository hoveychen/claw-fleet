import { describe, expect, it } from "vitest";

import { RELAY_URL_CN, RELAY_URL_GLOBAL, relayChoiceOf } from "./relayPresets";

describe("relayChoiceOf", () => {
  it("maps each preset host to its dropdown entry", () => {
    expect(relayChoiceOf(RELAY_URL_GLOBAL)).toBe("global");
    expect(relayChoiceOf(RELAY_URL_CN)).toBe("cn");
  });

  it("ignores a trailing slash and surrounding whitespace", () => {
    // A stored config that carries either still has to land on its preset,
    // otherwise the panel shows "custom" for a host picked from the dropdown.
    expect(relayChoiceOf(`${RELAY_URL_GLOBAL}/`)).toBe("global");
    expect(relayChoiceOf(` ${RELAY_URL_CN}//`)).toBe("cn");
  });

  it("calls anything else custom", () => {
    expect(relayChoiceOf("https://relay.example.com")).toBe("custom");
    expect(relayChoiceOf("")).toBe("custom");
    // Same host over plain http is a different URL, not the preset.
    expect(relayChoiceOf("http://fleet-relay.muveeai.com")).toBe("custom");
  });
});
