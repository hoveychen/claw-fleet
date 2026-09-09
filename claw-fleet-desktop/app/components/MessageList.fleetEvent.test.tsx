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
});
