import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import { libraryErrorText } from "./libraryErrors";

/** The inline Chinese defaults, interpolated — what zh users see. */
const t = ((_key: string, def: string, opts?: Record<string, string>) =>
  def.replace(/\{\{(\w+)\}\}/g, (_m, k: string) => opts?.[k] ?? "")) as unknown as TFunction;

describe("libraryErrorText", () => {
  it("localizes a known core message", () => {
    expect(libraryErrorText(t, "'发票' still holds 2 item(s) — move them out first")).toBe(
      "「发票」里还有 2 份资料，先挪走再删。",
    );
  });

  it("keeps an unknown message under the generic line", () => {
    expect(libraryErrorText(t, new Error("sqlite open: disk I/O error"))).toBe(
      "操作没成功，请稍后再试。\nsqlite open: disk I/O error",
    );
  });
});
