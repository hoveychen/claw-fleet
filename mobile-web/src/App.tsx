import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { Inbox, ListChecks, MoreHorizontal, Package, Plus, X } from "lucide-react";
import styles from "./App.module.css";
import {
  disablePush,
  enablePush,
  isPushMuted,
  setPushMuted as setPushMutedStored,
  setPushOptedOut,
  pushState,
  unsubscribeChannel,
  type PushState,
} from "./push";
// This module is transport-agnostic. main.tsx decides which transport to instantiate
// based on the build mode and injects it here. The selection relies on dynamic imports
// so that same-origin builds can tree-shake the relay branch with its entire dependency
// tree; if App statically imported RelayClient, that would be impossible.

import type { FleetTransport, TransportHandlers } from "./transport";
import { NEEDS_PAIRING, SUPPORTS_PUSH } from "./hostMode";
import { formatRttSplit } from "./connQuality";
import { ConnIcon, connIconKind } from "./views/ConnIcon";
import { DeviceConnection, type DeviceHandle } from "./DeviceConnection";
import { HIDDEN_DISCONNECT_MS, type VisibilityState } from "./connectionPolicy";
import {
  aggregateDecisions,
  aggregateSessions,
  allDecisionsLoaded,
  anyAgentOnline,
  anyConnected,
  anySessionsLoaded,
  itemKey,
  devicesReducer,
  emptyDeviceState,
  offlineDeviceCount,
  totalUsage,
  usageByDevice,
  worstCongestion,
  type DeviceStates,
  type WithDevice,
} from "./deviceRuntime";
// Import only the zero-dependency switch here. The `?mock` mock client extends
// RelayClient; importing it here would statically drag the entire relay dependency
// tree into same-origin builds — that logic belongs in transportRelay.ts instead.

import { isMockMode } from "./mockMode";
import type { RepoSummary, SessionInfo, WikiDoc } from "./types";
import type { HostIdentity } from "./generated/types";
import { randomId } from "./clientId";
import { needsA2hsForDurableStorage } from "./secretStore";
import {
  activeDevice,
  addPendingUnsub,
  applyHostIdentity,
  adoptScannedDevice,
  clearBook,
  consumeHashPairing,
  dropPendingUnsub,
  emptyBook,
  loadBookFromIdb,
  loadBookSync,
  loadPendingUnsub,
  nextDeviceLabel,
  persistBook,
  removeDevice,
  renameDevice,
  setActiveDevice,
  type DeviceBook,
  type PairedDevice,
} from "./devices";
import { onPairingLink } from "./deepLink";
import type { PairedLink } from "./pairingLink";
import { PairPasteForm } from "./views/PairPasteForm";
import { PairScanner } from "./views/PairScanner";
import { scanAvailability } from "./scanAvailability";
import { clearCachedSessions } from "./sessionCache";
import { AUTH_WAIT_MS, waitAuthed } from "./transportWait";
import { DeviceScopeProvider, scopedKey } from "./deviceScope";
import { ErrorBoundary } from "./ErrorBoundary";
import { t as translate, useI18n } from "./i18n";
import { ExitGuard, installUnloadPrompt } from "./exitGuard";
import { HistoryLayer, setRootBackHandler } from "./useNavStack";
import { NEW_SESSION_DRAFT_KEY, NewSessionSheet } from "./views/Composer";
import { clearDraftsByPrefix, loadDraft, saveDraft } from "./draft";
import { onShareReceived, shareToPrompt, sharedFilesToFiles } from "./shareTarget";
import { onNativePushToken } from "./nativePush";
import { onDecisionDeepLink } from "./decisionDeepLink";
import { DecisionsView } from "./views/DecisionsView";
import { DecisionDrawer } from "./views/DecisionDrawer";
import { MoreView } from "./views/MoreView";
import { DeviceSwitcher } from "./views/DeviceSwitcher";
import { PlansView } from "./views/PlansView";
import { ArtifactsView } from "./views/ArtifactsView";
import { RepoView } from "./views/RepoView";
import { RepoDetailView } from "./views/RepoDetailView";
import { TerminalView, type TerminalWorkspace } from "./views/TerminalView";
import { useHostFeatures } from "./useHostFeatures";
import { SessionDetailView } from "./views/SessionDetailView";
import { sessionDetailKey } from "./sessionDetailKey";
import { TasksView } from "./views/TasksView";
import { UsageView } from "./views/UsageView";
import { WikiView } from "./views/WikiView";
import { WikiDocView } from "./views/WikiDocView";

const A2HS_DISMISSED_KEY = "fleet-a2hs-dismissed";
/** The PushState when the notification banner was last dismissed, not a boolean.
    The reason we store state instead of a flag is explained in the banner rendering logic. */
const PUSH_NOTICE_DISMISSED_KEY = "fleet-push-notice-dismissed";

/** Compact token count: 1.2M / 34.5K / 780. */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

type Tab = "decisions" | "tasks" | "artifacts" | "more";

/** Display name of each tab. Used only for error boundary fallback titles and console
 *  logs (bottom navigation text is still inlined in each button). Having "decisions page
 *  failed to display" is more useful than "decisions page failed to display". */
const TAB_LABEL: Record<Tab, string> = {
  decisions: "决策",
  tasks: "任务",
  artifacts: "产出",
  more: "更多",
};

/** Default fields for a newly paired device. The default name goes through i18n, so this
 *  function lives here rather than in devices.ts — that module intentionally avoids i18n
 *  to remain a testable pure function. */
function newDeviceMint(book: DeviceBook): { id: string; label: string; now: number } {
  return { id: randomId(), label: nextDeviceLabel(book, translate("设备")), now: Date.now() };
}

// `?mock` runs the whole app on fixtures (no relay, no pairing) — used by the
// promo screen-recording pipeline and for quick UI work in a plain browser.
const MOCK = isMockMode();

/** Factory to instantiate the appropriate transport layer for this deployment.
    Injected by main.tsx based on build mode. */
export type TransportFactory = (
  /** The device to connect to. We pass the whole device record rather than scattered
   *  parameters because all the routing details — relay vs HTTP direct, relay address,
   *  baseUrl, token — are properties that each device stores about itself (devices.ts). */
  device: PairedDevice,
  handlers: TransportHandlers,
) => FleetTransport;

/** Mock and same-origin deployments have no pairing, but everything downstream works in
 *  units of "one device". We synthesize a device for each so the entire chain has a
 *  uniform shape — no need to split logic in every view. */
const MOCK_DEVICE: PairedDevice = {
  kind: "relay",
  id: "mock",
  label: "Mock",
  secret: "mock-secret",
  relayBase: null,
  addedAt: 0,
};
/** Same-origin deployment device. It is an HTTP device with an empty baseUrl, which
 *  means "talk to the same origin that served this page". This way the "route transport
 *  by device kind" rule naturally covers same-origin deployments without a special case. */
const SAME_ORIGIN_DEVICE: PairedDevice = {
  kind: "http",
  id: "same-origin",
  label: "",
  baseUrl: "",
  token: null,
  addedAt: 0,
};

/** State for a device that has not yet received any frame. Module-level constant:
 *  creating a new object on every render would cause all the destructured arrays below
 *  to change reference on every render. */
const EMPTY_DEVICE_STATE = emptyDeviceState();

export function App({ makeTransport }: { makeTransport: TransportFactory }) {
  // Subscribe to language changes. Re-rendering the App root propagates the change
  // through the entire tree (there's no React.memo), and all t() calls recalculate.

  const { t } = useI18n();
  // Record of every Fleet device this phone has paired with (devices.ts).
  // Mock and same-origin builds have no pairing, so the book stays empty and the
  // `secret` below gets a placeholder string instead.

  const [book, setBook] = useState<DeviceBook>(() => {
    if (MOCK || !NEEDS_PAIRING) return emptyBook();
    const stored = loadBookSync(newDeviceMint(emptyBook()));
    // Check if this page load came with a pairing fragment (`#k=`).

    const scanned = consumeHashPairing();
    if (!scanned) return stored;
    return adoptScannedDevice(
      stored,
      scanned.secret,
      newDeviceMint(stored),
      scanned.relayBase,
      {
        // Shell boot re-injection should not steal focus: the device the user last
        // switched to should not be reset every time the app reopens.

        focus: !scanned.boot,
      },
    ).book;
  });
  // Pairing secret for the current scoped device. `?mock` stands in for a pairing
  // secret so the gate below opens and the effect that builds the client runs — it
  // just builds a MockRelayClient. Same-origin has no pairing (the backend is the
  // process that served this page), so that gate doesn't exist at all; we just give
  // a placeholder string to let the connection-building effect proceed.

  const current = NEEDS_PAIRING && !MOCK ? activeDevice(book) : null;
  /** Pairing gate guard: show the onboarding page only if pairing is needed but we have no devices. */
  const paired = !NEEDS_PAIRING || MOCK || current !== null;
  /** Relay address specified by the current device (relay devices only); push uses this to fetch the VAPID key. */
  const relayBase = current?.kind === "relay" ? current.relayBase : null;
  // Local storage namespace. Session snapshots, drafts, attachments, and workspace
  // memories are segmented by device — that content only makes sense for one machine
  // (see deviceScope.tsx). Mock and same-origin don't segment (they have one data
  // source; adding a prefix would orphan existing user drafts).

  const deviceId = current?.id ?? null;

  // Runtime view of devices: pairing mode = the device book, mock/same-origin = one synthesized device.

  const runtimeDevices = useMemo(
    () => (MOCK ? [MOCK_DEVICE] : NEEDS_PAIRING ? book.devices : [SAME_ORIGIN_DEVICE]),
    [book.devices],
  );
  const deviceOrder = useMemo(() => runtimeDevices.map((d) => d.id), [runtimeDevices]);
  /** Runtime ID of the currently scoped device. */
  const activeDeviceId =
    MOCK || !NEEDS_PAIRING ? runtimeDevices[0].id : (deviceId ?? "");
  // The share menu effect has empty dependencies (subscribe only once on mount), but
  // when it fires it needs the **current** device, so we read from a ref there instead
  // of relying on a closure value.

  const deviceIdRef = useRef<string | null>(deviceId);
  deviceIdRef.current = deviceId;
  // Similarly: the clear-all-pairing callback has empty dependencies, but it needs to
  // iterate over the **current** book at runtime.

  const bookRef = useRef(book);
  bookRef.current = book;
  // null = still probing IndexedDB; only after that fails do we show the gate.
  const [idbProbed, setIdbProbed] = useState(false);
  // Scanner viewfinder in the pairing gate (native shell only).

  const [scanning, setScanning] = useState(false);
  const [a2hsDismissed, setA2hsDismissed] = useState(
    () => localStorage.getItem(A2HS_DISMISSED_KEY) === "1",
  );
  /** Which PushState the notification banner had the last time it was dismissed (`null` = never dismissed). */
  const [pushNoticeDismissed, setPushNoticeDismissed] = useState<string | null>(() =>
    localStorage.getItem(PUSH_NOTICE_DISMISSED_KEY),
  );
  const dismissPushNotice = useCallback((state: PushState) => {
    localStorage.setItem(PUSH_NOTICE_DISMISSED_KEY, state);
    setPushNoticeDismissed(state);
  }, []);

  // localStorage may be wiped (iOS 7-day eviction, cache clear) but the IDB copy may
  // have survived — try to re-hydrate before declaring the pairing lost.
  useEffect(() => {
    if (paired) {
      setIdbProbed(true);
      return;
    }
    let cancelled = false;
    loadBookFromIdb(newDeviceMint(emptyBook())).then((recovered) => {
      if (cancelled) return;
      if (recovered) {
        // Write the recovered copy back to both storage so cold starts next time don't
        // need to fall back to the recovery path again.

        persistBook(recovered);
        setBook(recovered);
      }
      setIdbProbed(true);
    });
    return () => {
      cancelled = true;
    };
  }, [paired]);

  /** Pairing completion callback. Handles both App Link deliveries and manual paste inputs
   *  through a single entry point like PWA's `#k=` path: deduplication, preserving user
   *  edits to names, and focusing the newly paired device all have one implementation
   *  (devices.ts::adoptScannedDevice).
   *
   *  relayBase must be passed along — the shell's page origin is `capacitor://localhost`,
   *  so losing it means we can only connect to the relay baked in at build time; custom
   *  relay hosts become unreachable. */
  const adoptPaired = useCallback((paired: PairedLink) => {
    setBook(
      (prev) => adoptScannedDevice(prev, paired.secret, newDeviceMint(prev), paired.relayBase).book,
    );
    setIdbProbed(true);
  }, []);

  // Native shell only: the app boots from bundled assets, so there is no URL to
  // read the pairing secret from. Universal Links / App Links deliver the
  // scanned pairing URL here instead. No-op in the browser/PWA.
  useEffect(() => onPairingLink(adoptPaired), [adoptPaired]);

  // ── Device management (the "More" page section) ─────────────────────────────────────
  //
  // All three actions follow the pattern: update book + persist. Persist immediately
  // after state updates to ensure unexpected exits don't make users think their changes
  // were lost.


  /** Clear all pairings. Unsubscribe requests for each device are queued for later
   *  (not sent immediately here): we're about to reload and can't wait for N temporary
   *  connections to handshake. The retry effect on next boot will complete them. */
  const unpairAll = useCallback(() => {
    const now = Date.now();
    for (const d of bookRef.current.devices) {
      // Only relay devices have a push channel to unsubscribe from; HTTP direct
      // connections don't have a push channel at all.

      if (SUPPORTS_PUSH && d.kind === "relay") {
        addPendingUnsub({ secret: d.secret, relayBase: d.relayBase, at: now });
      }
      clearCachedSessions(d.id);
      clearDraftsByPrefix(scopedKey(d.id, ""));
    }
    clearBook();
    location.reload();
  }, []);

  const switchDevice = useCallback((id: string) => {
    setBook((prev) => {
      const next = setActiveDevice(prev, id);
      persistBook(next);
      return next;
    });
  }, []);

  /** Desktop host reported its hostname — replace "Device 2" with "Harrys-MacBook-Pro".
   *
   *  User-renamed devices won't be overwritten (applyHostIdentity only updates auto-generated
   *  names), so no extra logic needed here; just persist. When the name doesn't change,
   *  applyHostIdentity returns the same object, so setBook won't trigger a re-render or
   *  waste a storage write. */
  const adoptHostIdentity = useCallback((id: string, identity: HostIdentity) => {
    setBook((prev) => {
      const next = applyHostIdentity(prev, id, identity);
      if (next !== prev) persistBook(next);
      return next;
    });
  }, []);

  const renameDeviceLabel = useCallback((id: string, label: string) => {
    setBook((prev) => {
      const next = renameDevice(prev, id, label);
      persistBook(next);
      return next;
    });
  }, []);

  /** Stop push notifications on a device's relay channel.
   *
   *  The device might not be the one currently connected, so we open a temporary
   *  connection for it. We can't just do a "local unsubscribe" instead: the browser's
   *  push subscription is shared across all devices, so canceling it would also disable
   *  notifications for other devices (see push.ts::unsubscribeChannel).
   *
   *  Returns whether unsubscription succeeded. Even if it fails, we shouldn't block the
   *  user from removing the device (that's their explicit intent) — the caller will queue
   *  it for retry on next boot. */
  const stopPushFor = useCallback(
    async (device: PairedDevice): Promise<boolean> => {
      // HTTP direct devices have no push channel (pushSubscribe always returns false),
      // so there's nothing to unsubscribe from — just return success to avoid wasting
      // time on a temporary connection handshake.
      if (device.kind !== "relay") return true;
      // If this device is currently connected, reuse its connection.

      const live = handlesRef.current[device.id]?.transport;
      if (live?.isAuthed) return unsubscribeChannel(live);
      const temp = makeTransport(device, {});
      try {
        temp.connect();
        if (!(await waitAuthed(temp, AUTH_WAIT_MS))) return false;
        return await unsubscribeChannel(temp);
      } catch {
        return false;
      } finally {
        temp.close();
      }
    },
    [makeTransport],
  );

  const removeDeviceEntry = useCallback(
    async (device: PairedDevice) => {
      // Unsubscribe first, then update the book: if we reversed it, retry logic wouldn't
      // have the secret (the queued-retry path still needs it).

      const unsubscribed = SUPPORTS_PUSH ? await stopPushFor(device) : true;
      if (!unsubscribed && device.kind === "relay") {
        addPendingUnsub({ secret: device.secret, relayBase: device.relayBase, at: Date.now() });
      }
      // Clear all local traces for this device: session snapshot cache + all drafts
      // in its namespace (new session form, attachment paths, last-used repo, session
      // partial inputs).

      clearCachedSessions(device.id);
      clearDraftsByPrefix(scopedKey(device.id, ""));
      setBook((prev) => {
        const next = removeDevice(prev, device.id);
        persistBook(next);
        return next;
      });
    },
    [stopPushFor],
  );

  /** Retry queued unsubscriptions from last session. Failed ones stay queued for next
   *  boot (they expire after 7 days, see devices.ts). */
  useEffect(() => {
    if (!SUPPORTS_PUSH || MOCK) return;
    const pending = loadPendingUnsub(Date.now());
    if (pending.length === 0) return;
    let cancelled = false;
    void (async () => {
      for (const entry of pending) {
        if (cancelled) return;
        // Queued unsubscriptions only exist for relay devices (HTTP direct has no
        // channel), so we synthesize a relay device to connect with — it lives only
        // for this one request.

        const temp = makeTransport(
          {
            kind: "relay",
            id: "pending-unsub",
            label: "",
            secret: entry.secret,
            relayBase: entry.relayBase,
            addedAt: 0,
          },
          {},
        );
        try {
          temp.connect();
          if (await waitAuthed(temp, AUTH_WAIT_MS)) {
            if (await unsubscribeChannel(temp)) dropPendingUnsub(entry.secret, Date.now());
          }
        } catch {
          // Leave failed unsubscriptions for next boot's retry.
        } finally {

          temp.close();
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [makeTransport]);

  // Android share sheet → new-session composer. Seed the draft *before* opening
  // the sheet: NewSessionSheet reads it once on mount via useDraft, so writing
  // after would be ignored. Merging (rather than replacing) keeps whatever
  // workspace/model the user last picked.
  useEffect(() => {
    return onShareReceived((share) => {
      void (async () => {
        const files = await sharedFilesToFiles(share.files);
        // Files that became real attachments don't need naming in the prose —
        // the chips already show them. Only the ones we failed to read stay in
        // the text, so the user still knows something came along.
        const fetched = new Set(files.map((f) => f.name));
        const missed = share.files.filter((f) => !fetched.has(f.name));
        const prompt = shareToPrompt({ ...share, files: missed });
        // 草稿按设备分家，否则从分享菜单塞进来的提示词会落进另一台的新会话表单。
        const key = scopedKey(deviceIdRef.current, NEW_SESSION_DRAFT_KEY);
        const existing = loadDraft<Record<string, unknown>>(key, {});
        saveDraft(key, { ...existing, prompt });
        setSharedFiles(files);
        setShowNewSession(true);
      })();
    });
  }, []);

  const [tab, setTab] = useState<Tab>("decisions");
  /** Whether the device switcher dropdown in the header is open. */
  const [deviceMenuOpen, setDeviceMenuOpen] = useState(false);
  /// Decision card to focus when notification is clicked. The nonce ensures clicking
  /// the same card twice still triggers a focus; deviceId is reverse-looked-up from the
  /// source mark in the notification (undefined for old relay versions without marks).

  const [focusDecision, setFocusDecision] = useState<{
    id: string;
    deviceId?: string;
    nonce: number;
  } | null>(null);

  // ── Per-device runtime ─────────────────────────────────────────────────────────
  //
  // This used to be flat useState: one connected, one sessions, one decisions. That
  // locked in "single data source" at the component level. Now each device gets its
  // own copy (pure reducer in deviceRuntime.ts), with each device's connection handled
  // by its own <DeviceConnection>.

  const [states, dispatch] = useReducer(devicesReducer, {} as DeviceStates);
  // Control surface for each device (transport + active refresh). We use state, not
  // ref: the UI needs to re-render the instant a connection establishes so it can
  // pass the transport to downstream views.

  const [handles, setHandles] = useState<Record<string, DeviceHandle>>({});
  const registerHandle = useCallback((id: string, handle: DeviceHandle | null) => {
    setHandles((prev) => {
      if (!handle) {
        if (!(id in prev)) return prev;
        const next = { ...prev };
        delete next[id];
        return next;
      }
      return { ...prev, [id]: handle };
    });
  }, []);

  // Callbacks with empty dependencies like stopPushFor / unpairAll need to read the
  // **current** connection table.
  const handlesRef = useRef(handles);
  handlesRef.current = handles;

  /** Transport for a specific device. Items from the merged list that you drill into
   *  belong to **that device**, not the currently scoped one — grabbing the wrong one
   *  sends a request to the wrong place. */

  const transportFor = useCallback(
    (id: string): FleetTransport | null => handles[id]?.transport ?? null,
    [handles],
  );

  const activeState = states[activeDeviceId] ?? EMPTY_DEVICE_STATE;
  const client = handles[activeDeviceId]?.transport ?? null;
  // Which optional features the current desktop host has enabled. This is a **per-device**
  // query: two desktop instances might have FLEET_TERMINAL configured differently, so
  // switching devices requires re-querying instead of caching globally.

  const hostFeatures = useHostFeatures(client);
  const {
    sessionsFrame,
    rttSplit,
    snapshotSources,
    authError,
  } = activeState;
  // First-screen "loading tasks" gate: once any device delivers the first frame, we
  // stop showing the loading state.
  const sessionsLoaded = anySessionsLoaded(states, deviceOrder);
  // Decision cards are **merged**: all devices' cards are sorted by arrival time with
  // each card marked with its source device. This is the core value of multi-device:
  // one inbox instead of "remember to check the other device".
  const decisions = useMemo(
    () => aggregateDecisions(states, deviceOrder),
    [states, deviceOrder],
  );
  // Skeleton screens only render for devices that are **online but haven't delivered
  // their first snapshot yet**. Offline devices don't block (their snapshot never comes).

  const decisionsLoaded = allDecisionsLoaded(states, deviceOrder);
  // Don't report this count for single-device setups: in that case "desktop offline"
  // is the terminal state of the entire page; reporting it again is just noise.
  const offlineDevices =
    runtimeDevices.length > 1 ? offlineDeviceCount(states, deviceOrder) : 0;
  /** Device badges on cards/lists. Returns null for single-device setups — single-device
   *  users shouldn't pay the visual cost of multi-device features. */

  const deviceLabelOf = useCallback(
    (id: string): string | null => {
      if (runtimeDevices.length <= 1) return null;
      return runtimeDevices.find((d) => d.id === id)?.label ?? null;
    },
    [runtimeDevices],
  );
  /** The indicator light on each line of the device switcher. It shows **this device's**
   *  connectivity, opposite from the header's "best of all" light — the switcher's purpose
   *  is to let users see which device dropped. */
  const deviceStatusOf = useCallback(
    (id: string) => {
      const s = states[id];
      return s ? { connected: s.connected, agentOnline: s.agentOnline } : undefined;
    },
    [states],
  );
  // The three values in the header show the **overall** state: one device being offline
  // shouldn't show offline for the whole page, and perceived congestion is the worst
  // connection. Cost is the sum of all devices for the day.

  const connected = anyConnected(states, deviceOrder);
  const agentOnline = anyAgentOnline(states, deviceOrder);
  const congestion = worstCongestion(states, deviceOrder);
  const todayUsage = totalUsage(states, deviceOrder);
  /** Per-device cost summaries (the rows that expand in the usage page). Labels come
   *  from the device book, same as the device switcher. */
  const usageRows = useMemo(
    () =>
      usageByDevice(states, deviceOrder).map((r) => ({
        ...r,
        label: runtimeDevices.find((d) => d.id === r.id)?.label ?? r.id,
      })),
    [states, deviceOrder, runtimeDevices],
  );
  // Header connection icon: shape and brightness determined by kind. The original text
  // is downgraded to title + aria-label (still accessible to screen readers and long-press,
  // just not taking up layout space).

  const connKind = connIconKind(connected, agentOnline, congestion);
  const connText =
    connKind === "connecting"
      ? t("连接中…")
      : connKind === "desktop-offline"
        ? t("桌面端离线")
        : connKind === "congested"
          ? t("在线 · 网络拥挤")
          : connKind === "fair"
            ? t("在线 · 网络一般")
            : t("桌面端在线");

  const [push, setPush] = useState<PushState>(pushState);
  // Sub-state below "granted": the user can turn notifications off even while
  // the browser permission stays granted. Persisted so it survives reloads.
  // Muting is per-device. The master toggle checks "are all devices muted?" —
  // if only one device is muted, the banner shouldn't say "notifications off" because
  // that would make users think the other device is also silent.

  const [pushMuted, setPushMuted] = useState<Record<string, boolean>>({});
  useEffect(() => {
    setPushMuted((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const d of runtimeDevices) {
        if (!(d.id in next)) {
          next[d.id] = isPushMuted(d.id);
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [runtimeDevices]);
  const pushOptedOut =
    runtimeDevices.length > 0 && runtimeDevices.every((d) => pushMuted[d.id] === true);
  // Session detail is a drill-down chain, not a single page: clicking "open subagent" from
  // an Agent card stacks another layer (same stack-based overlay model as wikiStack).
  // The top ID defines the current detail. Each layer carries its device: session IDs are
  // unique within a device, and drilling down needs that device's transport to fetch the
  // tail (see itemKey's explanation).
  const [detailStack, setDetailStack] = useState<Array<{ deviceId: string; id: string }>>([]);
  // Wiki documents are a chain, not a single page: `[[slug]]` links stack another doc,
  // back button pops it.

  const [wikiStack, setWikiStack] = useState<Array<{ deviceId: string; doc: WikiDoc }>>([]);
  const [showRepo, setShowRepo] = useState(false);
  const [repoDetail, setRepoDetail] = useState<{ deviceId: string; repo: RepoSummary } | null>(
    null,
  );
  const [showUsage, setShowUsage] = useState(false);
  // Terminal page state. `{ workspace: null }` = open but directory not yet chosen (e.g.,
  // entering from tasks page when "all workspaces" filter is active). `null` = not open.
  // These must be distinct: otherwise "open then return to select workspace" would look like
  // not opened.

  const [terminal, setTerminal] = useState<{ workspace: TerminalWorkspace | null } | null>(null);
  const [showPlans, setShowPlans] = useState(false);
  const [showWiki, setShowWiki] = useState(false);
  const [showNewSession, setShowNewSession] = useState(false);
  // Which device the new session opens on. null = follow the current scoped device
  // (reset on every sheet open), unless the user explicitly picks a different device
  // in the sheet. We don't reuse activeDeviceId because they mean different things:
  // scope = "which device am I viewing", this = "which device should this session open
  // on" — opening a session in the cloud shouldn't force the user to switch the whole
  // interface to that device first.

  const [newSessionDeviceId, setNewSessionDeviceId] = useState<string | null>(null);
  // Files handed over by another app's share, pending upload once the
  // new-session sheet mounts (that's where the attachment state lives).
  const [sharedFiles, setSharedFiles] = useState<File[]>([]);
  const [exitArmed, setExitArmed] = useState(false);

  // Back at the stack base (home tab, no overlay): first press shows "press again to exit",
  // second press actually exits. beforeunload only covers refresh/close/address bar navigation,
  // not back — mock mode skips this to avoid stalling the screen recording pipeline with
  // native dialogs.

  useEffect(() => {
    const install = () => (MOCK ? () => {} : installUnloadPrompt());
    let uninstall = install();
    let timer: number | undefined;
    const guard = new ExitGuard(setExitArmed, () => {
      uninstall();
      // 万一没走成（本页就是历史里的第一条，退无可退），把兜底装回来。
      timer = window.setTimeout(() => {
        uninstall = install();
      }, 1_000);
    });
    setRootBackHandler(guard.handleRootBack);
    return () => {
      if (timer !== undefined) window.clearTimeout(timer);
      uninstall();
      setRootBackHandler(() => "leave");
    };
  }, []);

  /** Card just answered on this device: optimistically remove it and record a timestamp
   *  to suppress late snapshots from reviving it (see answered/snapshot actions in
   *  deviceRuntime.ts). Which device gets the answer is specified by the caller — the
   *  inbox is merged, so the card might not belong to the currently scoped device. */

  const markAnswered = useCallback((deviceId: string, id: string) => {
    dispatch({ deviceId, type: "answered", id, now: Date.now() });
  }, []);

  // Page visibility. After multi-device support, this is a **connection policy** input,
  // not just a trigger for a one-time refetch: hide long enough and we drop all N sockets
  // (background delivery uses push, not this socket). Coming back to the foreground will
  // stagger reconnections. See connectionPolicy.ts.

  const [visibility, setVisibility] = useState<VisibilityState>(() => ({
    visible: typeof document === "undefined" || document.visibilityState === "visible",
    hiddenSince: Date.now(),
  }));
  useEffect(() => {
    const onChange = () => {
      const visible = document.visibilityState === "visible";
      setVisibility((prev) =>
        prev.visible === visible ? prev : { visible, hiddenSince: Date.now() },
      );
    };
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);
  // After the page goes hidden, once the grace period expires we re-evaluate (otherwise
  // "should disconnect now" would never trigger).

  useEffect(() => {
    if (visibility.visible) return;
    const left = HIDDEN_DISCONNECT_MS - (Date.now() - visibility.hiddenSince);
    const timer = window.setTimeout(
      () => setVisibility((prev) => ({ ...prev })),
      Math.max(left, 0) + 100,
    );
    return () => window.clearTimeout(timer);
  }, [visibility]);

  // Recalculate push state when returning to foreground or regaining focus: Notification.permission
  // might have changed in the background (user toggled it in system settings), but the banner
  // only read pushState() once at mount. Snapshot re-fetching is separate — that's each
  // device's responsibility (DeviceConnection).

  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      setPush(pushState());
    };
    const onFocus = () => setPush(pushState());
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onFocus);
    // The Permissions API fires 'change' the instant the setting flips, even
    // without a focus/visibility bounce. Best-effort: older Safari lacks it.
    let perm: PermissionStatus | undefined;
    navigator.permissions
      ?.query({ name: "notifications" as PermissionName })
      .then((status) => {
        perm = status;
        status.addEventListener("change", onFocus);
      })
      .catch(() => {
        /* Permissions API / "notifications" name unsupported; focus+visibility cover it */
      });
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onFocus);
      perm?.removeEventListener("change", onFocus);
    };
  }, []);

  // Subscription registration itself is not here: it's per **device** (relay subscriptions
  // are one file per channel), so that logic lives in DeviceConnection — each device
  // registers once after connecting.

  // Native shell delivers vendor push tokens. Arrival is unpredictable (shell must pass
  // system notification permission first), so we just recalculate push state once.
  // classifyPush returns "granted" when it sees a token, then the "granted && not opted
  // out → enablePush" effect handles relay registration. Register logic is in one place,
  // no need to duplicate here.

  useEffect(() => {
    return onNativePushToken(() => setPush(pushState()));
  }, []);

  // Notification click → jump to that decision card. Three delivery paths (cold-start URL /
  // SW foreground re-delivery / native shell injection) all funnel through onDecisionDeepLink;
  // here we just switch tabs and focus once we have the target.
  //
  // Use nonce instead of just id: clicking the same notification twice has the same id,
  // so the DecisionsView effect wouldn't re-run and would appear unresponsive on the
  // second click.

  useEffect(() => {
    return onDecisionDeepLink((target) => {
      setTab("decisions");
      setFocusDecision({ id: target.id, nonce: Date.now() });
      // The relay's source mark is a prefix of the channel id; we only have the pairing
      // secret, so we recalculate each device's channel id to compare. This is async
      // (SubtleCrypto), so we focus by id first, then refine to the specific device after
      // getting it — clicking doesn't wait for hashing.
      //
      // Dynamic import + direct define wrapping: relayCrypto lives in relay-land, and
      // same-origin (webui) builds aren't allowed to have it. Going through a const
      // stops Rollup from inlining (see main.tsx and hostMode.test.ts for two empirical
      // tests).

      const mark = target.channelMark;
      if (!mark || import.meta.env.VITE_FLEET_HOST === "webui") return;
      void (async () => {
        const { channelIdOf } = await import("./relayCrypto");
        for (const d of bookRef.current.devices) {
          // 只有 relay 设备有 channel;HTTP 直连的通知不经 relay,也就不会带标记。
          if (d.kind !== "relay") continue;
          const id = await channelIdOf(d.secret);
          if (!id.startsWith(mark)) continue;
          setFocusDecision({ id: target.id, deviceId: d.id, nonce: Date.now() });
          return;
        }
      })();
    });
  }, []);

  /** Master toggle ON: ask for system permission using the current device's connection
   *  and register it, then clear mute flags for all other devices — their effects will
   *  register their own subscriptions next. */

  const handleEnablePush = useCallback(async () => {
    const ids = runtimeDevices.map((d) => d.id);
    if (client) setPush(await enablePush(client, relayBase, activeDeviceId));
    setPushOptedOut(false, ids);
    setPushMuted(Object.fromEntries(ids.map((id) => [id, false])));
  }, [client, relayBase, activeDeviceId, runtimeDevices]);

  /** Master toggle OFF: unsubscribe all devices. If we only unsubscribed the current
   *  one, the others would keep pushing — but the user just said "stop notifications". */

  const handleDisablePush = useCallback(async () => {
    const ids = runtimeDevices.map((d) => d.id);
    setPushOptedOut(true, ids);
    setPushMuted(Object.fromEntries(ids.map((id) => [id, true])));
    for (const id of ids) {
      const t = handlesRef.current[id]?.transport;
      if (!t) continue;
      // Current device calls disablePush (which also revokes the browser subscription);
      // others just send unsubscribe frames. The browser subscription is shared across
      // all devices, so revoking it would also disable the ones we haven't turned off yet.

      if (id === activeDeviceId) await disablePush(t, id);
      else await unsubscribeChannel(t);
    }
  }, [runtimeDevices, activeDeviceId]);

  /** Mute/unmute a single device (the device list toggle). Never touches the browser
   *  subscription itself here. */

  const handleMuteDevice = useCallback(
    async (device: PairedDevice, muted: boolean) => {
      setPushMuted((prev) => ({ ...prev, [device.id]: muted }));
      setPushMutedStored(device.id, muted);
      const t = handlesRef.current[device.id]?.transport;
      if (!t) return;
      if (muted) await unsubscribeChannel(t);
      else if (device.kind === "relay") await enablePush(t, device.relayBase, device.id);
    },
    [],
  );

  // Tasks list is also **merged**: all devices' sessions in one list, each marked with
  // its source device.
  const mergedSessions = useMemo<Array<WithDevice<SessionInfo>>>(
    () =>
      aggregateSessions(states, deviceOrder).sort(
        (a, b) => (b.lastActivityMs ?? 0) - (a.lastActivityMs ?? 0),
      ),
    [states, deviceOrder],
  );

  /** Sessions on the current scoped device. Used by device-scoped pages (new session,
   *  plans, subagent resolution in session detail) — those pages ask "what's on this
   *  device?", so mixing in others would make them see IDs they can't fetch. */

  const scopedSessions = useMemo(
    () => mergedSessions.filter((s) => s.deviceId === activeDeviceId),
    [mergedSessions, activeDeviceId],
  );

  /** Which directories the terminal can open in — every workspace seen in tasks,
   *  de-duped by device. Kept cross-device: terminal processes run on their owning host,
   *  so "which device" and "which directory" are a single unit. */

  const terminalWorkspaces = useMemo(() => {
    const seen = new Map<string, TerminalWorkspace>();
    for (const s of mergedSessions) {
      const key = `${s.deviceId}::${s.workspacePath}`;
      if (!seen.has(key)) {
        seen.set(key, {
          deviceId: s.deviceId,
          path: s.workspacePath,
          name: s.workspaceName,
        });
      }
    }
    return [...seen.values()].sort((a, b) => a.name.localeCompare(b.name));
  }, [mergedSessions]);

  /** Which device this new session opens on (defaults to currently scoped device if user hasn't picked). */
  const newSessionTargetId = newSessionDeviceId ?? activeDeviceId;
  /** Storage namespace for the target device. Same as `deviceId` — in mock/same-origin
   *  mode with only one data source, we don't add a prefix (see deviceScope.tsx). */
  const newSessionScopeId = MOCK || !NEEDS_PAIRING ? null : newSessionTargetId || null;
  /** Sessions on the target device. The new session form uses this only to calculate
   *  "recent workspace" — that's a directory on the target machine; paths from other
   *  devices don't exist there. */

  const newSessionSessions = useMemo(
    () => mergedSessions.filter((s) => s.deviceId === newSessionTargetId),
    [mergedSessions, newSessionTargetId],
  );

  // Reverse lookup: "what session owns this decision card?". The key is composite —
  // same-numbered sessions on two devices are two different sessions.
  const workspaceOf = useMemo(() => {
    const map = new Map<string, WithDevice<SessionInfo>>();
    for (const s of mergedSessions) map.set(itemKey(s.deviceId, s.id), s);
    return (deviceId: string, sessionId: string) => map.get(itemKey(deviceId, sessionId));
  }, [mergedSessions]);


  // The detail page derives its session from the live snapshot so status/plan chips
  // stay fresh while it's open. The top of the drill-down stack determines which one.

  const detailSession = useMemo(() => {
    const top = detailStack[detailStack.length - 1];
    if (!top) return null;
    return (
      mergedSessions.find((s) => s.deviceId === top.deviceId && s.id === top.id) ?? null
    );
  }, [mergedSessions, detailStack]);

  // Open a session as a fresh root detail (from the tasks / decisions lists).
  const openSessionRoot = useCallback(
    (deviceId: string, id: string) => setDetailStack([{ deviceId, id }]),
    [],
  );
  // Drill into a subagent from within a detail view — push another layer so the
  // back button returns to the opener. `AgentNav.open` prefixes `agent-`.
  const openSessionById = useCallback(
    (deviceId: string, id: string) => setDetailStack((s) => [...s, { deviceId, id }]),
    [],
  );

  // A secondary page (session detail / wiki doc / repo / usage / new session)
  // is covering the tab body. Used to decide whether the global decision drawer
  // should take over as the answering surface.
  const overlayOpen =
    detailStack.length > 0 ||
    wikiStack.length > 0 ||
    showRepo ||
    repoDetail !== null ||
    showUsage ||
    showPlans ||
    showWiki ||
    showNewSession;
  // Float the decision drawer over everything the user is looking at — EXCEPT the
  // plain decisions tab, which already renders cards inline (no overlay on top of it),
  // so the drawer would just duplicate them. Everywhere else (other tabs, or any tab
  // with a detail page open) the drawer is the answering surface.

  const showDecisionDrawer =
    decisions.length > 0 && !showNewSession && (tab !== "decisions" || overlayOpen);

  /** Which overlay is currently open. Used as resetKey for the overlay layer's error
   *  boundary — closing and reopening, or switching to a different overlay, automatically
   *  retries on error. One render failure shouldn't lock out this entry point permanently. */

  const overlayKey = [
    detailSession ? `detail:${detailSession.deviceId}:${detailSession.id}` : "",
    showWiki ? "wiki" : "",
    wikiStack.length ? `wiki:${wikiStack.length}:${wikiStack[wikiStack.length - 1].doc.slug}` : "",
    showRepo ? "repo" : "",
    repoDetail ? `repoDetail:${repoDetail.repo.root}` : "",
    showPlans ? "plans" : "",
    showUsage ? "usage" : "",
    showNewSession ? `newSession:${newSessionTargetId}` : "",
    showDecisionDrawer ? "drawer" : "",
  ]
    .filter(Boolean)
    .join("|");

  /** 把所有浮层收起来,回到主界面。
   *
   *  只有渲染兜底用得上:浮层的 `HistoryLayer`(接系统返回键的那个)和浮层内容是
   *  同一块 JSX,一起被 fallback 替换掉,于是返回键退不掉那个浮层(实测过)。iOS
   *  PWA 更没有系统返回键,所以 fallback 上必须有一条自己的出路,否则崩一次就把人
   *  困在那儿。 */
  const closeAllOverlays = useCallback(() => {
    setDetailStack([]);
    setShowWiki(false);
    setWikiStack([]);
    setShowRepo(false);
    setRepoDetail(null);
    setShowPlans(false);
    setShowUsage(false);
    setTerminal(null);
    setShowNewSession(false);
    setNewSessionDeviceId(null);
  }, []);

  if (!paired) {
    // 两条不依赖地址栏的配对入口。它们本来是原生壳限定的：壳从 rawfile 启动，
    // 没有「打开一条带 #k= 的链接」这回事；系统相机扫出来的链接由 App Link 决定
    // 交给谁，而 App Link 只认 manifest 里编译期写死的 host，自建 relay 的 host
    // 编译期不可知，那条路对它结构上不可用。app 内扫码拿到的是二维码原文，粘贴
    // 更是不依赖任何 host 声明。
    //
    // PWA 同样需要它们，而且是**唯一**的出路。iOS 把「添加到主屏幕」装出来的
    // web app 放进独立的存储分区：Safari 标签页里刚落盘的那份配对不会跟过去，
    // 而 A2HS 存的是 manifest 的 start_url（`/`），fragment 里的密钥也一并丢掉。
    // 于是用户第一次点主屏幕图标就落在这张门上 —— 主屏幕 app 没有地址栏，没法
    // 再开一次带 #k= 的链接，而门上一个按钮都没有，人就彻底卡死（老板 2026-09-15
    // 反馈）。
    //
    // 扫码那条按能力出：地址不是 https 时浏览器根本不暴露 getUserMedia，摆一个点
    // 了必然失败的按钮不如当场说清原因，把人直接引到粘贴（scanAvailability.ts）。
    const pairEntries = idbProbed;
    const scan = scanAvailability();
    if (scanning) {
      return <PairScanner onPaired={adoptPaired} onClose={() => setScanning(false)} />;
    }
    return (
      <div className={styles.gate}>
        <div className={styles.gateLogo}>F</div>
        <h1>{t("Fleet 移动端")}</h1>
        <p>
          {idbProbed
            ? t("扫描桌面端 Fleet「移动端」板块里的二维码完成配对。")
            : t("正在恢复配对…")}
        </p>
        {pairEntries && (
          <>
            {scan === "ok" ? (
              <button className={styles.gateButton} onClick={() => setScanning(true)}>
                {t("扫码配对")}
              </button>
            ) : (
              <p className={styles.gateNote}>
                {scan === "insecure-origin"
                  ? t("这个地址不是 HTTPS，浏览器不允许网页调用摄像头，扫码这条路走不了。请用下面的粘贴。")
                  : t("这台设备用不了摄像头，扫不了码。请用下面的粘贴。")}
              </p>
            )}
            <PairPasteForm onPaired={adoptPaired} />
          </>
        )}
      </div>
    );
  }

  // 配对失败的整屏拦截只在**只有一台**设备时成立:多台在册时,一台密钥失效不该
  // 把其他几台的卡一并挡在外面 —— 那一台的错误由「更多」页的连接状态如实呈现,
  // 用户可以在设备列表里把它移除或重新扫码。
  if (authError && runtimeDevices.length <= 1) {
    return (
      <div className={styles.gate}>
        <div className={styles.gateLogo}>F</div>
        <h1>{t("配对失败")}</h1>
        <p>{t("{0}。密钥可能已被重置，请回到桌面端重新扫码。", authError)}</p>
        <button
          className={styles.gateButton}
          onClick={() => {
            clearBook();
            clearCachedSessions(deviceId);
            location.reload();
          }}
        >
          {t("清除本机密钥")}
        </button>
      </div>
    );
  }

  return (
    // 整棵树跑在「当前作用域设备」里：里面所有按设备分家的本地存储（草稿、附件、
    // workspace 记忆）都从这里取命名空间，无需逐个 prop 往下传。
    <DeviceScopeProvider deviceId={deviceId}>
    {/* 每台设备一条连接。不渲染任何 DOM：它只是把那条 socket 的生命周期挂在
        React 树上，设备被移除时 key 消失、清理函数自然把它关掉。 */}
    {runtimeDevices.map((d, i) => (
      <DeviceConnection
        key={d.id}
        device={d}
        storageId={MOCK ? undefined : NEEDS_PAIRING ? d.id : null}
        makeTransport={makeTransport}
        dispatch={dispatch}
        registerHandle={registerHandle}
        hasPendingDecisions={(states[d.id]?.decisions.length ?? 0) > 0}
        agentOnline={states[d.id]?.agentOnline ?? false}
        isActive={d.id === activeDeviceId}
        index={i}
        visibility={visibility}
        connected={states[d.id]?.connected ?? false}
        pushGranted={push === "granted"}
        pushMuted={pushMuted[d.id] ?? true}
        onHostIdentity={adoptHostIdentity}
      />
    ))}
    <div className={styles.app}>
      <header className={styles.header}>
        {/* 标题位 = 当前设备。多台在册时它是切换器,一台时退化成那台的名字。 */}
        <DeviceSwitcher
          devices={runtimeDevices}
          activeId={activeDeviceId}
          statusOf={deviceStatusOf}
          open={deviceMenuOpen}
          onOpenChange={setDeviceMenuOpen}
          onSwitch={switchDevice}
          onManage={() => setTab("more")}
        />
        {todayUsage && (
          <span
            className={styles.usage}
            title={`${t("今日累计")} $${todayUsage.costUsd.toFixed(2)} · ${fmtTokens(todayUsage.inputTokens + todayUsage.outputTokens)} tok`}
          >
            <span className={styles.usageCost}>${todayUsage.costUsd.toFixed(2)}</span>
            <span className={styles.usageTokens}>
              {fmtTokens(todayUsage.inputTokens + todayUsage.outputTokens)}
            </span>
          </span>
        )}
        {/* 连接状态与强度收成一个图标：状态文案退居 title/aria-label，不再吃掉
            窄屏 header 的横向空间。 */}
        <span
          className={styles.connIcon}
          data-kind={connKind}
          role="img"
          aria-label={connText}
          title={rttSplit ? `${connText} · ${formatRttSplit(rttSplit, t)}` : connText}
        >
          <ConnIcon kind={connKind} />
        </span>
      </header>

      {/* 这条横幅讲的是「iOS 7 天不用会抹掉本地配对」——同源形态根本没有配对可丢
          （后端就是发出这张页面的那个进程），显示它纯属误导。用 NEEDS_PAIRING
          而不是 SUPPORTS_PUSH：这条说的是配对，不是推送。

          文案里那句「首次打开需要再扫一次码」不是免责声明，是这条路的实情：iOS 把
          主屏幕 web app 的存储单独分区，Safari 里的配对不会跟过去，而 A2HS 存的是
          manifest 的 start_url，fragment 里的密钥也带不走。不先说明，用户点开图标
          撞上配对门只会以为坏了（见配对门的注释）。 */}
      {NEEDS_PAIRING && !MOCK && needsA2hsForDurableStorage() && !a2hsDismissed && (
        <div className={styles.pushBanner}>
          <span>
            {t(
              "建议用 Safari 分享菜单「添加到主屏幕」：留在 Safari 里 7 天不访问，iOS 会清掉本机配对。主屏幕 app 的存储是独立的一份，首次打开需再扫一次码。",
            )}
          </span>
          <button
            className={styles.pushButton}
            onClick={() => {
              localStorage.setItem(A2HS_DISMISSED_KEY, "1");
              setA2hsDismissed(true);
            }}
          >
            {t("知道了")}
          </button>
        </div>
      )}

      {/* 这条横幅以前关不掉：`ios-needs-a2hs` 与 `denied` 两个分支连个按钮都没有，
          而 iOS 上前者恰恰是**常驻**的（用 Safari 看就一直满足），于是每一屏顶上
          都挂着一条撵不走的告示。关掉记的是**当时那个状态**而不是一个布尔位：
          「先添加到主屏幕」被撵走之后，后来真的变成「权限被拒绝」时那条新消息仍
          该出来说话。 */}
      {!MOCK &&
        push !== "granted" &&
        push !== "unsupported" &&
        push !== "unsupported-harmony" &&
        pushNoticeDismissed !== push && (
        <div className={styles.pushBanner}>
          {push === "ios-needs-a2hs" ? (
            <span>{t("要接收通知，请先用 Safari 分享菜单「添加到主屏幕」，再从主屏幕打开。")}</span>
          ) : push === "denied" ? (
            <span>{t("通知权限已被拒绝，请在系统设置中为本站点重新开启。")}</span>
          ) : (
            <>
              <span>{t("开启通知，第一时间收到新决策卡。")}</span>
              <button className={styles.pushButton} onClick={handleEnablePush}>
                {t("开启")}
              </button>
            </>
          )}
          <button
            className={styles.bannerClose}
            aria-label={t("关闭提示")}
            title={t("关闭提示")}
            onClick={() => dismissPushNotice(push)}
          >
            <X size={16} />
          </button>
        </div>
      )}

      {/* 每个 tab 一层。resetKey 是 tab 名 —— 一个 tab 崩了,底部导航还在,切走
          再切回来自动重试。以前这里任何一处抛异常都是整个 app 变白。 */}
      <main className={styles.main}>
        <ErrorBoundary label={t("{0} 页", t(TAB_LABEL[tab]))} resetKey={tab}>
        {tab === "decisions" ? (
          <DecisionsView
            decisions={decisions}
            transportFor={transportFor}
            connected={connected}
            agentOnline={agentOnline}
            decisionsLoaded={decisionsLoaded}
            workspaceOf={workspaceOf}
            onAnswered={markAnswered}
            onOpenSession={openSessionRoot}
            deviceLabelOf={deviceLabelOf}
            offlineDevices={offlineDevices}
            focusDecision={focusDecision}
          />
        ) : tab === "tasks" ? (
          <TasksView
            sessions={mergedSessions}
            deviceLabelOf={runtimeDevices.length > 1 ? deviceLabelOf : undefined}
            clientFor={transportFor}
            connected={connected}
            agentOnline={agentOnline}
            sessionsLoaded={sessionsLoaded}
            onOpenSession={(s: WithDevice<SessionInfo>) => openSessionRoot(s.deviceId, s.id)}
          />
        ) : tab === "artifacts" ? (
          <ArtifactsView client={client} />
        ) : (
          <MoreView
            endpointLabel={client?.endpointLabel ?? ""}
            onOpenTerminal={() => setTerminal({ workspace: null })}
            terminalEnabled={hostFeatures.terminal}
            devices={book.devices}
            activeDeviceId={deviceId}
            activeKind={current?.kind ?? (NEEDS_PAIRING && !MOCK ? "relay" : "http")}
            onSwitchDevice={switchDevice}
            onRenameDevice={renameDeviceLabel}
            onRemoveDevice={(d) => void removeDeviceEntry(d)}
            deviceMuted={(id) => pushMuted[id] ?? true}
            onMuteDevice={(d, muted) => void handleMuteDevice(d, muted)}
            onAddDevice={adoptPaired}
            onUnpairAll={unpairAll}
            supportsPush={SUPPORTS_PUSH}
            connected={connected}
            agentOnline={agentOnline}
            sessionsFrame={sessionsFrame}
            rttSplit={rttSplit}
            snapshotSources={snapshotSources}
            push={push}
            pushOptedOut={pushOptedOut}
            onEnablePush={handleEnablePush}
            onDisablePush={handleDisablePush}
            onOpenRepo={() => setShowRepo(true)}
            onOpenPlans={() => setShowPlans(true)}
            onOpenWiki={() => setShowWiki(true)}
            onOpenUsage={() => setShowUsage(true)}
          />
        )}
        </ErrorBoundary>
      </main>

      {/* 后退栈的底层：只要不在主页 tab，返回一次先回主页（安卓惯例，反复切 tab
          也只占一条历史），再返回才轮到栈底的退出确认。挂在浮层之前，保证浮层始终压在它上面。 */}
      {tab !== "decisions" && <HistoryLayer onBack={() => setTab("decisions")} />}

      {/* 浮层各自一层。tab 层管不到这里(浮层是 main 的兄弟),而根层接住只会
          让整个 app 变成一张错误页 —— 已知咬过人的 sources_config 异形应答就发生
          在新会话表单里,它正是一个浮层。resetKey 是栈顶浮层的身份,所以关掉再开
          自动重试。 */}
      <ErrorBoundary
        label={t("当前页面")}
        resetKey={overlayKey}
        onDismiss={{ label: t("返回主界面"), run: closeAllOverlays }}
      >
      {/* 每一层下钻占一层历史，但只渲染栈顶那层详情——底下几层不必挂着重复拉 tail。 */}
      {detailStack.map((_, i) => (
        <HistoryLayer key={i} onBack={() => setDetailStack((s) => s.slice(0, i))} />
      ))}
      {detailSession && (
        <SessionDetailView
          key={sessionDetailKey(detailSession.deviceId, detailSession.id)}
          session={detailSession}
          // 详情页解析子代理/父会话都按 id 找,所以只给它**这条会话所属那一台**的
          // 列表 —— 混进别台的会话只会让它按同名 id 找到一条自己拉不动的记录。
          sessions={mergedSessions.filter((s) => s.deviceId === detailSession.deviceId)}
          client={transportFor(detailSession.deviceId)}
          onBack={() => setDetailStack((s) => s.slice(0, -1))}
          onOpenSessionId={(id: string) => openSessionById(detailSession.deviceId, id)}
          // 头部那条状态轨上「N 张待决策」要靠这个数。决策卡是跨设备聚合的一个
          // 收件箱，不挂在 SessionInfo 上，所以按 (设备, 会话) 在这里数——只数
          // 这一台的卡，免得别台一张同名会话的卡被算进来。
          pendingDecisions={
            decisions.filter(
              (d) =>
                d.deviceId === detailSession.deviceId &&
                d.request.sessionId === detailSession.id,
            ).length
          }
        />
      )}

      {/* 知识库列表本身也是一层浮层（从「更多」进来），文档详情再压在它之上——
          所以它要排在 wikiStack 之前渲染。 */}
      {showWiki && (
        <>
          <HistoryLayer onBack={() => setShowWiki(false)} />
          <WikiView
            client={client}
            onBack={() => setShowWiki(false)}
            onOpenDoc={(doc) => setWikiStack([{ deviceId: activeDeviceId, doc }])}
          />
        </>
      )}

      {/* 每篇文档占一层，但只渲染栈顶那篇——底下几篇不必挂着重复拉正文/渲 mermaid。 */}
      {wikiStack.map((_, i) => (
        <HistoryLayer key={i} onBack={() => setWikiStack((s) => s.slice(0, i))} />
      ))}
      {wikiStack.length > 0 && (
        <WikiDocView
          doc={wikiStack[wikiStack.length - 1].doc}
          client={transportFor(wikiStack[wikiStack.length - 1].deviceId)}
          onBack={() => setWikiStack((s) => s.slice(0, -1))}
          onOpenDoc={(doc) =>
            setWikiStack((s) => [...s, { deviceId: s[s.length - 1].deviceId, doc }])
          }
        />
      )}

      {showRepo && (
        <>
          <HistoryLayer onBack={() => setShowRepo(false)} />
          <RepoView
            client={client}
            onBack={() => setShowRepo(false)}
            onOpenRepo={(repo) => setRepoDetail({ deviceId: activeDeviceId, repo })}
          />
        </>
      )}

      {repoDetail && (
        <>
          <HistoryLayer onBack={() => setRepoDetail(null)} />
          <RepoDetailView
            repo={repoDetail.repo}
            client={transportFor(repoDetail.deviceId)}
            onBack={() => setRepoDetail(null)}
          />
        </>
      )}

      {/* hostFeatures 默认全关,所以这一层在答案回来之前也不会闪一下 */}
      {terminal && hostFeatures.terminal && (
        <>
          <HistoryLayer onBack={() => setTerminal(null)} />
          <TerminalView
            workspaces={terminalWorkspaces}
            initial={terminal.workspace}
            clientFor={transportFor}
            onBack={() => setTerminal(null)}
          />
        </>
      )}

      {showPlans && (
        <>
          <HistoryLayer onBack={() => setShowPlans(false)} />
          <PlansView
            sessions={scopedSessions}
            client={client}
            onBack={() => setShowPlans(false)}
          />
        </>
      )}

      {showUsage && (
        <>
          <HistoryLayer onBack={() => setShowUsage(false)} />
          <UsageView
            client={client}
            todayUsage={todayUsage}
            perDevice={usageRows}
            activeDeviceLabel={deviceLabelOf(activeDeviceId)}
            onBack={() => setShowUsage(false)}
          />
        </>
      )}

      {showNewSession && (
        <>
          <HistoryLayer onBack={() => setShowNewSession(false)} />
          {/* 表单整个跑在**目标设备**的作用域里,而不是当前作用域那台:里面的
              草稿、附件、上次用的 repo 都是「某一台机器上的东西」。
              `key` 是必需的而非优化 —— useDraft 只在挂载时读盘(见 draft.ts),
              光换 provider 的话换设备后表单会顶着 A 的 workspace/附件,还会把
              它们写进 B 的命名空间。重挂载让每台各自恢复自己的那份。 */}
          <DeviceScopeProvider deviceId={newSessionScopeId}>
            <NewSessionSheet
              key={newSessionTargetId}
              sessions={newSessionSessions}
              client={transportFor(newSessionTargetId)}
              devices={runtimeDevices}
              targetDeviceId={newSessionTargetId}
              onTargetDevice={setNewSessionDeviceId}
              initialFiles={sharedFiles}
              relayReady={states[newSessionTargetId]?.connected ?? false}
              onClose={() => {
                setShowNewSession(false);
                // 下次打开回到「当前作用域那台」,不记住上次挑的那台 —— 记住会
                // 让人在 A 页面上打开表单却默默开去 B。
                setNewSessionDeviceId(null);
                // Consumed by the sheet — don't re-upload them if it reopens.
                setSharedFiles([]);
              }}
            />
          </DeviceScopeProvider>
        </>
      )}

      {showDecisionDrawer && (
        <DecisionDrawer
          decisions={decisions}
          transportFor={transportFor}
          connected={connected}
          agentOnline={agentOnline}
          decisionsLoaded={decisionsLoaded}
          workspaceOf={workspaceOf}
          onAnswered={markAnswered}
          onOpenSession={openSessionRoot}
          deviceLabelOf={deviceLabelOf}
        />
      )}

      </ErrorBoundary>

      {exitArmed && <div className={styles.exitToast}>{t("再按一次返回退出")}</div>}

      <nav className={styles.tabs}>
        <button
          className={styles.tabButton}
          data-active={tab === "decisions"}
          onClick={() => setTab("decisions")}
        >
          <span className={styles.tabIcon}>
            <Inbox size={20} />
          </span>
          {t("决策")}
          {decisions.length > 0 && <span className={styles.badge}>{decisions.length}</span>}
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "tasks"}
          onClick={() => setTab("tasks")}
        >
          <span className={styles.tabIcon}>
            <ListChecks size={20} />
          </span>
          {t("任务")}
        </button>
        <button
          className={styles.centerTab}
          aria-label={t("新会话")}
          onClick={() => setShowNewSession(true)}
        >
          <span className={styles.centerFab}>
            <Plus size={24} />
          </span>
          <span className={styles.centerLabel}>{t("新会话")}</span>
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "artifacts"}
          onClick={() => setTab("artifacts")}
        >
          <span className={styles.tabIcon}>
            <Package size={20} />
          </span>
          {t("产出")}
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "more"}
          onClick={() => setTab("more")}
        >
          <span className={styles.tabIcon}>
            <MoreHorizontal size={20} />
          </span>
          {t("更多")}
        </button>
      </nav>
    </div>
    </DeviceScopeProvider>
  );
}
