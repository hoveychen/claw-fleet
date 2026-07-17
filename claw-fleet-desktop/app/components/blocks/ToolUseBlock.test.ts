import { describe, expect, it } from "vitest";
import {
  codexToolSummary,
  parseExecCommand,
  parsePatchFiles,
  readRange,
  searchFlags,
  timeoutMsToSecs,
  waitTimeoutSecs,
} from "./ToolUseBlock";

// Stub translator: echoes the key plus any interpolation params so tests can
// assert which i18n key + values the router picked, without loading i18next.
const t = (key: string, opts?: Record<string, unknown>) =>
  opts ? `${key}|${JSON.stringify(opts)}` : key;

describe("timeoutMsToSecs", () => {
  it("formats whole-second timeouts without a decimal", () => {
    expect(timeoutMsToSecs(30000)).toBe("30");
  });

  it("keeps one decimal for sub-second precision", () => {
    expect(timeoutMsToSecs(1500)).toBe("1.5");
    expect(timeoutMsToSecs(500)).toBe("0.5");
  });

  it("coerces numeric strings (codex serialises some timeouts as strings)", () => {
    expect(timeoutMsToSecs("30000")).toBe("30");
    expect(timeoutMsToSecs("10000")).toBe("10");
  });

  it("returns null when the value is absent or unusable", () => {
    expect(timeoutMsToSecs(undefined)).toBeNull();
    expect(timeoutMsToSecs(0)).toBeNull();
    expect(timeoutMsToSecs(-5)).toBeNull();
    expect(timeoutMsToSecs("nope")).toBeNull();
  });
});

describe("waitTimeoutSecs (alias reading yield_time_ms)", () => {
  it("reads the wait tool's timeout field", () => {
    expect(waitTimeoutSecs({ cell_id: "54", max_tokens: 20000, yield_time_ms: 30000 })).toBe("30");
    expect(waitTimeoutSecs({ cell_id: "54" })).toBeNull();
  });
});

describe("codexToolSummary", () => {
  it("wait → timed key with seconds", () => {
    expect(codexToolSummary("wait", { cell_id: "54", yield_time_ms: 30000 }, t)).toBe(
      'detail.tool_wait_timed|{"secs":"30"}',
    );
  });

  it("wait without timeout → bare key", () => {
    expect(codexToolSummary("wait", { cell_id: "54" }, t)).toBe("detail.tool_wait");
  });

  it("write_stdin with typed input → stdin key carrying the text", () => {
    expect(codexToolSummary("write_stdin", { session_id: "1", chars: "yes\n" }, t)).toBe(
      'detail.tool_stdin|{"text":"yes"}',
    );
  });

  it("write_stdin with empty chars → treated as a poll (wait)", () => {
    expect(
      codexToolSummary("write_stdin", { session_id: "1", chars: "", yield_time_ms: 1000 }, t),
    ).toBe('detail.tool_wait_timed|{"secs":"1"}');
  });

  it("wait_agent → subagent-wait key with string timeout coerced", () => {
    expect(codexToolSummary("wait_agent", { timeout_ms: "30000" }, t)).toBe(
      'detail.tool_wait_agent_timed|{"secs":"30"}',
    );
    expect(codexToolSummary("wait_agent", {}, t)).toBe("detail.tool_wait_agent");
  });

  it("spawn_agent → named by task_name, else agent_type, else bare", () => {
    expect(
      codexToolSummary("spawn_agent", { task_name: "create_sub_file", message: "gAAA..." }, t),
    ).toBe('detail.tool_spawn_agent_named|{"name":"create_sub_file"}');
    expect(codexToolSummary("spawn_agent", { agent_type: "explorer" }, t)).toBe(
      'detail.tool_spawn_agent_named|{"name":"explorer"}',
    );
    expect(codexToolSummary("spawn_agent", {}, t)).toBe("detail.tool_spawn_agent");
  });

  it("update_plan / request_user_input → their own keys", () => {
    expect(codexToolSummary("update_plan", { plan: "[...]" }, t)).toBe("detail.tool_update_plan");
    expect(codexToolSummary("request_user_input", { questions: "[...]" }, t)).toBe(
      "detail.tool_request_input",
    );
  });

  it("returns null for tools it does not handle (falls back to formatInput)", () => {
    expect(codexToolSummary("exec_command", { cmd: "pwd" }, t)).toBeNull();
    expect(codexToolSummary("Bash", { command: "ls" }, t)).toBeNull();
  });

  it("apply_patch → names a single touched file, action-aware + basename", () => {
    const patch = `*** Begin Patch
*** Update File: /ws/app/foo.ts
@@
-old
+new
*** End Patch`;
    expect(codexToolSummary("apply_patch", { command: patch }, t)).toBe(
      'detail.tool_patch_update|{"path":"foo.ts"}',
    );
    expect(
      codexToolSummary("apply_patch", { command: "*** Begin Patch\n*** Add File: a.ts\n" }, t),
    ).toBe('detail.tool_patch_add|{"path":"a.ts"}');
    expect(
      codexToolSummary("apply_patch", { command: "*** Begin Patch\n*** Delete File: a.ts\n" }, t),
    ).toBe('detail.tool_patch_delete|{"path":"a.ts"}');
  });

  it("apply_patch → counts multiple files", () => {
    const patch = `*** Begin Patch
*** Update File: a.ts
*** Add File: b.ts
*** Delete File: c.ts
*** End Patch`;
    expect(codexToolSummary("apply_patch", { command: patch }, t)).toBe(
      'detail.tool_patch_multi|{"count":3}',
    );
  });

  it("apply_patch with an unparseable body → null (formatInput shows it raw)", () => {
    expect(codexToolSummary("apply_patch", { command: "garbage" }, t)).toBeNull();
    expect(codexToolSummary("apply_patch", {}, t)).toBeNull();
  });
});

describe("parseExecCommand", () => {
  it("extracts cmd + workdir from a code-mode exec harness (double-quoted)", () => {
    const command =
      '// 按要求运行完整 Rust 回归并等待结束。\n' +
      "const r = await tools.exec_command({\n" +
      '  cmd: "cargo test -p claw-fleet-core -p fleet-cli",\n' +
      '  workdir: "/Users/hoveychen/workspace/claude-fleet/.worktrees/x",\n' +
      "  yield_time_ms: 30000,\n" +
      "  max_output_tokens: 12000\n" +
      "});\ntext(JSON.stringify(r));\n";
    expect(parseExecCommand(command)).toEqual({
      cmd: "cargo test -p claw-fleet-core -p fleet-cli",
      workdir: "/Users/hoveychen/workspace/claude-fleet/.worktrees/x",
    });
  });

  it("unescapes JSON escapes inside a double-quoted cmd", () => {
    expect(parseExecCommand('tools.exec_command({ cmd: "echo \\"hi\\"\\n" })')).toEqual({
      cmd: 'echo "hi"\n',
      workdir: undefined,
    });
  });

  it("handles single-quoted and backtick literals", () => {
    expect(parseExecCommand("exec_command({ cmd: 'ls -la' })").cmd).toBe("ls -la");
    expect(parseExecCommand("exec_command({ cmd: `pwd` })").cmd).toBe("pwd");
  });

  it("returns {} for a plain shell command (older-style exec, no cmd: field)", () => {
    expect(parseExecCommand("cargo build")).toEqual({ cmd: undefined, workdir: undefined });
  });
});

describe("readRange", () => {
  it("offset + limit → closed line range", () => {
    expect(readRange(40, 80)).toBe("lines 40–119");
  });

  it("offset only → open range", () => {
    expect(readRange(40, undefined)).toBe("from line 40");
  });

  it("limit only → head cap", () => {
    expect(readRange(undefined, 80)).toBe("first 80 lines");
  });

  it("neither → null (whole file)", () => {
    expect(readRange(undefined, undefined)).toBeNull();
    expect(readRange("40", "80")).toBeNull();
  });
});

describe("searchFlags", () => {
  it("collects the present scalar flags, ws-relative path first", () => {
    expect(
      searchFlags(
        { pattern: "foo", path: "/ws/app", glob: "*.ts", type: "rust", output_mode: "content", "-i": true },
        "/ws",
      ),
    ).toEqual(["app", "glob *.ts", "type rust", "content", "-i"]);
  });

  it("skips absent / empty fields (pattern is never a chip)", () => {
    expect(searchFlags({ pattern: "foo" })).toEqual([]);
    expect(searchFlags({ pattern: "foo", path: "  ", glob: "" })).toEqual([]);
  });
});

describe("parsePatchFiles", () => {
  it("extracts op + path for every file header", () => {
    const patch = `*** Begin Patch
*** Add File: src/new.ts
*** Update File: src/old.ts
*** Delete File: src/gone.ts
*** End Patch`;
    expect(parsePatchFiles(patch)).toEqual([
      { op: "Add", path: "src/new.ts" },
      { op: "Update", path: "src/old.ts" },
      { op: "Delete", path: "src/gone.ts" },
    ]);
  });

  it("returns [] when there are no file headers", () => {
    expect(parsePatchFiles("just some text")).toEqual([]);
  });
});
