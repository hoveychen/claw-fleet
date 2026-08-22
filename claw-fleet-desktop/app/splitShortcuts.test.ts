import { describe, expect, it } from "vitest";
import { resolveSplitKey, type ChordState, type KeyLike } from "./splitShortcuts";

/** A ⌘-modified stroke, the common case. */
const cmd = (key: string, extra: Partial<KeyLike> = {}): KeyLike => ({
  key,
  metaKey: true,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...extra,
});

/** A bare stroke — someone typing into a composer. */
const plain = (key: string): KeyLike => ({
  key,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
});

describe("resolveSplitKey", () => {
  it("splits right on ⌘\\", () => {
    expect(resolveSplitKey(cmd("\\"), null)).toEqual({
      action: { kind: "splitRight" },
      chord: null,
      handled: true,
    });
  });

  it("accepts Ctrl as the modifier for Windows/Linux", () => {
    expect(
      resolveSplitKey(cmd("\\", { metaKey: false, ctrlKey: true }), null).action,
    ).toEqual({ kind: "splitRight" });
  });

  it("arms the chord on ⌘K and swallows the stroke", () => {
    // Swallowed on purpose: an unclaimed ⌘K can trigger the webview's own
    // behaviour, and the user would see the prefix "do something".
    expect(resolveSplitKey(cmd("k"), null)).toEqual({
      action: null,
      chord: "k",
      handled: true,
    });
  });

  it("splits down on ⌘K then ⌘\\", () => {
    const first = resolveSplitKey(cmd("K"), null);
    expect(resolveSplitKey(cmd("\\"), first.chord)).toEqual({
      action: { kind: "splitDown" },
      chord: null,
      handled: true,
    });
  });

  it("ignores the modifier's own keydown so it cannot break a chord", () => {
    // Observed in the real webview: pressing ⌘\ emits TWO keydowns — one for
    // `Meta` itself (already carrying metaKey: true) and one for the letter. A
    // user who releases ⌘ between the two halves of ⌘K ⌘\ therefore sends a
    // bare `Meta` in the middle, and treating it as "some other modified
    // stroke" cancels the chord — the split-down silently became a split-right.
    for (const mk of ["Meta", "Control", "Shift", "Alt"]) {
      const r = resolveSplitKey(cmd(mk), "k");
      expect(r.chord).toBe("k");
      expect(r.handled).toBe(false);
      expect(r.action).toBe(null);
    }
  });

  it("completes ⌘K ⌘\\ across a released-and-repressed modifier", () => {
    let chord: ChordState = null;
    chord = resolveSplitKey(cmd("Meta"), chord).chord;
    chord = resolveSplitKey(cmd("k"), chord).chord;
    chord = resolveSplitKey(cmd("Meta"), chord).chord;
    expect(resolveSplitKey(cmd("\\"), chord).action).toEqual({ kind: "splitDown" });
  });

  it("cancels a half-typed chord on any other modified stroke", () => {
    expect(resolveSplitKey(cmd("f"), "k")).toEqual({
      action: null,
      chord: null,
      handled: false,
    });
  });

  it("does not let a chord swallow ⌘1 as a split-down", () => {
    // The chord's second stroke is *only* backslash; ⌘K ⌘1 must not split.
    expect(resolveSplitKey(cmd("1"), "k").action).toBe(null);
  });

  it("focuses groups on ⌘1..⌘9", () => {
    expect(resolveSplitKey(cmd("1"), null).action).toEqual({ kind: "focusGroup", pos: 1 });
    expect(resolveSplitKey(cmd("9"), null).action).toEqual({ kind: "focusGroup", pos: 9 });
  });

  it("ignores ⌘0, which names no group", () => {
    expect(resolveSplitKey(cmd("0"), null).handled).toBe(false);
  });

  it("ignores unmodified typing and leaves an armed chord alone", () => {
    // Holding ⌘ between the two strokes emits no bare keys, so bare typing
    // means the user is doing something else entirely — but it must not cancel,
    // or a stray keyup-driven event would break the chord.
    expect(resolveSplitKey(plain("\\"), null)).toEqual({
      action: null,
      chord: null,
      handled: false,
    });
    expect(resolveSplitKey(plain("a"), "k").chord).toBe("k");
  });

  it("ignores Alt and Shift variants so they stay free for other bindings", () => {
    expect(resolveSplitKey(cmd("\\", { altKey: true }), null).handled).toBe(false);
    expect(resolveSplitKey(cmd("\\", { shiftKey: true }), null).handled).toBe(false);
  });

  it("never claims a stroke it produced no action for, except the prefix", () => {
    const claimedWithoutAction: ChordState[] = [null, "k"];
    for (const chord of claimedWithoutAction) {
      const r = resolveSplitKey(cmd("x"), chord);
      expect(r.handled).toBe(false);
      expect(r.action).toBe(null);
    }
  });
});
