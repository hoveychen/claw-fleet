import { describe, expect, it } from "vitest";
import { waitTimeoutSecs } from "./ToolUseBlock";

describe("waitTimeoutSecs", () => {
  it("formats whole-second timeouts without a decimal", () => {
    expect(waitTimeoutSecs({ cell_id: "54", max_tokens: 20000, yield_time_ms: 30000 })).toBe("30");
  });

  it("keeps one decimal for sub-second precision", () => {
    expect(waitTimeoutSecs({ yield_time_ms: 1500 })).toBe("1.5");
    expect(waitTimeoutSecs({ yield_time_ms: 500 })).toBe("0.5");
  });

  it("returns null when the timeout is missing or unusable", () => {
    expect(waitTimeoutSecs({ cell_id: "54" })).toBeNull();
    expect(waitTimeoutSecs({ yield_time_ms: 0 })).toBeNull();
    expect(waitTimeoutSecs({ yield_time_ms: -5 })).toBeNull();
    expect(waitTimeoutSecs({ yield_time_ms: "30000" })).toBeNull();
  });
});
