import { beforeEach, describe, expect, it } from "vitest";
import { useComposerDraftStore } from "./composerDraft";
import type { ChatComposerAttachment } from "./components/ChatComposer";

/**
 * Regression for the reported bug: picking several files in the attach dialog
 * (or pasting / dragging several at once) only ever added the *last* one.
 *
 * Root cause: the composer's `addAttachmentEntry` read `attachments` from the
 * render-time closure and wrote `patch({ attachments: [...attachments, entry] })`.
 * In a synchronous loop that snapshot never advances, so each write overwrites
 * the previous one and only the final file survives. The fix is the updater
 * form of `patch`/`patchDraft` — `(prev) => ...` reads the *latest* draft inside
 * the store setter, so looped appends accumulate.
 *
 * There is no jsdom here (see store.test.ts); this exercises the store contract
 * the fix relies on, mirroring the component's loop exactly.
 */
describe("composerDraft attachment accumulation", () => {
  const attach = (path: string): ChatComposerAttachment => ({ path, name: path });

  beforeEach(() => {
    useComposerDraftStore.getState().clearDraft("new");
  });

  it("keeps every attachment added in one synchronous loop (multi-file pick / paste / drop)", () => {
    const paths = ["/a.png", "/b.png", "/c.png"];
    // Mirror the fixed component: each add uses the updater form so it appends
    // to the latest draft rather than a stale snapshot.
    for (const p of paths) {
      useComposerDraftStore.getState().patchDraft("new", (d) => ({
        attachments: [...d.attachments, attach(p)],
      }));
    }
    const got = useComposerDraftStore.getState().drafts["new"]?.attachments ?? [];
    expect(got.map((a) => a.path)).toEqual(paths);
  });

  it("supports a dedup-aware appender that ignores duplicate paths", () => {
    const add = (entry: ChatComposerAttachment) =>
      useComposerDraftStore.getState().patchDraft("new", (d) =>
        d.attachments.some((a) => a.path === entry.path)
          ? {}
          : { attachments: [...d.attachments, entry] },
      );
    add(attach("/x.png"));
    add(attach("/y.png"));
    add(attach("/x.png")); // duplicate — must not double-add
    const got = useComposerDraftStore.getState().drafts["new"]?.attachments ?? [];
    expect(got.map((a) => a.path)).toEqual(["/x.png", "/y.png"]);
  });

  it("still accepts a plain-object patch for scalar fields", () => {
    useComposerDraftStore.getState().patchDraft("new", { prompt: "hello" });
    expect(useComposerDraftStore.getState().drafts["new"]?.prompt).toBe("hello");
  });
});
