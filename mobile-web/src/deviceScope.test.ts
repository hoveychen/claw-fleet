import { describe, expect, it } from "vitest";
import { scopedKey } from "./deviceScope";
import { loadDraft, saveDraft } from "./draft";

// React 侧的 hook（useDeviceDraft / useDeviceScope）没有测试环境可跑（这个包没装
// jsdom 与 testing-library），所以这里钉的是它们背后那个决定一切的纯函数：键怎么
// 分家。hook 只是 `useDraft(scopedKey(...))` 的一行包装。

const store = new Map<string, string>();
const mem = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
};

describe("scopedKey", () => {
  it("namespaces a key by device", () => {
    expect(scopedKey("d1", "new-session")).toBe("d/d1/new-session");
  });

  it("keeps two devices' identically-named drafts apart", () => {
    expect(scopedKey("d1", "resume:s-42")).not.toBe(scopedKey("d2", "resume:s-42"));
  });

  // 未配对 / 同源形态 / mock 都只有一个数据源。那里加前缀不但没用，还会让老用户
  // 已经存在的草稿凭空消失（键变了就读不到了）。
  it("leaves the key alone when there is no device", () => {
    expect(scopedKey(null, "new-session")).toBe("new-session");
  });
});

describe("scoped drafts through draft.ts", () => {
  it("two devices' drafts do not overwrite each other", () => {
    store.clear();
    saveDraft(scopedKey("d1", "new-session"), { prompt: "在 Mac 上写的" }, mem);
    saveDraft(scopedKey("d2", "new-session"), { prompt: "在 Linux 上写的" }, mem);
    expect(loadDraft(scopedKey("d1", "new-session"), { prompt: "" }, mem).prompt).toBe(
      "在 Mac 上写的",
    );
    expect(loadDraft(scopedKey("d2", "new-session"), { prompt: "" }, mem).prompt).toBe(
      "在 Linux 上写的",
    );
  });

  it("a device sees no draft where another device has one", () => {
    store.clear();
    saveDraft(scopedKey("d1", "tasks:workspace"), "/Users/me/repo-a", mem);
    expect(loadDraft(scopedKey("d2", "tasks:workspace"), "", mem)).toBe("");
  });
});
