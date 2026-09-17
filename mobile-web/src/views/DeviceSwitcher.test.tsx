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
  it("Single registered device: just a title line, no clickable switcher", () => {
    // A dropdown with only one option is noise: no second device to switch to.
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

  it("When name is unknown, fall back to Fleet instead of leaving a blank", () => {
    // Same-origin form / mock synthesized device has label as empty string.
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

  it("Multiple registered devices: title becomes current device name + expandable", () => {
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
    // When closed, there should be no list
    expect(html).not.toContain('role="listbox"');
  });

  it("When expanded, one row per device and report each one's connectivity status", () => {
    // The header light reports "the best of all", switcher needs the opposite: which device went down is visible.
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
    // Test environment is en, so assert English here — incidentally pins these entries are really in the dictionary
    // (missing one shows Chinese suddenly in the UI instead of an error).
    expect(html).toContain("Online");
    expect(html).toContain("Desktop offline");
    expect(html).toContain("Not connected");
    expect(html).toMatch(/data-kind="online"/);
    expect(html).toMatch(/data-kind="offline"/);
    expect(html).toMatch(/data-kind="down"/);
    // Current device marked as selected — screen readers and the checkmark both rely on it
    expect(html).toMatch(/aria-selected="true"[^>]*>(?:(?!aria-selected)[\s\S])*?mac/);
    expect(html).toContain("Manage devices");
  });
});
