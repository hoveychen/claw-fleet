import { describe, expect, it } from "vitest";
import { parseDecisionDeepLink } from "./decisionDeepLink";

describe("parseDecisionDeepLink", () => {
  // Three delivery paths have different shapes: address bar is a full URL, SW
  // relay sends the original `/#d=...` from notify, native shell also sends the
  // latter. Must handle all.
  it("full URL", () => {
    expect(parseDecisionDeepLink("https://fleet.local/index.html#d=guard:g1")).toEqual({
      kind: "guard",
      id: "g1",
    });
  });

  it("path form as received from notify", () => {
    expect(parseDecisionDeepLink("/#d=elicitation:e-1")).toEqual({
      kind: "elicitation",
      id: "e-1",
    });
  });

  // ID is externally provided, may contain colons; kind never does. So split by
  // the first colon only; everything after goes to id.
  it("split by first colon only when id contains colons", () => {
    expect(parseDecisionDeepLink("/#d=guard:a:b:c")).toEqual({ kind: "guard", id: "a:b:c" });
  });

  it("no fragment → null", () => {
    expect(parseDecisionDeepLink("/")).toBeNull();
    expect(parseDecisionDeepLink("https://fleet.local/")).toBeNull();
  });

  // Pairing key uses `#k=` (see secretStore), must never be mistaken for a
  // decision target.
  it("other fragments not confused", () => {
    expect(parseDecisionDeepLink("/#k=SECRET")).toBeNull();
  });

  // When a request has no id, desktop degenerates the tag to bare kind; such a
  // link has no focusable target.
  it("kind without id → null", () => {
    expect(parseDecisionDeepLink("/#d=guard")).toBeNull();
    expect(parseDecisionDeepLink("/#d=guard:")).toBeNull();
    expect(parseDecisionDeepLink("/#d=")).toBeNull();
    expect(parseDecisionDeepLink("/#d=:g1")).toBeNull();
  });
});

// Relay stamps the source channel on click targets when fanning out notify
// (fleet-relay's notify_target.rs). Without it, when two machines have cards at
// once, which one opens is luck.
describe("channel mark", () => {
  it("reads the source channel relay stamped on", () => {
    const target = parseDecisionDeepLink("/#d=guard:g1&ch=105e300f");
    expect(target).toEqual({ kind: "guard", id: "g1", channelMark: "105e300f" });
  });

  // Key regression: old parsing was "everything after d= prefix is id", which
  // would swallow &ch=… into the id, so **every** stamped notification couldn't
  // open (id wouldn't match any card).
  it("does not swallow the mark into the card id", () => {
    expect(parseDecisionDeepLink("/#d=guard:g1&ch=105e300f")?.id).toBe("g1");
  });

  it("still parses a link from a relay that stamps nothing", () => {
    expect(parseDecisionDeepLink("/#d=guard:g1")).toEqual({ kind: "guard", id: "g1" });
  });

  it("tolerates the mark coming first", () => {
    expect(parseDecisionDeepLink("/#ch=105e300f&d=fleet-ask:abc")).toEqual({
      kind: "fleet-ask",
      id: "abc",
      channelMark: "105e300f",
    });
  });

  it("ignores a link that only carries a mark", () => {
    expect(parseDecisionDeepLink("/#ch=105e300f")).toBeNull();
  });
});
