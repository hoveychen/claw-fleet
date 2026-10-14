import { describe, expect, it } from "vitest";
import type { SessionInfo, SessionStatus } from "../../types";
import { isAskTool, lookFor, STATE_COLORS, toolLook } from "./poses";

const s = (status: SessionStatus, currentTool: string | null = null, procAlive = false) =>
  ({ id: "x", status, currentTool, procAlive }) as SessionInfo;

describe("toolLook", () => {
  it("maps tool families to poses", () => {
    expect(toolLook("Edit").pose).toBe("type");
    expect(toolLook("MultiEdit").pose).toBe("type");
    expect(toolLook("Grep").pose).toBe("read");
    expect(toolLook("WebFetch").pose).toBe("read");
    expect(toolLook("Bash").pose).toBe("run");
    expect(toolLook("Task").pose).toBe("delegate");
    expect(toolLook("Agent").pose).toBe("delegate");
    expect(toolLook("mcp__github__create_issue").pose).toBe("run");
  });

  it("recognises the ask tools under any MCP prefix", () => {
    expect(isAskTool("AskUserQuestion")).toBe(true);
    expect(isAskTool("mcp__fleet__fleet__ask")).toBe(true);
    expect(isAskTool("fleet__ask")).toBe(true);
    expect(isAskTool("mcp__other__task")).toBe(false);
  });
});

describe("lookFor", () => {
  it("lets a waiting card win over everything else", () => {
    expect(lookFor(s("executing", "Bash"), "pending")).toEqual({
      pose: "wave",
      color: STATE_COLORS.waiting,
      alert: "pending",
    });
    expect(lookFor(s("idle"), "parked").alert).toBe("parked");
  });

  it("raises an alert for a native ask in flight", () => {
    expect(lookFor(s("waitingInput", "AskUserQuestion"))).toMatchObject({ pose: "wave", alert: "pending" });
  });

  it("poses working sessions by their current tool", () => {
    expect(lookFor(s("executing", "Edit")).pose).toBe("type");
    expect(lookFor(s("thinking", "Read")).pose).toBe("read");
    expect(lookFor(s("executing")).pose).toBe("run");
    expect(lookFor(s("thinking")).pose).toBe("think");
    expect(lookFor(s("streaming")).pose).toBe("type");
    expect(lookFor(s("delegating", "Task")).pose).toBe("delegate");
  });

  it("does not shout for a turn that simply ended", () => {
    const look = lookFor(s("waitingInput"));
    expect(look.alert).toBe("none");
    expect(look.pose).toBe("coffee");
  });

  it("shows trouble and parked states", () => {
    expect(lookFor(s("rateLimited")).pose).toBe("error");
    expect(lookFor(s("serverErrored")).pose).toBe("error");
    expect(lookFor(s("stuck")).pose).toBe("stuck");
    expect(lookFor(s("watching")).pose).toBe("watch");
    expect(lookFor(s("idle")).pose).toBe("idle");
    expect(lookFor(s("idle", null, true)).pose).toBe("coffee");
  });

  it("covers every status", () => {
    const all: SessionStatus[] = [
      "thinking", "executing", "streaming", "delegating", "processing", "waitingInput", "active",
      "idle", "rateLimited", "serverErrored", "remoteDisconnected", "stuck", "watching",
    ];
    for (const st of all) expect(lookFor(s(st)).pose, st).toBeTruthy();
  });
});
