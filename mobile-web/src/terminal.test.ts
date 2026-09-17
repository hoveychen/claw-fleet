import { describe, expect, it } from "vitest";
import {
  applyCtrl,
  createOutputPump,
  decodeOutput,
  encodeInput,
  type ProcOutputChunk,
  type ProcRecord,
} from "./terminal";

/** A fake pty log: slice by offset for increments, responses can be delayed. */
function fakeHost(log: string) {
  const record: ProcRecord = {
    id: "p1",
    workspacePath: "/w",
    command: "",
    status: "running",
    startedMs: 0,
    cols: 80,
    rows: 24,
  };
  let pending: Array<() => void> = [];
  return {
    read(offset: number | null): Promise<ProcOutputChunk> {
      const from = offset ?? 0;
      const slice = log.slice(from);
      return new Promise((resolve) => {
        // Response hangs here, test decides when to resolve—simulates slow link.
        pending.push(() =>
          resolve({ dataB64: btoa(slice), nextOffset: from + slice.length, record }),
        );
      });
    },
    /** Flush all pending requests at once, then drain microtasks. */
    async flush() {
      const fire = pending;
      pending = [];
      for (const f of fire) f();
      await Promise.resolve();
      await Promise.resolve();
    },
    get inFlight() {
      return pending.length;
    },
  };
}

describe("encodeInput", () => {
  it("survives non-ASCII input", () => {
    // Direct btoa("中") throws InvalidCharacterError (btoa only accepts latin1).
    // IME input of Chinese and emoji all goes through here; encoding errors silently drop keystrokes.
    expect(() => encodeInput("中文")).not.toThrow();
    expect(decodeOutput(encodeInput("中文 🚀"))).toEqual(
      new TextEncoder().encode("中文 🚀"),
    );
  });

  it("keeps control bytes byte-exact", () => {
    // Ctrl-C must be exactly 0x03; if UTF-8 or escape handling touches it, commands won't interrupt.
    expect(decodeOutput(encodeInput("\x03"))).toEqual(new Uint8Array([3]));
  });
});

describe("createOutputPump", () => {
  it("does not duplicate output when response is slower than polling interval", async () => {
    // Mobile phone via relay to desktop often takes > 300ms for proc_output roundtrip.
    // Offset advances only after await returns, so second tick sends the same offset again,
    // same pty echo gets written twice—screen shows `ls` as `llss`.
    const host = fakeHost("ls");
    const written: string[] = [];
    const pump = createOutputPump({
      read: host.read,
      write: (bytes) => written.push(new TextDecoder().decode(bytes)),
    });

    void pump.poll(); // tick 1: sent, not yet returned
    void pump.poll(); // tick 2: another tick 300ms later
    await host.flush();

    expect(written.join("")).toBe("ls");
  });

  it("continues advancing offset after previous round returns", async () => {
    const host = fakeHost("abc");
    const written: string[] = [];
    const pump = createOutputPump({
      read: host.read,
      write: (bytes) => written.push(new TextDecoder().decode(bytes)),
    });

    void pump.poll();
    await host.flush();
    void pump.poll(); // already read to the end, this tick should be empty
    await host.flush();

    expect(written.join("")).toBe("abc");
  });

  it("in-flight responses do not appear after stop", async () => {
    const host = fakeHost("x");
    const written: string[] = [];
    const pump = createOutputPump({
      read: host.read,
      write: (bytes) => written.push(new TextDecoder().decode(bytes)),
    });

    void pump.poll();
    pump.stop();
    await host.flush();

    expect(written).toEqual([]);
  });
});

describe("applyCtrl", () => {
  it("folds letters into control codes regardless of case", () => {
    expect(applyCtrl("c")).toBe("\x03");
    expect(applyCtrl("C")).toBe("\x03");
    expect(applyCtrl("a")).toBe("\x01");
    expect(applyCtrl("z")).toBe("\x1a");
  });

  it("covers the punctuation controls a soft keyboard can reach", () => {
    expect(applyCtrl("[")).toBe("\x1b");
    expect(applyCtrl("@")).toBe("\x00");
  });

  it("passes through what Ctrl has no meaning for", () => {
    // Swallowing would make it look like the key was lost—pass it through, worst case is just a normal character.
    expect(applyCtrl("1")).toBe("1");
    expect(applyCtrl("中")).toBe("中");
    // Complete sequences from the keyboard itself are not single characters and must never be folded.
    expect(applyCtrl("\x1b[A")).toBe("\x1b[A");
  });
});
