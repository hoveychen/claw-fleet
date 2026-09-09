import { describe, expect, it } from "vitest";

import { shortBuildCommit } from "./buildCommit";

/**
 * 「关于」里那行构建 commit 的取值规则。值得钉住的是**空值那一侧**：vite 的
 * define 在拿不到 git 时给的是字符串 "unknown"，直接渲染就会在版本号下面印出
 * 一行「构建 unknown」，读起来像是构建坏了，而实际上只是这份 bundle 不是从
 * git 里构建的。空串让整行不渲染。
 */
describe("shortBuildCommit", () => {
  it("截前 7 位", () => {
    expect(shortBuildCommit("b4372394f0e1c2d3")).toBe("b437239");
  });

  it("已经不足 7 位的原样返回", () => {
    expect(shortBuildCommit("abc12")).toBe("abc12");
  });

  it("\"unknown\" 不是 commit，返回空串", () => {
    expect(shortBuildCommit("unknown")).toBe("");
  });

  it("空串与 undefined 都返回空串", () => {
    expect(shortBuildCommit("")).toBe("");
    expect(shortBuildCommit(undefined)).toBe("");
  });
});
