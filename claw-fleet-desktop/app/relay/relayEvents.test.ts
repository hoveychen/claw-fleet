// The relay live-event stream: lease lifecycle, push forwarding, and the
// liveProxy poll cadence it drives.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FORWARDED_SSE_EVENTS, shouldPollSessions, SESSIONS_POLL_STREAM_MS } from "../mock/liveProxy";
import type { LiveEventSink } from "../mock/liveProxy";
import { DESKTOP_EVENTS_METHOD, LEASE_RENEW_MS, relayEventStream, type RelayEventSocket } from "./relayEvents";
import type { RelaySocketStatus } from "./relaySocket";

class FakeSocket implements RelayEventSocket {
  status: RelaySocketStatus = { connected: true, agentOnline: true };
  calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  answer: () => Promise<unknown> = () => Promise.resolve({ leaseMs: 90_000 });
  private push = new Set<(p: Record<string, unknown>) => void>();
  private stat = new Set<(s: RelaySocketStatus) => void>();

  request<T>(method: string, params: Record<string, unknown>): Promise<T> {
    this.calls.push({ method, params });
    return this.answer() as Promise<T>;
  }
  onPush(fn: (p: Record<string, unknown>) => void) {
    this.push.add(fn);
    return () => this.push.delete(fn);
  }
  onStatusChange(fn: (s: RelaySocketStatus) => void) {
    this.stat.add(fn);
    return () => this.stat.delete(fn);
  }
  deliver(p: Record<string, unknown>) {
    for (const fn of this.push) fn(p);
  }
  setStatus(s: RelaySocketStatus) {
    this.status = s;
    for (const fn of this.stat) fn(s);
  }
}

function recorder() {
  const log: string[] = [];
  const events: Array<[string, unknown]> = [];
  const sink: LiveEventSink = {
    event: (n, p) => events.push([n, p]),
    up: () => log.push("up"),
    down: (why) => log.push(`down:${why}`),
  };
  return { sink, log, events };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("relayEventStream", () => {
  it("takes a lease on start, goes up, renews, and forwards desktop_event pushes only", async () => {
    const sock = new FakeSocket();
    const { sink, log, events } = recorder();
    relayEventStream(sock).start(sink);
    await vi.advanceTimersByTimeAsync(0);
    expect(sock.calls).toHaveLength(1);
    expect(sock.calls[0].method).toBe(DESKTOP_EVENTS_METHOD);
    expect(log).toEqual(["up"]);

    sock.deliver({ event: "desktop_event", name: "sessions-frame", data: { kind: "delta", seq: 4 } });
    sock.deliver({ event: "sessions_delta", upsert: [], remove: [] }); // the phone's frame
    expect(events).toEqual([["sessions-frame", { kind: "delta", seq: 4 }]]);

    await vi.advanceTimersByTimeAsync(LEASE_RENEW_MS);
    expect(sock.calls).toHaveLength(2);
    expect(log).toEqual(["up"]);
  });

  it("goes down when the Mac drops offline and resubscribes when it returns", async () => {
    const sock = new FakeSocket();
    const { sink, log } = recorder();
    relayEventStream(sock).start(sink);
    await vi.advanceTimersByTimeAsync(0);
    sock.setStatus({ connected: true, agentOnline: false });
    expect(log).toEqual(["up", "down:relay or Mac offline"]);
    // No renewals while unreachable.
    await vi.advanceTimersByTimeAsync(LEASE_RENEW_MS * 2);
    expect(sock.calls).toHaveLength(1);
    sock.setStatus({ connected: true, agentOnline: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(sock.calls).toHaveLength(2);
    expect(log).toEqual(["up", "down:relay or Mac offline", "up"]);
  });

  it("stays down against a Mac that does not know the method, retrying rarely", async () => {
    const sock = new FakeSocket();
    sock.answer = () => Promise.reject(Object.assign(new Error("unknown method"), { remote: true }));
    const { sink, log } = recorder();
    relayEventStream(sock).start(sink);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(log).toEqual([]);
    expect(sock.calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(sock.calls).toHaveLength(2);
  });

  it("drops to down when a renewal fails, and backs off", async () => {
    const sock = new FakeSocket();
    const { sink, log } = recorder();
    relayEventStream(sock).start(sink);
    await vi.advanceTimersByTimeAsync(0);
    sock.answer = () => Promise.reject(Object.assign(new Error("timeout"), { remote: false }));
    await vi.advanceTimersByTimeAsync(LEASE_RENEW_MS);
    expect(log).toEqual(["up", "down:lease renewal failed"]);
    sock.answer = () => Promise.resolve({ leaseMs: 90_000 });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(log).toEqual(["up", "down:lease renewal failed", "up"]);
  });
});

describe("sessions poll cadence", () => {
  const base = { now: 100_000, lastFrameAt: 0, lastPollAt: 0 };
  it("polls every tick while the relay stream is down", () => {
    expect(shouldPollSessions({ ...base, pollOnly: true, streamUp: false, lastPollAt: 99_000 })).toBe(true);
  });
  it("only polls as a slow safety net while the stream is up", () => {
    const up = { ...base, pollOnly: true, streamUp: true };
    expect(shouldPollSessions({ ...up, lastPollAt: base.now - 3_000 })).toBe(false);
    expect(shouldPollSessions({ ...up, lastPollAt: base.now - SESSIONS_POLL_STREAM_MS })).toBe(true);
  });
  it("does not poll while pushed frames keep arriving", () => {
    const up = { ...base, pollOnly: true, streamUp: true, lastPollAt: 0 };
    expect(shouldPollSessions({ ...up, lastFrameAt: base.now - 5_000 })).toBe(false);
    expect(shouldPollSessions({ ...up, lastFrameAt: base.now - SESSIONS_POLL_STREAM_MS })).toBe(true);
  });
  it("keeps the SSE quiet-window rule for EventSource transports", () => {
    const sse = { ...base, pollOnly: false, streamUp: false };
    expect(shouldPollSessions({ ...sse, lastFrameAt: base.now - 2_000 })).toBe(false);
    expect(shouldPollSessions({ ...sse, lastFrameAt: base.now - 10_000 })).toBe(true);
  });
});

describe("forwarded event names", () => {
  // The Mac only pushes what `relay_events::FORWARDED_EVENTS` lists; an event
  // added to the front end's list but not there would silently never arrive.
  it("match claw-fleet-core's relay_events list", () => {
    const rs = readFileSync(
      resolve(__dirname, "../../../claw-fleet-core/src/relay_events.rs"),
      "utf8",
    );
    const block = rs.match(/pub const FORWARDED_EVENTS: &\[&str\] = &\[([\s\S]*?)\];/);
    expect(block).not.toBeNull();
    const names = [...block![1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect([...names].sort()).toEqual([...FORWARDED_SSE_EVENTS].sort());
  });
});
