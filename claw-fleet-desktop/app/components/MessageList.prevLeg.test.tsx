// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import "../i18n";
import i18n from "../i18n";
import { MessageList } from "./MessageList";
import type { RawMessage } from "../types";
import {
  legStartedAt,
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
  const T0 = Date.UTC(2026, 9, 20, 8);
  const leg = (sessionId: string, hop: number, messages: string[]): LoadedLeg<string> => ({
    sessionId,
    hop,
    title: sessionId,
    createdAtMs: T0 + hop,
    fullyLoaded: false,
    messages,
  });
  const current = (messages: string[]) => ({
    sessionId: "s4",
    hop: 4,
    title: "s4",
    createdAtMs: T0 + 4,
    fullyLoaded: true,
    messages,
  });

  it("returns the current list untouched when nothing is prepended", () => {
    const cur = current(["c1"]);
    expect(stitchLegs([], cur)).toBe(cur.messages);
    expect(prependedCount([])).toBe(0);
  });

  it("opens every leg with a heading, oldest on top, the current one marked", () => {
    const legs = [leg("s3", 3, ["b1", "b2"]), leg("s2", 2, ["a1"])];
    const out = stitchLegs(legs, current(["c1"]));
    const shape = out.map((m) =>
      typeof m === "string" ? m : `|${m._fleetLegHeading.hop}${m._fleetLegHeading.current ? "*" : ""}|`,
    );
    expect(shape).toEqual(["|2|", "a1", "|3|", "b1", "b2", "|4*|", "c1"]);
    expect(prependedCount(legs)).toBe(6);
  });

  it("dates a leg by its first message once loaded to it, by creation time before", () => {
    const rec = (ts: string) => ({ timestamp: ts });
    expect(legStartedAt({ messages: [{}, rec("2026-10-20T09:00:00.000Z")], fullyLoaded: true, createdAtMs: 7 })).toBe(
      Date.UTC(2026, 9, 20, 9),
    );
    expect(legStartedAt({ messages: [rec("2026-10-20T09:00:00.000Z")], fullyLoaded: false, createdAtMs: 7 })).toBe(7);
    expect(legStartedAt({ messages: [{}], fullyLoaded: true, createdAtMs: 7 })).toBe(7);
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

  const twoLegs = (): RawMessage[] =>
    stitchLegs(
      [
        {
          sessionId: "s3",
          hop: 3,
          title: "修 CI",
          createdAtMs: Date.UTC(2026, 9, 20, 7),
          fullyLoaded: true,
          messages: [user("p1", "old prompt"), reply("p2", "old answer")],
        },
      ],
      {
        sessionId: "s4",
        hop: 4,
        title: "接着修",
        createdAtMs: Date.UTC(2026, 9, 20, 9),
        fullyLoaded: true,
        messages: [user("u1", "current prompt")],
      },
    ) as RawMessage[];

  it("heads each leg at its start, the current leg marked as current", () => {
    render({ messages: twoLegs(), fullyLoaded: true, readOnlyPrefix: 4 });
    const headings = [...container!.querySelectorAll('[data-testid="leg-heading"]')];
    expect(headings.map((h) => h.textContent)).toEqual([
      expect.stringMatching(/^第 3 棒 · 修 CI · .+ 开始$/),
      expect.stringMatching(/^第 4 棒（当前）· 接着修 · .+ 开始$/),
    ]);
    expect(headings[1].getAttribute("data-current")).toBe("true");
    const text = container!.textContent ?? "";
    expect(text.indexOf("第 3 棒")).toBeLessThan(text.indexOf("old prompt"));
    expect(text.indexOf("old answer")).toBeLessThan(text.indexOf("第 4 棒（当前）"));
    expect(text.indexOf("第 4 棒（当前）")).toBeLessThan(text.indexOf("current prompt"));
  });

  it("formats the heading in English with the active locale", async () => {
    await act(async () => {
      await i18n.changeLanguage("en");
    });
    try {
      render({ messages: twoLegs(), fullyLoaded: true, readOnlyPrefix: 4 });
      const headings = [...container!.querySelectorAll('[data-testid="leg-heading"]')];
      expect(headings[0].textContent).toMatch(/^Leg 3 · 修 CI · started Oct 20, .+$/);
      expect(headings[1].textContent).toMatch(/^Leg 4 \(current\) · 接着修 · started .+$/);
    } finally {
      await act(async () => {
        await i18n.changeLanguage("zh");
      });
    }
  });

  it("shows no heading while nothing is prepended", () => {
    render({ messages: [user("u1", "hi")], fullyLoaded: true });
    expect(container!.querySelector('[data-testid="leg-heading"]')).toBeNull();
  });
});
