import { beforeEach, describe, expect, it } from "vitest";
import { clearDraft, loadDraft, saveDraft, type DraftStorage } from "./draft";

// node 环境没有 window.localStorage，用内存实现注入。顺带断言 key 带上了命名空间前缀。
function memStore(): DraftStorage & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k) => (map.has(k) ? map.get(k)! : null),
    setItem: (k, v) => void map.set(k, v),
    removeItem: (k) => void map.delete(k),
  };
}

describe("draft store", () => {
  let store: ReturnType<typeof memStore>;
  beforeEach(() => {
    store = memStore();
  });

  it("对象草稿 round-trip：save 后 load 拿回同样内容", () => {
    const draft = { prompt: "写点东西", model: "opus" };
    saveDraft("new-session", draft, store);
    expect(loadDraft("new-session", { prompt: "", model: "" }, store)).toEqual(draft);
  });

  it("key 带 fleet-draft: 前缀，不污染其它 localStorage 键", () => {
    saveDraft("new-session", { prompt: "x" }, store);
    expect([...store.map.keys()]).toEqual(["fleet-draft:new-session"]);
  });

  it("字符串草稿 round-trip（继续会话的 prompt）", () => {
    saveDraft("resume:abc", "继续做那个", store);
    expect(loadDraft("resume:abc", "", store)).toBe("继续做那个");
  });

  it("没存过时返回 fallback", () => {
    expect(loadDraft("missing", { prompt: "默认" }, store)).toEqual({ prompt: "默认" });
  });

  it("schema 漂移：旧草稿缺字段时，缺的字段取 fallback 默认值（浅合并）", () => {
    saveDraft("new-session", { prompt: "老草稿" }, store); // 当时还没有 effort 字段
    const loaded = loadDraft("new-session", { prompt: "", effort: "medium" }, store);
    expect(loaded).toEqual({ prompt: "老草稿", effort: "medium" });
  });

  it("存进去的字段能覆盖 fallback 默认值", () => {
    saveDraft("new-session", { prompt: "x", effort: "high" }, store);
    const loaded = loadDraft("new-session", { prompt: "", effort: "medium" }, store);
    expect(loaded.effort).toBe("high");
  });

  it("损坏的 JSON 不抛异常，退回 fallback", () => {
    store.map.set("fleet-draft:new-session", "{不是合法json");
    expect(loadDraft("new-session", { prompt: "兜底" }, store)).toEqual({ prompt: "兜底" });
  });

  it("clear 后 load 回到 fallback", () => {
    saveDraft("new-session", { prompt: "待清除" }, store);
    clearDraft("new-session", store);
    expect(loadDraft("new-session", { prompt: "空" }, store)).toEqual({ prompt: "空" });
    expect(store.map.size).toBe(0);
  });

  it("storage 为 null（私隐模式）时静默降级，不抛异常", () => {
    expect(() => saveDraft("k", { a: 1 }, null)).not.toThrow();
    expect(loadDraft("k", { a: 0 }, null)).toEqual({ a: 0 });
    expect(() => clearDraft("k", null)).not.toThrow();
  });
});
