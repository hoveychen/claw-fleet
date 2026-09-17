// Connection for one device: set up transport layer, translate its callbacks to
// actions, run polling loops specific to this device. Renders nothing.
//
// Why a **component** instead of a loop in App: hooks cannot be called in a loop
// over an array (device count changes cause misalignment). Rendering one
// `<DeviceConnection key={id}>` per device is the standard React pattern for
// expressing "independent lifecycles" — when the device is removed, the key
// vanishes and its effect cleanup function naturally closes the socket without
// any manual teardown registration.
//
// State does not live here: it dispatches to App's `DeviceStates`
// (deviceRuntime.ts pure reducer). This lets the aggregated view read all
// devices at once, and lets migration rules decouple from React tests.

import { useEffect, useRef } from "react";
import type { TransportFactory } from "./App";
import type { PairedDevice } from "./devices";
import type { HostIdentity } from "./generated/types";
import type { DeviceAction } from "./deviceRuntime";
import {
  connectDelayMs,
  shouldConnect,
  usagePollMs,
  type VisibilityState,
} from "./connectionPolicy";
import { SUPPORTS_PUSH } from "./hostMode";
import { enablePush } from "./push";
import { reconcilePlan } from "./reconcilePlan";
import { loadCachedSessions, saveCachedSessions } from "./sessionCache";
import type { FleetTransport, RttSample } from "./transport";
import type {
  DecisionKind,
  DecisionRequest,
  PendingDecision,
  PendingSnapshot,
  SessionInfo,
  TodayUsage,
} from "./types";

/** The operations surface exposed by one device to the UI. Stored in App's ref
 *  table for actions like "reply to this card" or "drill into this session"
 *  that need to address a specific device. */
export interface DeviceHandle {
  transport: FleetTransport;
  /** Actively pull a fresh authoritative snapshot (after foreground restore or answer). */
  refresh: () => Promise<void>;
}

interface Props {
  device: PairedDevice;
  /** Namespace for session snapshot cache. `null` = use legacy global key
   *  (same-origin mode: single data source); `undefined` = do not touch cache
   *  (mock: fixed data; reading cache would render real data onto fake UI). */
  storageId: string | null | undefined;
  makeTransport: TransportFactory;
  dispatch: (action: DeviceAction & { deviceId: string }) => void;
  /** Register/unregister this device's operations surface on mount/unmount. */
  registerHandle: (deviceId: string, handle: DeviceHandle | null) => void;
  /** Whether this device has pending decision cards now — determines reconciliation
   *  polling cadence (reconcilePlan). */
  hasPendingDecisions: boolean;
  /** Whether the desktop agent is online — gates both polling loops. */
  agentOnline: boolean;
  /** Whether this device is the current-scope device. Affects polling frequency
   *  only (see connectionPolicy.ts). */
  isActive: boolean;
  /** This device's index in the device list, used for staggered connection startup. */
  index: number;
  /** Page visibility. Hidden long enough drops the connection — the background
   *  channel is push notifications, not this socket. */
  visibility: VisibilityState;
  /** Whether this device is currently connected to the relay. Push subscription
   *  writes directly to the socket (no queue, no retransmit), so we must wait
   *  for this to be true before registering — otherwise that frame drops and the
   *  relay never gets the subscription. */
  connected: boolean;
  /** Whether notification permission has been granted at the browser/system level
   *  (per phone). */
  pushGranted: boolean;
  /** Whether the user has muted notifications for **this specific device**. */
  pushMuted: boolean;
  /** The host has reported its identity (hostname + platform). App uses it to
   *  give this device a recognizable name. */
  onHostIdentity: (deviceId: string, identity: HostIdentity) => void;
}

/** Flatten six request kinds from `pending_snapshot` into a sequence of cards. */
function flattenSnapshot(snap: PendingSnapshot, now: number): PendingDecision[] {
  const kinds: Array<[DecisionKind, DecisionRequest[] | undefined]> = [
    ["guard", snap.guard],
    ["elicitation", snap.elicitation],
    ["fleet-ask", snap.fleetAsk],
    ["plan-approval", snap.planApproval],
    ["permission-prompt", snap.permissionPrompt],
    ["a2ui-render", snap.a2uiRender],
  ];
  const out: PendingDecision[] = [];
  for (const [kind, list] of kinds) {
    for (const request of list ?? []) {
      out.push({ kind, id: request.id, request, arrivedAt: now });
    }
  }
  return out;
}

export function DeviceConnection({
  device,
  storageId,
  makeTransport,
  dispatch,
  registerHandle,
  hasPendingDecisions,
  agentOnline,
  isActive,
  index,
  visibility,
  connected,
  pushGranted,
  pushMuted,
  onHostIdentity,
}: Props) {
  const deviceId = device.id;
  const clientRef = useRef<FleetTransport | null>(null);
  // The polling effect reads the current dispatch/registerHandle, which come from
  // App's useCallback and are already stable. Using a ref keeps the effect
  // dependency array minimal to prevent unrelated re-renders from tearing down
  // and reconnecting the socket.
  const dispatchRef = useRef(dispatch);
  dispatchRef.current = dispatch;

  const refreshRef = useRef<() => Promise<void>>(async () => {});
  refreshRef.current = async () => {
    const client = clientRef.current;
    // No longer require `isAuthed`. It means "the stream/socket is open right now",
    // but this request uses a different channel — in same-origin mode it's a
    // plain fetch; the host often responds even if the stream is down. Using it
    // as a gate would close the only path to recover missed cards precisely when
    // we need it most (see reconcilePlan).
    if (!client) return;
    try {
      const snap = await client.request<PendingSnapshot>("pending_snapshot");
      dispatchRef.current({
        deviceId,
        type: "snapshot",
        fresh: flattenSnapshot(snap, Date.now()),
        agent: snap.agent,
        now: Date.now(),
      });
      // The host answered this request, so it is online. The reverse is not true:
      // a single request failure is not enough to mark it dead. The "offline"
      // signal is still owned by the stream/socket itself.
      dispatchRef.current({ deviceId, type: "agentOnline", online: true });
    } catch {
      // Desktop offline — real-time events or next polling round will catch up
    }
  };

  // Transport layer lifecycle. Dependency array includes only "which device,
  // which relay" plus the visibility-policy computed gate — everything else
  // changing should not tear down and reconnect the socket.
  const connectAllowed = shouldConnect(visibility, Date.now());
  useEffect(() => {
    if (!connectAllowed) return;
    const d = (a: DeviceAction) => dispatchRef.current({ ...a, deviceId });
    const client = makeTransport(device, {
      onStatus: (connected) => d({ type: "status", connected }),
      onAgentOnline: (online) => {
        d({ type: "agentOnline", online });
        if (online) void refreshRef.current();
      },
      onDecisionCreated: (kind, request) =>
        d({
          type: "decisionCreated",
          kind,
          request: request as DecisionRequest,
          now: Date.now(),
        }),
      onDecisionResolved: (_kind, id) => d({ type: "decisionResolved", id }),
      onSessions: (list: SessionInfo[]) => {
        d({ type: "sessions", list });
        // The persisted copy is what cold startup sees first. Both full and
        // incremental frames have already been merged by this point
        // (see relay.ts), so the cache always contains a complete snapshot.
        if (storageId !== undefined) saveCachedSessions(storageId, list);
      },
      onSessionsKind: (kind) => d({ type: "sessionsKind", kind }),
      onRttSample: (sample: RttSample) => d({ type: "rtt", sample }),
      onReconnect: () => d({ type: "reconnect", now: Date.now() }),
      onAuthError: (message) => d({ type: "authError", message }),
    });
    clientRef.current = client;
    d({ type: "attach" });
    // Stagger connections: N simultaneous handshakes bunch up when network recovers.
    const startAt = window.setTimeout(() => client.connect(), connectDelayMs(index));
    registerHandle(deviceId, { transport: client, refresh: () => refreshRef.current() });
    // Mobile browsers may never run React cleanup (user closes tab directly), so
    // pagehide also signals "I'm leaving" to avoid making the desktop wait for
    // timeout to remove this device.
    const onPageHide = () => client.sayGoodbye();
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.clearTimeout(startAt);
      window.removeEventListener("pagehide", onPageHide);
      registerHandle(deviceId, null);
      clientRef.current = null;
      client.close();
    };
    // Include `device` in full in dependencies: any change to key, relay, baseUrl,
    // or token should reconnect. devices.ts produces a new object on every edit
    // (pure function layer), so reference equality precisely means "this device's
    // connection params changed".
  }, [
    deviceId,
    storageId,
    device,
    makeTransport,
    registerHandle,
    connectAllowed,
    index,
  ]);

  // Cold start renders cached task list first to avoid blank page while socket
  // is still handshaking.
  useEffect(() => {
    if (storageId === undefined) return;
    let cancelled = false;
    void loadCachedSessions(storageId).then((cached) => {
      if (cancelled || !cached?.length) return;
      dispatchRef.current({ deviceId, type: "cachedSessions", list: cached });
    });
    return () => {
      cancelled = true;
    };
  }, [deviceId, storageId]);

  // What is the host called. One request is enough — the hostname does not
  // change during a session, and it only serves to name this device record in
  // the device list (devices.ts::applyHostIdentity).
  //
  // Gate is `agentOnline` not `connected`: reaching the relay only means this
  // socket works; the desktop answers the method. Old desktops don't know this
  // method, so it stays "Device N" — a cosmetic name not worth erroring in UI.
  const identityAskedRef = useRef(false);
  useEffect(() => {
    if (!agentOnline || identityAskedRef.current) return;
    const client = clientRef.current;
    if (!client) return;
    identityAskedRef.current = true;
    let cancelled = false;
    void client
      .request<HostIdentity>("host_identity")
      .then((identity) => {
        if (!cancelled && identity) onHostIdentity(deviceId, identity);
      })
      .catch(() => {
        // Old desktop doesn't have this method — try again on next mount, no retry
        // or error needed
        identityAskedRef.current = false;
      });
    return () => {
      cancelled = true;
    };
  }, [agentOnline, deviceId, onHostIdentity]);

  // Today's usage. Poll only when desktop is online; desktop computes the number,
  // phone just displays it.
  useEffect(() => {
    if (!agentOnline) return;
    const intervalMs = usagePollMs(isActive);
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      const client = clientRef.current;
      // Skip this round when page is hidden (but still schedule next): querying
      // the display-only number in the background is just wasted power and bandwidth.
      if (client?.isAuthed && document.visibilityState === "visible") {
        try {
          const usage = await client.request<TodayUsage>("today_usage");
          if (!cancelled) dispatchRef.current({ deviceId, type: "usage", usage });
        } catch {
          /* Transient failure — keep previous value */
        }
      }
      if (!cancelled) timer = window.setTimeout(poll, intervalMs);
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentOnline, deviceId, isActive]);

  // Foreground fallback reconciliation for pending decision cards.
  // decision_created/decision_resolved are unacked, unreliable broadcasts; a
  // dropped frame on weak networks loses a card; refresh on (re)connect may
  // timeout. This loop is the durable fallback: fast (3s) when cards are
  // pending, slow (15s) when idle, slower (30s) when offline. Cadence is
  // determined by reconcilePlan — the offline tier is the antidote to "refresh
  // the page" on a desktop-as-server scenario.
  useEffect(() => {
    const plan = reconcilePlan(agentOnline, hasPendingDecisions);
    if (!plan.poll) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      if (document.visibilityState === "visible") await refreshRef.current();
      if (!cancelled) timer = window.setTimeout(tick, plan.intervalMs);
    };
    timer = window.setTimeout(tick, plan.intervalMs);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentOnline, hasPendingDecisions]);

  // Push subscription registered **per device**: relay subscriptions are per
  // channel file, so to receive from N devices one phone must register in N
  // channels (same browser endpoint).
  //
  // `connected` is a dependency, not incidental: pushSubscribe writes directly
  // to socket; returns false and doesn't queue/retry if socket is not OPEN.
  // Historically this effect only depended on [push, optedOut], so registration
  // happened before handshake — phone reported itself subscribed, relay's
  // subscription DB was empty.
  //
  // enablePush is idempotent: reuse existing subscription, requestPermission
  // returns immediately when already granted.
  useEffect(() => {
    if (!SUPPORTS_PUSH) return;
    if (!connected || !pushGranted || pushMuted) return;
    // HTTP direct-connect transport has no push channel (pushSubscribe always
    // returns false), so subscription has nowhere to register — don't ask for
    // VAPID public key, that request only wastes a trip.
    if (device.kind !== "relay") return;
    const client = clientRef.current;
    if (!client) return;
    void enablePush(client, device.relayBase, deviceId);
  }, [connected, pushGranted, pushMuted, deviceId, device]);

  // Refresh when returning to foreground: mobile browsers freeze background tab
  // sockets; reconnect happens but the snapshot at that moment may already be stale.
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") void refreshRef.current();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, []);

  return null;
}
