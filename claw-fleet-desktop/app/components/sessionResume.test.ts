import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { resumeSession } from "./sessionResume";

const ARGS = { sessionId: "old", workspacePath: "/repo", agentSource: "claude-code" };
const REFUSAL = "retired: session old was taken over by 1234567890ab";

describe("resumeSession on a retired session", () => {
  beforeEach(() => invoke.mockReset());

  it("resends with allowRetired only after the boss confirms", async () => {
    invoke.mockRejectedValueOnce(REFUSAL).mockResolvedValueOnce(undefined);
    const confirm = vi.fn(() => true);
    await resumeSession(ARGS, confirm);
    expect(confirm).toHaveBeenCalledWith("1234567890ab");
    expect(invoke.mock.calls[1][1]).toMatchObject({ sessionId: "old", allowRetired: true });
  });

  it("rejects without resending when the boss declines", async () => {
    invoke.mockRejectedValueOnce(REFUSAL);
    await expect(resumeSession(ARGS, () => false)).rejects.toBeTruthy();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("passes any other error through untouched", async () => {
    invoke.mockRejectedValueOnce("Workspace directory not found: /repo");
    const confirm = vi.fn(() => true);
    await expect(resumeSession(ARGS, confirm)).rejects.toBe("Workspace directory not found: /repo");
    expect(confirm).not.toHaveBeenCalled();
  });
});
