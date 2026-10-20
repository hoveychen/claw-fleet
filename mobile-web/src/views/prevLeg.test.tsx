import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { PrevLegBlock } from "./SessionDetailView";
import type { PrevLeg } from "./usePrevLegs";
import type { RawMessage } from "../types";
import { nextLeg } from "../../../shared-ts/prevLeg";

const LINKS = [
  { fromSessionId: "s1", toSessionId: "s2", handedAt: 1000 },
  { fromSessionId: "s2", toSessionId: "s3", handedAt: 2000 },
];
const SESSIONS = [{ id: "s1" }, { id: "s2" }, { id: "s3" }];

describe("nextLeg", () => {
  it("offers the predecessor only once the top leg reached its first message", () => {
    const base = { links: LINKS, topId: "s3", topHop: 3, sessions: SESSIONS };
    expect(nextLeg({ ...base, topFullyLoaded: false })).toBeNull();
    const n = nextLeg({ ...base, topFullyLoaded: true });
    expect(n?.hop).toBe(2);
    expect(n?.pred.id).toBe("s2");
    expect(n?.link.handedAt).toBe(2000);
  });

  it("ends at leg 1, before the chain is read, and when the predecessor is not scanned", () => {
    expect(nextLeg({ links: LINKS, topId: "s1", topHop: 1, topFullyLoaded: true, sessions: SESSIONS })).toBeNull();
    expect(nextLeg({ links: null, topId: "s3", topHop: 3, topFullyLoaded: true, sessions: SESSIONS })).toBeNull();
    expect(
      nextLeg({ links: LINKS, topId: "s3", topHop: 3, topFullyLoaded: true, sessions: [{ id: "s3" }] }),
    ).toBeNull();
  });
});

describe("PrevLegBlock", () => {
  const msg = (type: "user" | "assistant", uuid: string, text: string): RawMessage =>
    ({
      type,
      uuid,
      timestamp: "2026-10-20T08:00:00.000Z",
      message: { role: type, content: [{ type: "text", text }] },
    }) as unknown as RawMessage;

  const leg: PrevLeg = {
    sessionId: "s2",
    hop: 2,
    title: "修 CI",
    handedAt: Date.UTC(2026, 9, 20, 8, 30),
    jsonlPath: "/t/s2.jsonl",
    messages: [msg("user", "p1", "old prompt"), msg("assistant", "p2", "old answer")],
    n: 120,
    hasEarlier: false,
  };

  const html = renderToStaticMarkup(
    <PrevLegBlock leg={leg} client={null} expandedThinking={new Set()} onToggleThinking={() => {}} />,
  );

  it("renders the leg's rows followed by a divider naming it", () => {
    expect(html).toContain("old answer");
    expect(html).toContain('data-testid="leg-divider"');
    // The suite runs without a DOM, so whichever language the runner's locale picks.
    expect(html).toMatch(/第 2 棒 · 修 CI · .+ 交棒|Leg 2 · 修 CI · handed off .+/);
    expect(html.indexOf("old answer")).toBeLessThan(html.indexOf("leg-divider"));
  });

  it("stamps no selection anchors, so no side question forks over another leg", () => {
    expect(html).not.toContain("data-role");
    expect(html).not.toContain("data-msg-idx");
  });
});
