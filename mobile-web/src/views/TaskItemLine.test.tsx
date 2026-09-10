import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TaskItemLine } from "./TaskItemLine";

// 链接组件表要判断壳身份（Capacitor 运行时），mobile-web 不装 jsdom，故 mock 掉。
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => false } }));

const ITEM = "**P4** — 全绿验证 + 真 serve 端到端 → `--no-ff` 合并回 main";

const render = (text: string, state: "done" | "current" | "pending" = "pending") =>
  renderToStaticMarkup(<TaskItemLine text={text} state={state} />);

describe("手机端 P 任务行", () => {
  // 老板的原话是「至少文本渲染用上 markdown」——桌面改完后手机上仍是星号。
  it("正文走真 markdown，不再把星号和反引号印出来", () => {
    const html = render(ITEM);
    expect(html).toContain("<code");
    expect(html).not.toContain("`--no-ff`");
    expect(html).not.toContain("**P4**");
  });

  it("marker 抽成独立徽章", () => {
    expect(render(ITEM)).toContain(">P4<");
  });

  it("压行态不出块级 <p>，否则一行的省略号会被段落打断", () => {
    expect(render("**P1** — 一句话")).not.toContain("<p>");
  });

  it("正文里的加粗渲染成真加粗", () => {
    expect(render("**P2** — 注意 **不要** 改 main")).toContain("<strong>不要</strong>");
  });

  it("状态透到 data-state，供删除线与 accent 用", () => {
    expect(render(ITEM, "done")).toContain('data-state="done"');
    expect(render(ITEM, "current")).toContain('data-state="current"');
  });
});
