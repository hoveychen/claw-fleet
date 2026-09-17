import { describe, expect, it } from "vitest";
import { scopedKey } from "./deviceScope";
import { loadDraft, saveDraft } from "./draft";

// React side hooks (useDeviceDraft / useDeviceScope) have no test environment to run in
// (this package has no jsdom or testing-library), so what's tested here is the pure function
// behind them that decides everything: how keys are partitioned. Hooks are just one-line
// wrappers around `useDraft(scopedKey(...))`.


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

  // Unpaired / same-origin / mock all have only one data source. Adding a prefix there
  // is not only useless, it makes old users' existing drafts vanish (key changed, can't read).
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
