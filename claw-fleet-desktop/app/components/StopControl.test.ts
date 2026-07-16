import { describe, expect, it } from "vitest";
import { stopMode, canControl } from "./StopControl";
import { canEnqueueSession, NEW_SESSION_ENTRYPOINT } from "../types";
import type { SessionInfo } from "../types";

/**
 * The enqueue composer surfaces a StopControl in its send slot while the turn
 * is running. This pins the behaviour that dock relies on: for exactly the
 * sessions `canEnqueueSession` accepts (a Fleet-owned main session mid-turn),
 * the Stop button must (a) render at all — `canControl` — and (b) resolve to
 * the gentle, resumable `interrupt` tier, NOT a hard kill. If a future change
 * to `stopMode` demoted this case to `stop`, the composer button would start
 * killing the process tree behind a friendly label; this test guards that.
 */
function runningFleetSession(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id: "s1",
    jsonlPath: "/p/s1.jsonl",
    workspacePath: "/w",
    workspaceName: "w",
    status: "executing",
    aiTitle: "title",
    slug: null,
    lastMessagePreview: "hi",
    lastActivityMs: 1000,
    createdAtMs: 500,
    pid: 42,
    pidPrecise: true,
    procAlive: true,
    isSubagent: false,
    entrypoint: NEW_SESSION_ENTRYPOINT,
    agentSource: "claude-code",
    ...over,
  } as unknown as SessionInfo;
}

describe("StopControl in the enqueue composer", () => {
  it("a running Fleet session is enqueue-able, controllable, and interrupts (not kills)", () => {
    const s = runningFleetSession();
    // The dock only mounts the composer (and thus the Stop button) when this holds.
    expect(canEnqueueSession(s)).toBe(true);
    // ResumeComposer gates the Stop button on canControl.
    expect(canControl(s)).toBe(true);
    // Precise pid + in-flight + Fleet-owned ⇒ the reversible interrupt tier.
    expect(stopMode(s)).toBe("interrupt");
  });

  it("falls back to a hard stop when the pid is imprecise", () => {
    // No unambiguous pid to SIGINT, so the friendly interrupt is unsafe; the
    // button drops to the confirm-gated kill tier instead.
    expect(stopMode(runningFleetSession({ pidPrecise: false }))).toBe("stop");
  });
});
