import { describe, expect, it } from "vitest";
import { parseFindResult } from "./iframeBridge";

describe("parseFindResult", () => {
  it("parses a valid count", () => {
    expect(parseFindResult({ __fleetFindResult: { count: 3 } })).toEqual({ count: 3 });
  });

  it("floors fractional counts", () => {
    expect(parseFindResult({ __fleetFindResult: { count: 2.9 } })).toEqual({ count: 2 });
  });

  it("accepts zero", () => {
    expect(parseFindResult({ __fleetFindResult: { count: 0 } })).toEqual({ count: 0 });
  });

  it("rejects unrelated / hostile payloads", () => {
    expect(parseFindResult(null)).toBeNull();
    expect(parseFindResult("boom")).toBeNull();
    expect(parseFindResult({ __fleetAskHeight: 500 })).toBeNull();
    expect(parseFindResult({ __fleetFindResult: { count: -1 } })).toBeNull();
    expect(parseFindResult({ __fleetFindResult: { count: "3" } })).toBeNull();
    expect(parseFindResult({ __fleetFindResult: {} })).toBeNull();
    expect(parseFindResult({ __fleetFindResult: { count: Infinity } })).toBeNull();
  });
});
