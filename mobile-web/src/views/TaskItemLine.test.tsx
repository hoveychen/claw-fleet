import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TaskItemLine } from "./TaskItemLine";

// Link component checks shell identity (Capacitor runtime); mobile-web doesn't
// use jsdom, so we mock it out.
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => false } }));

const ITEM = "**P4** — 全绿验证 + 真 serve 端到端 → `--no-ff` 合并回 main";

const render = (text: string, state: "done" | "current" | "pending" = "pending") =>
  renderToStaticMarkup(<TaskItemLine text={text} state={state} />);

describe("Mobile P task line", () => {
  // Boss's original words: "at minimum, text rendering should use markdown" —
  // after desktop was fixed, mobile still printed asterisks.
  it("body text uses real markdown, no longer prints asterisks and backticks", () => {
    const html = render(ITEM);
    expect(html).toContain("<code");
    expect(html).not.toContain("`--no-ff`");
    expect(html).not.toContain("**P4**");
  });

  it("marker extracted as independent badge", () => {
    expect(render(ITEM)).toContain(">P4<");
  });

  it("compressed line doesn't emit block-level <p>, or ellipsis gets broken by paragraph", () => {
    expect(render("**P1** — 一句话")).not.toContain("<p>");
  });

  it("bold in body text renders as real bold", () => {
    expect(render("**P2** — 注意 **不要** 改 main")).toContain("<strong>不要</strong>");
  });

  it("state passes through to data-state attribute for strikethrough and accent", () => {
    expect(render(ITEM, "done")).toContain('data-state="done"');
    expect(render(ITEM, "current")).toContain('data-state="current"');
  });
});
