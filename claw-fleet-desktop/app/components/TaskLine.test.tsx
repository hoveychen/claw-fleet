import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TaskLine, splitMarker, taskTip } from "./TaskLine";

const ITEM = "**P4** — 全绿验证 + 真 serve 端到端 → 汇报可合并 → `--no-ff` 合并回 main";

describe("splitMarker", () => {
  it("lifts the P-marker out and leaves the rest's markdown intact", () => {
    const { marker, rest } = splitMarker(ITEM);
    expect(marker).toBe("P4");
    // The remainder keeps its own markup — TaskLine renders it, so stripping
    // it here would be the very thing this redesign removed.
    expect(rest).toContain("`--no-ff`");
    expect(rest.startsWith("全绿验证")).toBe(true);
  });

  it("leaves a non-marker item verbatim", () => {
    expect(splitMarker("just a line").marker).toBeNull();
    expect(splitMarker("just a line").rest).toBe("just a line");
  });

  it("accepts a lettered marker (P3a) and a plain hyphen separator", () => {
    expect(splitMarker("**P3a** - do it").marker).toBe("P3a");
    expect(splitMarker("**P3a** - do it").rest).toBe("do it");
  });
});

describe("taskTip", () => {
  it("flattens to plain prose — a tooltip renders no markdown", () => {
    const tip = taskTip(ITEM);
    expect(tip.startsWith("P4 — ")).toBe(true);
    expect(tip).not.toContain("`");
    expect(tip).not.toContain("**");
  });

  it("truncates to the cap", () => {
    expect(taskTip(`**P1** — ${"x".repeat(400)}`, 20)).toBe(`P1 — ${"x".repeat(20)}…`);
  });
});

describe("TaskLine markdown", () => {
  const render = (text: string, state: "done" | "current" | "pending" = "pending") =>
    renderToStaticMarkup(<TaskLine text={text} state={state} />);

  it("renders the item's markdown instead of literal markers", () => {
    const html = render(ITEM);
    expect(html).toContain("<code");
    expect(html).not.toContain("`--no-ff`");
    expect(html).not.toContain("**P4**");
  });

  it("shows the marker as its own badge", () => {
    expect(render(ITEM)).toContain(">P4<");
  });

  it("stays inline while clamped — no block <p> to break the two-line clamp", () => {
    expect(render("**P1** — 一句话")).not.toContain("<p>");
  });

  it("renders emphasis inside the prose as real emphasis", () => {
    expect(render("**P2** — 注意 **不要** 改 main")).toContain("<strong>不要</strong>");
  });

  it("carries its state for the accent rail / strike-through", () => {
    expect(render(ITEM, "current")).toContain('data-state="current"');
    expect(render(ITEM, "done")).toContain('data-state="done"');
  });

  it("exposes the collapsed prose as a tooltip", () => {
    expect(render(ITEM)).toContain("title=");
  });
});
