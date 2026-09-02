import { beforeEach, describe, expect, it } from "vitest";
import {
  clearDraft,
  clearDraftsByPrefix,
  loadDraft,
  saveDraft,
  type DraftStorage,
} from "./draft";

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

  it("数组草稿 round-trip（附件 chip 列表），不与 fallback 合并", () => {
    const atts = [
      { name: "a.png", path: "/store/a.png" },
      { name: "b.pdf", path: "/store/b.pdf" },
    ];
    saveDraft("new-session:attachments", atts, store);
    expect(loadDraft("new-session:attachments", [], store)).toEqual(atts);
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

// 移除一台设备时要扫掉它那个命名空间下的全部草稿（新会话表单、附件路径、上次
// 用的 repo、各会话的半截输入）。不清的话每移除一台就留下一堆永远不会再被读到
// 的键；清错了则会连累另一台设备的草稿。
describe("clearDraftsByPrefix", () => {
  /** 可枚举的内存 storage —— clearDraftsByPrefix 需要 length/key(i)。 */
  function enumerableStore() {
    const map = new Map<string, string>();
    return {
      map,
      get length() {
        return map.size;
      },
      key: (i: number) => [...map.keys()][i] ?? null,
      getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
    };
  }

  it("clears one device's drafts and leaves the other's alone", () => {
    const store = enumerableStore();
    saveDraft("d/d1/new-session", { prompt: "a" }, store);
    saveDraft("d/d1/resume:s-1", "半截", store);
    saveDraft("d/d2/new-session", { prompt: "b" }, store);
    saveDraft("tasks:search", "全局偏好", store);

    clearDraftsByPrefix("d/d1/", store);

    expect(loadDraft("d/d1/new-session", null, store)).toBeNull();
    expect(loadDraft("d/d1/resume:s-1", null, store)).toBeNull();
    expect(loadDraft<{ prompt: string } | null>("d/d2/new-session", null, store)?.prompt).toBe("b");
    expect(loadDraft("tasks:search", "", store)).toBe("全局偏好");
  });

  // 注入的内存实现通常没有 length/key —— 那种情况下什么都不做，而不是抛。
  it("is a no-op on a storage that cannot be enumerated", () => {
    const plain = memStore();
    saveDraft("d/d1/new-session", { prompt: "a" }, plain);
    expect(() => clearDraftsByPrefix("d/d1/", plain)).not.toThrow();
    expect(loadDraft<{ prompt: string } | null>("d/d1/new-session", null, plain)?.prompt).toBe("a");
  });
});
