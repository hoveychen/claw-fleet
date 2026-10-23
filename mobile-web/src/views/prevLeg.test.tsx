import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { LegRule, PrevLegBlock } from "./SessionDetailView";
import type { PrevLeg } from "./usePrevLegs";
import type { RawMessage } from "../types";
import { hasSuccessor, legHeading, nextLeg, successorLeg } from "../../../shared-ts/prevLeg";

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

describe("successorLeg", () => {
  it("names the leg that took over, titled from the scan", () => {
    const sessions = [{ id: "s1" }, { id: "s2", aiTitle: "Second leg" }];
    expect(successorLeg({ links: LINKS, sessionId: "s1", hop: 1, sessions })).toEqual({
      sessionId: "s2",
      hop: 2,
      title: "Second leg",
    });
  });

  it("falls back to the id before the successor is scanned, and is null on the last leg", () => {
    expect(successorLeg({ links: LINKS, sessionId: "s2", hop: 2, sessions: [] })?.title).toBe("s3…");
    expect(successorLeg({ links: LINKS, sessionId: "s3", hop: 3, sessions: SESSIONS })).toBeNull();
  });

  it("is only looked up once the chain grew past this leg", () => {
    expect(hasSuccessor(null)).toBe(false);
    expect(hasSuccessor({ hop: 3, chainLen: 3 })).toBe(false);
    expect(hasSuccessor({ hop: 2, chainLen: 3 })).toBe(true);
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
    createdAtMs: Date.UTC(2026, 9, 20, 7),
    fullyLoaded: true,
    jsonlPath: "/t/s2.jsonl",
    messages: [msg("user", "p1", "old prompt"), msg("assistant", "p2", "old answer")],
    n: 120,
  };

  const html = renderToStaticMarkup(
    <PrevLegBlock leg={leg} client={null} expandedThinking={new Set()} onToggleThinking={() => {}} />,
  );

  it("opens the leg's rows with a heading naming it", () => {
    expect(html).toContain("old answer");
    expect(html).toContain('data-testid="leg-heading"');
    // The suite runs without a DOM, so whichever language the runner's locale picks.
    expect(html).toMatch(/第 2 棒 · 修 CI · .+ 开始|Leg 2 · 修 CI · started .+/);
    expect(html).not.toContain("data-current");
    expect(html.indexOf("leg-heading")).toBeLessThan(html.indexOf("old prompt"));
  });

  it("dates the heading by the leg's first message once it is loaded to it", () => {
    expect(legHeading(leg, false).startedAt).toBe(Date.UTC(2026, 9, 20, 8));
    expect(legHeading({ ...leg, fullyLoaded: false }, false).startedAt).toBe(Date.UTC(2026, 9, 20, 7));
  });

  it("marks the current leg's heading as current", () => {
    const cur = renderToStaticMarkup(<LegRule info={legHeading({ ...leg, hop: 3, title: "接着修" }, true)} />);
    expect(cur).toContain('data-current="true"');
    expect(cur).toMatch(/第 3 棒（当前）· 接着修 · .+ 开始|Leg 3 \(current\) · 接着修 · started .+/);
  });

  it("stamps no selection anchors, so no side question forks over another leg", () => {
    expect(html).not.toContain("data-role");
    expect(html).not.toContain("data-msg-idx");
  });
});
