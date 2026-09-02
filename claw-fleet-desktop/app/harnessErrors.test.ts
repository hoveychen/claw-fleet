import { describe, expect, it } from "vitest";
import { classifyHarnessError } from "./harnessErrors";

describe("classifyHarnessError", () => {
  it("matches the real backend spawn-error strings", () => {
    // Verbatim messages from session_launch.rs / claude_cli.rs /
    // codex_source.rs / codex_launch.rs / dsh_source.rs.
    expect(classifyHarnessError("Claude CLI not found on PATH")).toBe("claude-code");
    expect(classifyHarnessError("claude binary not found")).toBe("claude-code");
    expect(classifyHarnessError("Codex binary not found")).toBe("codex");
    expect(
      classifyHarnessError(
        "Codex CLI not found (no standalone install, VSCode extension, or codex on PATH)",
      ),
    ).toBe("codex");
    expect(classifyHarnessError("dsh is not installed (npm i -g @deepseek-ai/dsh)")).toBe("dsh");
  });

  it("leaves unrelated errors unclassified", () => {
    expect(classifyHarnessError("workspace path does not exist")).toBeNull();
    expect(classifyHarnessError("permission denied")).toBeNull();
    expect(classifyHarnessError("")).toBeNull();
  });
});
