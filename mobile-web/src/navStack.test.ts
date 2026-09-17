import { describe, expect, it, vi } from "vitest";
import { NavStack, type HistoryLike, type RootBackResult } from "./navStack";

/** Node environment lacks window.history, so we inject a fake accounting implementation. Only records
 *  call sequences — NavStack correctness is entirely reflected in how many entries were pushed and popped. */
function fakeHistory() {
  const calls: string[] = [];
  const history: HistoryLike = {
    pushState: () => void calls.push("push"),
    go: (d) => void calls.push(`go(${d})`),
  };
  return { history, calls };
}

/** Uses a synchronous scheduler by default, so assertions don't have to wait for microtasks;
 *  the StrictMode test case separately switches to manual flush. */
function mk(onRootBack: () => RootBackResult = () => "hold") {
  const { history, calls } = fakeHistory();
  const stack = new NavStack(history, onRootBack, (fn) => fn());
  stack.start();
  calls.length = 0; // Discard the sentinel push so subsequent assertions only see layer changes
  return { stack, calls };
}

describe("NavStack", () => {
  it("start pushes sentinel, bottom return has something to consume", () => {
    const { history, calls } = fakeHistory();
    const stack = new NavStack(history, () => "hold", (fn) => fn());
    stack.start();
    expect(calls).toEqual(["push"]);
    // Repeated start doesn't push a second sentinel
    stack.start();
    expect(calls).toEqual(["push"]);
  });

  it("push adds one layer and history entry; popstate pops top and calls its close", () => {
    const { stack, calls } = mk();
    const close = vi.fn();
    stack.push(close);
    expect(calls).toEqual(["push"]);

    stack.handlePopState();
    expect(close).toHaveBeenCalledTimes(1);
    expect(stack.depth).toBe(0);
    // User pressed back, browser already exited — shouldn't call go() again
    expect(calls).toEqual(["push"]);
  });

  it("after popstate pops one layer, React unmount-triggered drop doesn't re-back-navigate", () => {
    const { stack, calls } = mk();
    let id = 0;
    id = stack.push(() => stack.drop(id)); // Simulate close → component unmount → drop
    calls.length = 0;

    stack.handlePopState();
    expect(stack.depth).toBe(0);
    expect(calls).toEqual([]); // Key: no extra go(-1) calls
  });

  it("UI actively closes (drop) with its history entry; popstate feedback doesn't close next layer", () => {
    const { stack, calls } = mk();
    const closeOuter = vi.fn();
    const closeInner = vi.fn();
    stack.push(closeOuter);
    const inner = stack.push(closeInner);
    calls.length = 0;

    stack.drop(inner); // Clicked the back button
    expect(calls).toEqual(["go(-1)"]);

    // Browser then echoes back a popstate — must be absorbed, otherwise outer layer is wrongly closed
    stack.handlePopState();
    expect(closeOuter).not.toHaveBeenCalled();
    expect(stack.depth).toBe(1);

    // User presses back again, then the outer layer's turn
    stack.handlePopState();
    expect(closeOuter).toHaveBeenCalledTimes(1);
  });

  it("popstate only pops top of stack when there are multiple layers", () => {
    const { stack } = mk();
    const a = vi.fn();
    const b = vi.fn();
    stack.push(a);
    stack.push(b);

    stack.handlePopState();
    expect(b).toHaveBeenCalledTimes(1);
    expect(a).not.toHaveBeenCalled();
    expect(stack.depth).toBe(1);
  });

  it("StrictMode's push→drop→push double-run in one microtask cancels out, no half-layer residue", () => {
    const { history, calls } = fakeHistory();
    const pending: Array<() => void> = [];
    const stack = new NavStack(history, () => "hold", (fn) => void pending.push(fn));
    stack.start();
    calls.length = 0;

    // React 18/19 StrictMode: effect runs → cleanup → effect runs again, all within commit phase
    const id1 = stack.push(vi.fn());
    stack.drop(id1);
    stack.push(vi.fn());

    pending.forEach((fn) => fn()); // Microtasks land
    expect(calls).toEqual(["push"]); // Only one push, no go(-1)
    expect(stack.depth).toBe(1);
  });

  // In browsers, queueMicrotask is a method on window and must be called with window as receiver;
  // storing it as a bare reference in an instance field then calling this.schedule(...) changes
  // the receiver to the NavStack instance, causing Chrome to throw "Illegal invocation" — throwing
  // in an effect causes React to unmount the entire tree, making it look like clicking a tab blanks
  // the app. Node's queueMicrotask doesn't validate receiver, so we swap in an implementation that
  // does, porting browser semantics into the unit test.
  it("default scheduler calls queueMicrotask with global as receiver (not bare reference)", async () => {
    const real = globalThis.queueMicrotask;
    const seen: unknown[] = [];
    globalThis.queueMicrotask = function (this: unknown, cb: () => void) {
      seen.push(this);
      if (this !== undefined && this !== globalThis) throw new TypeError("Illegal invocation");
      return real(cb);
    };
    try {
      const { history, calls } = fakeHistory();
      const stack = new NavStack(history, () => "hold"); // Don't inject scheduler, use default
      stack.start();
      expect(() => stack.push(vi.fn())).not.toThrow();
      await Promise.resolve();
      await Promise.resolve();
      expect(calls).toEqual(["push", "push"]); // Sentinel + this layer, showing microtask actually ran
      expect(seen.every((r) => r === undefined || r === globalThis)).toBe(true);
    } finally {
      globalThis.queueMicrotask = real;
    }
  });

  it("bottom return: hold intercepts and pushes back sentinel, leave truly leaves", () => {
    const hold = mk(() => "hold");
    hold.stack.handlePopState(); // Consume the sentinel
    expect(hold.calls).toEqual(["push"]); // Sentinel is pushed back
    hold.calls.length = 0;
    hold.stack.handlePopState(); // Sentinel is still there, can intercept again
    expect(hold.calls).toEqual(["push"]);

    const onRoot = vi.fn<() => RootBackResult>().mockReturnValue("leave");
    const leave = mk(onRoot);
    leave.stack.handlePopState();
    expect(onRoot).toHaveBeenCalledTimes(1);
    expect(leave.calls).toEqual(["go(-1)"]); // Pass through: exit document
  });

  it("when there's an overlay, popstate doesn't reach the bottom return", () => {
    const onRoot = vi.fn<() => RootBackResult>().mockReturnValue("hold");
    const { stack } = mk(onRoot);
    stack.push(vi.fn());

    stack.handlePopState(); // Close the overlay
    expect(onRoot).not.toHaveBeenCalled();

    stack.handlePopState(); // This time is the bottom
    expect(onRoot).toHaveBeenCalledTimes(1);
  });
});
