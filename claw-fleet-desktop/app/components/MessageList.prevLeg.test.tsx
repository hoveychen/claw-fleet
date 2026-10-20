// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import "../i18n";
import i18n from "../i18n";
import { MessageList } from "./MessageList";
import type { RawMessage } from "../types";
import {
  predecessorLink,
  prependedCount,
  stitchLegs,
  type LoadedLeg,
} from "../../../shared-ts/prevLeg";
import type { PrevLegOffer } from "../usePrevLegs";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root | null = null;
let container: HTMLDivElement | null = null;

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  void i18n.changeLanguage("zh");
});
function cleanup() {
  if (root) act(() => root!.unmount());
  container?.remove();
  root = null;
  container = null;
}
afterEach(cleanup);

function user(uuid: string, text: string): RawMessage {
  return {
    type: "user",
    uuid,
    timestamp: "2026-10-20T10:00:00.000Z",
    message: { role: "user", content: [{ type: "text", text }] },
  } as unknown as RawMessage;
}

function reply(uuid: string, text: string): RawMessage {
  return {
    type: "assistant",
    uuid,
    timestamp: "2026-10-20T10:00:01.000Z",
    message: { role: "assistant", content: [{ type: "text", text }] },
  } as unknown as RawMessage;
}

function render(props: Partial<Parameters<typeof MessageList>[0]> & { messages: RawMessage[] }) {
  container = document.createElement("div");
  document.body.appendChild(container);
  const r = createRoot(container);
  root = r;
  act(() => r.render(<MessageList isLoading={false} {...props} />));
}

const prevButton = () => container!.querySelector('[data-testid="load-prev-leg"]');

describe("stitchLegs", () => {
  const leg = (sessionId: string, hop: number, messages: string[]): LoadedLeg<string> => ({
    sessionId,
    hop,
    title: sessionId,
    handedAt: Date.UTC(2026, 9, 20, 8),
    messages,
  });

  it("returns the current list untouched when nothing is prepended", () => {
    const current = ["c1"];
    expect(stitchLegs([], current)).toBe(current);
  });

  it("puts the oldest leg on top, a divider below each prepended leg", () => {
    const out = stitchLegs([leg("s3", 3, ["b1", "b2"]), leg("s2", 2, ["a1"])], ["c1"]);
    const shape = out.map((m) => (typeof m === "string" ? m : `|${m._fleetLegDivider.hop}|`));
    expect(shape).toEqual(["a1", "|2|", "b1", "b2", "|3|", "c1"]);
    expect(prependedCount([leg("s3", 3, ["b1", "b2"]), leg("s2", 2, ["a1"])])).toBe(5);
  });

  it("names the predecessor by the link whose successor is the open leg", () => {
    const links = [
      { fromSessionId: "s1", toSessionId: "s2", handedAt: 1 },
      { fromSessionId: "s2", toSessionId: "s3", handedAt: 2 },
    ];
    expect(predecessorLink(links, "s3")?.fromSessionId).toBe("s2");
    expect(predecessorLink(links, "s1")).toBeNull();
  });
});

describe("load previous relay leg", () => {
  const offer = (onLoad = vi.fn(async () => {})): PrevLegOffer => ({ hop: 3, loading: false, onLoad });

  it("is offered only once the current leg reaches its first message", () => {
    render({ messages: [user("u1", "hi")], fullyLoaded: false, onLoadEarlier: () => {}, prevLeg: offer() });
    expect(prevButton()).toBeNull();
    cleanup();

    render({ messages: [user("u1", "hi")], fullyLoaded: true, prevLeg: offer() });
    expect(prevButton()?.textContent).toContain("加载第 3 棒");
  });

  it("calls the loader on click", async () => {
    const onLoad = vi.fn(async () => {});
    render({ messages: [user("u1", "hi")], fullyLoaded: true, prevLeg: offer(onLoad) });
    await act(async () => {
      (prevButton() as HTMLButtonElement).click();
    });
    expect(onLoad).toHaveBeenCalledTimes(1);
  });

  it("draws a divider naming the prepended leg instead of a bubble", () => {
    const messages = stitchLegs(
      [
        {
          sessionId: "s3",
          hop: 3,
          title: "修 CI",
          handedAt: Date.UTC(2026, 9, 20, 8),
          messages: [user("p1", "old prompt"), reply("p2", "old answer")],
        },
      ],
      [user("u1", "current prompt")],
    ) as RawMessage[];
    render({ messages, fullyLoaded: true, readOnlyPrefix: 3 });
    const divider = container!.querySelector('[data-testid="leg-divider"]');
    expect(divider?.textContent).toMatch(/^第 3 棒 · 修 CI · .+ 交棒$/);
    const text = container!.textContent ?? "";
    expect(text.indexOf("old answer")).toBeLessThan(text.indexOf("第 3 棒"));
    expect(text.indexOf("第 3 棒")).toBeLessThan(text.indexOf("current prompt"));
  });
});
