import { describe, expect, it } from "vitest";
import { sessionDetailKey } from "./sessionDetailKey";

describe("sessionDetailKey", () => {
  it("changes when navigation switches to another session", () => {
    expect(sessionDetailKey("device-a", "session-1")).not.toBe(
      sessionDetailKey("device-a", "session-2"),
    );
  });

  it("keeps same-id sessions on different devices isolated", () => {
    expect(sessionDetailKey("device-a", "session-1")).not.toBe(
      sessionDetailKey("device-b", "session-1"),
    );
  });

  it("stays stable while a session snapshot refreshes", () => {
    expect(sessionDetailKey("device-a", "session-1")).toBe(
      sessionDetailKey("device-a", "session-1"),
    );
  });
});
