import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { FleetEventCard } from "./FleetEventCard";

describe("FleetEventCard", () => {
  it("keeps the injected prompt collapsed behind a passive watch header", () => {
    const html = renderToStaticMarkup(
      <FleetEventCard
        event={{ kind: "watch", status: "fired", id: "w7" }}
        text="large captured output"
      />,
    );
    expect(html).toContain("fleet-event-card");
    expect(html).toContain("Fleet watch");
    expect(html).toContain("w7");
    expect(html).not.toContain("large captured output");
  });

  it("labels a decision-card reply and keeps the boss's answer folded", () => {
    const html = renderToStaticMarkup(
      <FleetEventCard event={{ kind: "decision", status: "answered" }} text="【回答】发版" />,
    );
    expect(html).toContain('data-kind="decision"');
    expect(html).toContain("Decision card reply");
    expect(html).toContain("Answered");
    expect(html).not.toContain("【回答】发版");
  });

  it("folds a Fleet notice (e.g. a resolved decision card) instead of a boss bubble", () => {
    const html = renderToStaticMarkup(
      <FleetEventCard
        event={{ kind: "notice", status: "delivered" }}
        text="[fleet] 决策卡 `r1` 老板已经处理了。"
      />,
    );
    expect(html).toContain('data-kind="notice"');
    expect(html).toContain("Fleet notice");
    expect(html).toContain("Delivered");
    expect(html).not.toContain("老板已经处理了");
  });
});
