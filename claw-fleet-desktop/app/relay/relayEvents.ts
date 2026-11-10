/**
 * Live events over the relay socket — what `/events` SSE is for a browser on
 * `fleet webui`, for a browser that reaches the Mac through the relay.
 *
 * The Mac pushes `{event:"desktop_event", name, data}` payloads carrying the
 * desktop UI's own events (`sessions-frame` deltas, decision-card lifecycle;
 * see `claw-fleet-core/src/relay_events.rs`), but only while this page holds a
 * lease: the relay hands every agent frame to every client in the channel, so
 * the Mac does not spend phones' bandwidth on desktop rows nobody is viewing.
 * The lease is taken with the `desktop_events` relay method and renewed well
 * inside its term.
 *
 * The stream counts as up from the first granted lease until the socket drops,
 * the Mac goes offline, or a renewal fails. liveProxy polls fast while it is
 * down and slowly while it is up.
 */

import type { LiveEventSink, LiveEventStream } from "../mock/liveProxy";
import type { RelaySocketStatus } from "./relaySocket";

export const DESKTOP_EVENTS_METHOD = "desktop_events";
/** Renewal cadence; the Mac's lease is 90s, so two renewals may be lost. */
export const LEASE_RENEW_MS = 30_000;
const SUBSCRIBE_TIMEOUT_MS = 10_000;
/** Retry after a transport failure: 2s, 4s … capped. */
const RETRY_MIN_MS = 2_000;
const RETRY_MAX_MS = 30_000;
/** A Mac that answered "unknown method" predates this; ask again rarely. */
const UNSUPPORTED_RETRY_MS = 120_000;

/** The slice of `RelaySocket` this needs; lets the test drive a fake. */
export interface RelayEventSocket {
  readonly status: RelaySocketStatus;
  request<T>(method: string, params: Record<string, unknown>, timeoutMs?: number): Promise<T>;
  onPush(fn: (payload: Record<string, unknown>) => void): () => void;
  onStatusChange(fn: (s: RelaySocketStatus) => void): () => void;
}

export function relayEventStream(socket: RelayEventSocket): LiveEventStream {
  return {
    start(sink: LiveEventSink) {
      let up = false;
      let inFlight = false;
      let failures = 0;
      let timer: ReturnType<typeof setTimeout> | null = null;

      const clearTimer = () => {
        if (timer != null) clearTimeout(timer);
        timer = null;
      };
      const schedule = (ms: number) => {
        clearTimer();
        timer = setTimeout(() => void subscribe(), ms);
      };
      const goDown = (why: string) => {
        if (!up) return;
        up = false;
        sink.down(why);
      };
      const reachable = () => socket.status.connected && socket.status.agentOnline;

      async function subscribe() {
        clearTimer();
        if (inFlight || !reachable()) return;
        inFlight = true;
        try {
          const reply = await socket.request<{ leaseMs?: number }>(
            DESKTOP_EVENTS_METHOD,
            { supportsGzip: typeof DecompressionStream !== "undefined" },
            SUBSCRIBE_TIMEOUT_MS,
          );
          failures = 0;
          if (!up) {
            up = true;
            sink.up();
          }
          const lease = typeof reply?.leaseMs === "number" ? reply.leaseMs : 90_000;
          schedule(Math.min(LEASE_RENEW_MS, Math.max(1_000, lease / 3)));
        } catch (e) {
          const remote = (e as { remote?: unknown })?.remote === true;
          goDown(remote ? "Mac does not support live events" : "lease renewal failed");
          failures += 1;
          schedule(
            remote
              ? UNSUPPORTED_RETRY_MS
              : Math.min(RETRY_MIN_MS * 2 ** (failures - 1), RETRY_MAX_MS),
          );
        } finally {
          inFlight = false;
        }
      }

      socket.onPush((payload) => {
        if (payload.event !== "desktop_event" || typeof payload.name !== "string") return;
        sink.event(payload.name, payload.data);
      });
      socket.onStatusChange(() => {
        if (reachable()) {
          // A Mac that just came (back) online lost any lease it held.
          if (!up && !inFlight) void subscribe();
        } else {
          clearTimer();
          goDown("relay or Mac offline");
        }
      });
      void subscribe();
    },
  };
}
