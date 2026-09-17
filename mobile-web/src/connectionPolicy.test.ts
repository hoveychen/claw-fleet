import { describe, expect, it } from "vitest";
import {
  connectDelayMs,
  CONNECT_STAGGER_MAX_MS,
  HIDDEN_DISCONNECT_MS,
  shouldConnect,
  usagePollMs,
  USAGE_POLL_ACTIVE_MS,
  USAGE_POLL_BACKGROUND_MS,
} from "./connectionPolicy";

const T0 = 1_700_000_000_000;

describe("shouldConnect", () => {
  it("keeps every device connected while the page is visible", () => {
    expect(shouldConnect({ visible: true, hiddenSince: 0 }, T0)).toBe(true);
  });

  // Dropping N connections on each background switch and re-handshaking on return is
  // more annoying than the tiny power savings.
  it("rides out a brief hide", () => {
    expect(shouldConnect({ visible: false, hiddenSince: T0 }, T0 + 5_000)).toBe(true);
  });

  // Once truly backgrounded, no need to stay connected: background delivery is push
  // (subscription registration lives on relay, independent of this socket).
  it("drops the connections once hidden past the grace window", () => {
    expect(shouldConnect({ visible: false, hiddenSince: T0 }, T0 + HIDDEN_DISCONNECT_MS)).toBe(
      false,
    );
    expect(shouldConnect({ visible: false, hiddenSince: T0 }, T0 + 10 * 60_000)).toBe(false);
  });
});

describe("usagePollMs", () => {
  it("asks the device the user is looking at more often", () => {
    expect(usagePollMs(true)).toBe(USAGE_POLL_ACTIVE_MS);
    expect(usagePollMs(false)).toBe(USAGE_POLL_BACKGROUND_MS);
    expect(USAGE_POLL_BACKGROUND_MS).toBeGreaterThan(USAGE_POLL_ACTIVE_MS);
  });
});

describe("connectDelayMs", () => {
  it("staggers so N sockets do not all handshake at once", () => {
    expect(connectDelayMs(0)).toBe(0);
    expect(connectDelayMs(1)).toBeGreaterThan(0);
    expect(connectDelayMs(2)).toBeGreaterThan(connectDelayMs(1));
  });

  // No matter how many devices, the last one shouldn't make the user think it's hung.
  it("caps the stagger", () => {
    expect(connectDelayMs(100)).toBe(CONNECT_STAGGER_MAX_MS);
  });
});
