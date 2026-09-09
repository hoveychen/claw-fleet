import { describe, expect, it } from "vitest";
import { RESULT_MARKER, parseResult, runHarnessAction, splitLines, type HarnessTransport } from "./harnessInstall";

/** A `/proc_output` script: one entry per poll. */
interface Poll {
  text: string;
  status: "starting" | "running" | "exited";
  exitCode?: number | null;
}

/**
 * Drive `runHarnessAction` over a scripted output stream.
 *
 * Encodes each poll's text as base64 the way the route does, so the decode
 * path (and the CJK case) is exercised rather than stubbed.
 *
 * Faithful about the offset, which matters: the real `/proc_output` answers
 * only the bytes *after* the offset the client sends, so polling an exited proc
 * again yields an empty chunk rather than the same text twice. A fake that
 * replayed its last entry would hide a double-emit bug instead of catching it.
 */
function harness(polls: Poll[]) {
  const emitted: { source: string; line: string }[] = [];
  let i = 0;
  const t: HarnessTransport = {
    start: async () => ({ id: "p1", status: "running" }),
    output: async (_id, offset) => {
      const p = polls[Math.min(i++, polls.length - 1)];
      // Past the end of the script the proc is just sitting there, exited.
      const text = i > polls.length ? "" : p.text;
      const bytes = new TextEncoder().encode(text);
      return {
        dataB64: btoa(String.fromCharCode(...bytes)),
        nextOffset: offset + bytes.length,
        record: { id: "p1", status: p.status, exitCode: p.exitCode ?? null },
      };
    },
    emitProgress: (source, line) => emitted.push({ source, line }),
    sleep: async () => {},
  };
  return { t, emitted };
}

const okLine = (payload: unknown) => `${RESULT_MARKER} ${JSON.stringify({ ok: payload })}\r\n`;

describe("splitLines", () => {
  it("strips the pty's carriage returns and holds back a partial line", () => {
    const { lines, rest } = splitLines("first\r\nsecond\r\nthi");
    expect(lines).toEqual(["first", "second"]);
    expect(rest).toBe("thi");
  });
});

describe("parseResult", () => {
  it("takes the last marker line, so an echoed command cannot win", () => {
    const result = parseResult([
      `$ echo ${RESULT_MARKER} {"ok":"decoy"}`,
      "working",
      `${RESULT_MARKER} {"ok":"real"}`,
    ]);
    expect(result?.ok).toBe("real");
  });

  it("ignores a marker line whose payload is not JSON", () => {
    expect(parseResult([`${RESULT_MARKER} not-json`])).toBeNull();
  });
});

describe("runHarnessAction", () => {
  it("streams progress lines and resolves with the typed ok payload", async () => {
    const { t, emitted } = harness([
      { text: "$ npm install -g dsh\r\nfetching…\r\n", status: "running" },
      { text: `verifying installation…\r\n${okLine({ source: "dsh", installed: true })}`, status: "exited", exitCode: 0 },
    ]);
    const value = await runHarnessAction(t, "/harness_install", { source: "dsh" }, "dsh");

    expect(value).toEqual({ source: "dsh", installed: true });
    // The marker line is plumbing and must not reach the panel's log tail.
    expect(emitted.map((e) => e.line)).toEqual([
      "$ npm install -g dsh",
      "fetching…",
      "verifying installation…",
    ]);
    expect(new Set(emitted.map((e) => e.source))).toEqual(new Set(["dsh"]));
  });

  /**
   * The one failure mode the panel *branches* on: `node-missing` is what makes
   * it offer the Node bootstrap instead of a dead end. If the code were lost in
   * transit the user would see "install failed" with no way forward.
   */
  it("rejects with the InstallError, code intact", async () => {
    const { t } = harness([
      {
        text: `${RESULT_MARKER} ${JSON.stringify({ err: { code: "node-missing", message: "npm not found" } })}\r\n`,
        status: "exited",
        exitCode: 1,
      },
    ]);
    await expect(runHarnessAction(t, "/harness_install", { source: "dsh" }, "dsh")).rejects.toMatchObject({
      code: "node-missing",
      message: "npm not found",
    });
  });

  /**
   * The host writes the last bytes and the exit status independently, so a
   * loop that stopped the instant it saw `exited` could miss the marker line
   * it exists to read — and the action would report "ended without reporting a
   * result" on a perfectly successful install.
   */
  it("drains one more poll after the proc reports exited", async () => {
    const { t } = harness([
      { text: "almost there\r\n", status: "exited", exitCode: 0 },
      { text: okLine("late"), status: "exited", exitCode: 0 },
    ]);
    await expect(runHarnessAction(t, "/harness_install", { source: "dsh" }, "dsh")).resolves.toBe("late");
  });

  it("accepts a final marker line that never got its newline", async () => {
    const { t } = harness([
      { text: `${RESULT_MARKER} {"ok":"no-newline"}`, status: "exited", exitCode: 0 },
    ]);
    await expect(runHarnessAction(t, "/harness_install", { source: "dsh" }, "dsh")).resolves.toBe("no-newline");
  });

  /**
   * No marker means the process died before reporting (missing binary, killed
   * proc). The exit code and output tail are the only evidence, so they have to
   * be in the message — a bare "failed" would be unactionable.
   */
  it("reports the exit code and output tail when no result was ever printed", async () => {
    const { t } = harness([
      { text: "fleet: command not found\r\n", status: "exited", exitCode: 127 },
      { text: "", status: "exited", exitCode: 127 },
    ]);
    const err = await runHarnessAction(t, "/harness_install", { source: "dsh" }, "dsh").then(
      () => null,
      (e) => e as { code: string; message: string },
    );
    expect(err?.code).toBe("install-failed");
    expect(err?.message).toContain("exit 127");
    expect(err?.message).toContain("fleet: command not found");
  });

  it("decodes multi-byte output rather than mangling it into mojibake", async () => {
    const { t, emitted } = harness([
      { text: "正在安装…\r\n", status: "running" },
      { text: okLine("done"), status: "exited", exitCode: 0 },
    ]);
    await runHarnessAction(t, "/harness_install", { source: "dsh" }, "dsh");
    expect(emitted[0].line).toBe("正在安装…");
  });

  it("files the node bootstrap's progress under the source key the panel reads", async () => {
    const { t, emitted } = harness([
      { text: "downloading node\r\n", status: "running" },
      { text: okLine("/home/u/.fleet/node/bin/npm"), status: "exited", exitCode: 0 },
    ]);
    await runHarnessAction(t, "/harness_install_node", {}, "node");
    expect(emitted[0]).toEqual({ source: "node", line: "downloading node" });
  });
});
