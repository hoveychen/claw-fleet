// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import "../i18n";
import { MessageList } from "./MessageList";
import type { RawMessage } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root;
let container: HTMLDivElement;

beforeAll(() => { Element.prototype.scrollIntoView = () => {}; });
afterEach(() => { act(() => root.unmount()); container.remove(); });

describe("MessageList Fleet automation events", () => {
  it("renders a watch resume as a passive card, not a user bubble", () => {
    const msg: RawMessage = {
      type: "user",
      fleetEvent: { kind: "watch", status: "fired", id: "w1" },
      message: { role: "user", content: "你注册的 Fleet watch `w1` 触发了" },
    };
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => root.render(<MessageList messages={[msg]} isLoading={false} />));

    expect(container.querySelector('[data-testid="fleet-event-card"]')).not.toBeNull();
    expect(container.querySelector('[class*="user_text"]')).toBeNull();
    expect(container.textContent).toContain("w1");
  });

  // The three records a timed-out Decision Card leaves behind, in transcript
  // order. Together they used to occupy a bubble, a filler row and a wall of
  // text; the assertions below pin what each collapses to.
  it("collapses the decision-timeout trio into a rule plus one card", () => {
    const msgs: RawMessage[] = [
      { type: "user", message: { role: "user", content: "[Request interrupted by user]" } },
      {
        type: "assistant",
        message: {
          role: "assistant",
          model: "<synthetic>",
          content: [{ type: "text", text: "No response requested." }],
        },
      },
      {
        type: "user",
        fleetEvent: { kind: "decision", status: "answered" },
        message: { role: "user", content: "[Fleet] 你上一轮通过决策卡向老板提问…\n\n【问题 1】发版？" },
      },
    ];
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => root.render(<MessageList messages={msgs} isLoading={false} />));

    expect(container.querySelector('[data-testid="interrupt-rule"]')).not.toBeNull();
    expect(container.textContent).not.toContain("[Request interrupted by user]");
    expect(container.textContent).not.toContain("No response requested.");
    // Collapsed by default: the header shows, the answer body does not.
    expect(container.querySelector('[data-kind="decision"]')).not.toBeNull();
    expect(container.textContent).not.toContain("【问题 1】发版？");
  });

  it("keeps other <synthetic> assistant records, which carry real news", () => {
    const msgs: RawMessage[] = [
      {
        type: "assistant",
        message: {
          role: "assistant",
          model: "<synthetic>",
          content: [{ type: "text", text: "Failed to authenticate. API Error: 403" }],
        },
      },
    ];
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => root.render(<MessageList messages={msgs} isLoading={false} />));

    expect(container.textContent).toContain("API Error: 403");
  });
});
