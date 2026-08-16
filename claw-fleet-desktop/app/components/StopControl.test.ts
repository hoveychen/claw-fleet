import { describe, expect, it } from "vitest";
import { stopMode, canControl, usesSourceInterrupt } from "./StopControl";
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

/**
 * dsh has no per-session process: every session's turn runs inside one shared
 * `dsh web`, so the pid on its SessionInfo is that *server's*. Before this,
 * `stopMode` fell through to the `!pidPrecise` branch and the button killed by
 * *workspace* — and had the session ever been marked precise it would have
 * signalled the shared server, stopping every dsh session on the machine and
 * Fleet's own server with them. These pin the routing.
 */
describe("StopControl for a source with no per-session process", () => {
  const dshSession = (over: Partial<SessionInfo> = {}): SessionInfo =>
    runningFleetSession({
      agentSource: "dsh",
      jsonlPath: "dsh://session-abc",
      // What DshSource actually reports: the shared server's pid, and no
      // precision claim over it.
      pid: 4242,
      pidPrecise: false,
      ...over,
    });

  it("routes a running dsh session to the gentle, source-level interrupt", () => {
    expect(usesSourceInterrupt(dshSession())).toBe(true);
    expect(stopMode(dshSession())).toBe("interrupt");
  });

  it("offers nothing once the dsh session is idle — a turn cancel needs a turn", () => {
    expect(stopMode(dshSession({ status: "idle" }))).toBe("spent");
  });

  it("leaves the pid-based sources exactly as they were", () => {
    expect(usesSourceInterrupt(runningFleetSession())).toBe(false);
    expect(usesSourceInterrupt(runningFleetSession({ agentSource: "codex" }))).toBe(false);
    // The Claude case that must keep interrupting by pid.
    expect(stopMode(runningFleetSession())).toBe("interrupt");
  });
});
