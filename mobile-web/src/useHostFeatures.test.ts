import { describe, expect, it } from "vitest";
import { normalizeHostFeatures } from "./useHostFeatures";

/** 手机端的终端入口只由这一个判据决定，而它的默认必须是「关」：一台老桌面端
 *  不认 host_features 这个方法（应答 null / 报错），或者答复里根本没有这个字段，
 *  都不能被读成「开着」——那会让用户点进终端页、直到开 shell 被后端拒绝才知道
 *  这台主机没开这个面。 */
describe("normalizeHostFeatures", () => {
  it("treats a proper affirmative as on", () => {
    expect(normalizeHostFeatures({ terminal: true })).toEqual({ terminal: true });
  });

  it.each([
    ["explicit false", { terminal: false }],
    ["missing field", {}],
    ["null answer", null],
    ["undefined answer", undefined],
    ["a truthy string, not a boolean", { terminal: "true" }],
    ["a truthy number, not a boolean", { terminal: 1 }],
    ["nonsense", "yes"],
  ])("fails closed on %s", (_name, raw) => {
    expect(normalizeHostFeatures(raw)).toEqual({ terminal: false });
  });
});
