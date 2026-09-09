import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { DeviceSwitcher, type DeviceStatus } from "./DeviceSwitcher";
import type { PairedDevice } from "../devices";

const noop = () => {};

function device(id: string, label: string, platform?: string): PairedDevice {
  return {
    kind: "relay",
    id,
    label,
    platform,
    secret: `secret-${id}`,
    relayBase: null,
    addedAt: 0,
  };
}

const online: DeviceStatus = { connected: true, agentOnline: true };

describe("DeviceSwitcher", () => {
  it("一台在册时只是一行标题，没有可点的切换器", () => {
    // 一个永远只有一个选项的下拉是噪音：没有第二台可切。
    const html = renderToStaticMarkup(
      <DeviceSwitcher
        devices={[device("d1", "Harrys-MacBook-Pro")]}
        activeId="d1"
        statusOf={() => online}
        open={false}
        onOpenChange={noop}
        onSwitch={noop}
        onManage={noop}
      />,
    );
    expect(html).toContain("Harrys-MacBook-Pro");
    expect(html).not.toContain("<button");
  });

  it("名字不知道时退回 Fleet，而不是留一格空白", () => {
    // 同源形态 / mock 那台合成设备的 label 就是空串。
    const html = renderToStaticMarkup(
      <DeviceSwitcher
        devices={[device("same-origin", "")]}
        activeId="same-origin"
        statusOf={() => undefined}
        open={false}
        onOpenChange={noop}
        onSwitch={noop}
        onManage={noop}
      />,
    );
    expect(html).toContain("Fleet");
  });

  it("多台在册时标题变成当前那台的名字 + 可展开", () => {
    const html = renderToStaticMarkup(
      <DeviceSwitcher
        devices={[device("d1", "Harrys-MacBook-Pro", "macos"), device("d2", "build-box", "linux")]}
        activeId="d2"
        statusOf={() => online}
        open={false}
        onOpenChange={noop}
        onSwitch={noop}
        onManage={noop}
      />,
    );
    expect(html).toContain("build-box");
    expect(html).toContain('aria-expanded="false"');
    // 收起来时不该有列表
    expect(html).not.toContain('role="listbox"');
  });

  it("展开后每台各一行，并如实报告各自的连通性", () => {
    // 头部那盏灯报的是「全体里最好的一条」，切换器要的正相反：哪一台掉了得看得见。
    const statuses: Record<string, DeviceStatus> = {
      d1: { connected: true, agentOnline: true },
      d2: { connected: true, agentOnline: false },
      d3: { connected: false, agentOnline: false },
    };
    const html = renderToStaticMarkup(
      <DeviceSwitcher
        devices={[device("d1", "mac"), device("d2", "linux-box"), device("d3", "office")]}
        activeId="d1"
        statusOf={(id) => statuses[id]}
        open
        onOpenChange={noop}
        onSwitch={noop}
        onManage={noop}
      />,
    );
    expect(html).toContain('role="listbox"');
    // 测试环境的语言是 en，所以这里断言英文 —— 顺带钉住这几条词条真的进了字典
    // （漏一条的表现是界面上突然冒出一句中文，而不是报错）。
    expect(html).toContain("Online");
    expect(html).toContain("Desktop offline");
    expect(html).toContain("Not connected");
    expect(html).toMatch(/data-kind="online"/);
    expect(html).toMatch(/data-kind="offline"/);
    expect(html).toMatch(/data-kind="down"/);
    // 当前那台被标成选中 —— 屏幕阅读器与那枚勾都靠它
    expect(html).toMatch(/aria-selected="true"[^>]*>(?:(?!aria-selected)[\s\S])*?mac/);
    expect(html).toContain("Manage devices");
  });
});
