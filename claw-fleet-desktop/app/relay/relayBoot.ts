/**
 * Boot path for the desktop bundle served from a relay origin (`/desktop/`).
 *
 * The relay origin also serves mobile-web at its root, and mobile-web keeps
 * the phone's paired Macs in localStorage under `fleet-devices` — same origin,
 * so this page shares that book (format and operations in
 * `shared-ts/deviceBook.ts`). Opened with a pairing fragment (`#k=<secret>`,
 * the desktop's "open on a computer" link) it adds that Mac to the book
 * first, exactly as mobile-web would. Then: one relay pairing → use it.
 * Several → let the user pick (remembered). None → point at mobile-web's
 * pairing page and stop.
 *
 * Once a device is chosen this wires the HTTP-over-relay tunnel
 * (`tunnel.ts`): page `fetch` goes straight over the socket, browser-initiated
 * loads go through the Service Worker, and the asset URL helpers are pointed
 * at the tunnel prefix.
 */

import {
  DEVICE_BOOK_KEY as SHARED_BOOK_KEY,
  addDevice,
  emptyBook,
  nextDeviceLabel,
  parseBook,
  persistBook,
  type DeviceBook,
} from "../../../shared-ts/deviceBook";
import { parseHashPairing } from "../../../shared-ts/pairingLink";
import { setWebApiPrefix } from "../hostEnv";
import { RelaySocket, type RelaySocketStatus } from "./relaySocket";
import { attachSwBridge, installTunnelFetch, tunnelPrefix } from "./tunnel";

/** mobile-web's device book key (`mobile-web/src/devices.ts`). */
export const DEVICE_BOOK_KEY = SHARED_BOOK_KEY;
/** Which paired Mac this browser last chose for the desktop view. */
export const CHOSEN_DEVICE_KEY = "fleet-web:relay-device";

export interface RelayDeviceEntry {
  id: string;
  label: string;
  secret: string;
  /** `null` = the relay that served this page. */
  relayBase: string | null;
}

/**
 * The relay pairings in mobile-web's book, parsed by the same `parseBook`
 * mobile-web uses: a record without `kind` predates multi-transport days and
 * is a relay record when it has a secret; `http` records (direct hosts) are
 * not reachable over the relay and are skipped; a malformed record is dropped
 * alone.
 */
export function relayDevicesFromBook(raw: string | null): RelayDeviceEntry[] {
  const book = parseBook(raw);
  if (!book) return [];
  const out: RelayDeviceEntry[] = [];
  for (const d of book.devices) {
    if (d.kind !== "relay") continue;
    out.push({
      id: d.id,
      label: d.label || d.id.slice(0, 8),
      secret: d.secret,
      relayBase: d.relayBase || null,
    });
  }
  return out;
}

export interface PairingIntakeEnv {
  /** `window.location.hash`. */
  hash: string;
  /** Current book JSON (`localStorage[fleet-devices]`), or null. */
  readBook: () => string | null;
  /** Persist the updated book (production: shared `persistBook`, which mirrors to IDB). */
  writeBook: (book: DeviceBook) => void;
  /** Remove the fragment from the address bar. */
  scrub: () => void;
  /** Fresh id for a newly added device. */
  newId: () => string;
  /** Default-name prefix (mobile-web uses its localized "设备"/"Device"). */
  labelPrefix: string;
  now: number;
}

/**
 * Take a `#k=<secret>` pairing fragment into the shared device book, the way
 * mobile-web's `consumeHashPairing` + `adoptScannedDevice` do: the fragment is
 * scrubbed before anything else (the secret must not linger in the address
 * bar or history), a secret already in the book is the same Mac (no second
 * entry; its label survives), and the adopted device becomes active. Returns
 * the device id, or `null` when the fragment carries no pairing.
 */
export function adoptPairingFromHash(env: PairingIntakeEnv): string | null {
  const pairing = parseHashPairing(env.hash);
  if (!pairing) return null;
  env.scrub();
  const book = parseBook(env.readBook()) ?? emptyBook();
  const { book: next, device } = addDevice(book, {
    secret: pairing.secret,
    relayBase: pairing.relayBase,
    id: env.newId(),
    label: nextDeviceLabel(book, env.labelPrefix),
    now: env.now,
  });
  env.writeBook(next);
  return device.id;
}

function randomId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  }
}

/** The device to use without asking, or `null` when the user must choose. */
export function preselectDevice(
  devices: RelayDeviceEntry[],
  chosenId: string | null,
  forcePick: boolean,
): RelayDeviceEntry | null {
  if (devices.length === 1) return devices[0];
  if (forcePick) return null;
  return devices.find((d) => d.id === chosenId) ?? null;
}

// ── Minimal pre-React UI ────────────────────────────────────────────────────
// This runs before i18n and React load, so it is plain DOM with its own two
// strings tables.

const COPY = {
  zh: {
    noDevice: "这个浏览器还没有配对任何 Mac。",
    pairHint: "先在本站的移动端页面扫码配对，再回到这里。",
    pairLink: "去配对",
    pick: "选择要连接的 Mac",
    offline: "Mac 不在线",
    connecting: "正在连接 relay…",
  },
  en: {
    noDevice: "This browser has no paired Mac yet.",
    pairHint: "Pair on this site's mobile page first, then come back.",
    pairLink: "Pair a Mac",
    pick: "Choose a Mac to connect to",
    offline: "Mac is offline",
    connecting: "Connecting to relay…",
  },
};

function copy() {
  return /^zh/i.test(navigator.language || "") ? COPY.zh : COPY.en;
}

function overlay(): HTMLDivElement {
  const el = document.createElement("div");
  el.style.cssText =
    "position:fixed;inset:0;z-index:2147483646;display:flex;flex-direction:column;" +
    "align-items:center;justify-content:center;gap:12px;padding:16px;" +
    "background:#0d0e10;color:#e8e6e3;font:14px/1.5 -apple-system,system-ui,sans-serif;";
  document.body.appendChild(el);
  return el;
}

function showNoDevice(): void {
  const c = copy();
  const el = overlay();
  const title = document.createElement("div");
  title.style.cssText = "font-size:16px;font-weight:600";
  title.textContent = c.noDevice;
  const hint = document.createElement("div");
  hint.style.opacity = "0.7";
  hint.textContent = c.pairHint;
  const link = document.createElement("a");
  // mobile-web is served at the relay's root; with no devices it opens on its
  // pairing gate.
  link.href = "/";
  link.textContent = c.pairLink;
  link.style.cssText = "color:#e8a55a";
  el.append(title, hint, link);
}

function chooseDevice(devices: RelayDeviceEntry[]): Promise<RelayDeviceEntry> {
  const c = copy();
  const el = overlay();
  const title = document.createElement("div");
  title.style.cssText = "font-size:16px;font-weight:600";
  title.textContent = c.pick;
  el.append(title);
  return new Promise((resolve) => {
    for (const d of devices) {
      const btn = document.createElement("button");
      btn.textContent = d.label;
      btn.style.cssText =
        "min-width:240px;padding:10px 16px;border-radius:8px;border:1px solid #3a3b3f;" +
        "background:#18191c;color:inherit;font:inherit;cursor:pointer";
      btn.onclick = () => {
        el.remove();
        resolve(d);
      };
      el.append(btn);
    }
  });
}

/** Small corner pill while the relay is not carrying traffic. */
function statusBadge(): (s: RelaySocketStatus) => void {
  const el = document.createElement("div");
  el.style.cssText =
    "position:fixed;left:12px;bottom:12px;z-index:2147483645;padding:4px 10px;" +
    "border-radius:999px;background:#3a2a14;color:#f0c890;" +
    "font:12px/1.4 -apple-system,system-ui,sans-serif;display:none;pointer-events:none";
  const attach = () => document.body.appendChild(el);
  if (document.body) attach();
  else window.addEventListener("DOMContentLoaded", attach);
  return (s) => {
    const c = copy();
    if (s.connected && s.agentOnline) {
      el.style.display = "none";
      return;
    }
    el.textContent = s.error ?? (s.connected ? c.offline : c.connecting);
    el.style.display = "block";
  };
}

/** Wait (bounded) until this page is controlled by the tunnel's SW. */
async function registerTunnelWorker(mount: string, timeoutMs: number): Promise<boolean> {
  if (!("serviceWorker" in navigator)) return false;
  try {
    await navigator.serviceWorker.register(`${mount}sw.js`, { scope: mount });
  } catch {
    return false;
  }
  if (navigator.serviceWorker.controller) return true;
  // First visit: the worker claims open clients on activate.
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), timeoutMs);
    navigator.serviceWorker.addEventListener("controllerchange", () => {
      clearTimeout(timer);
      resolve(true);
    });
  });
}

export interface RelayBoot {
  /** Probe base for liveProxy (`/desktop/h`). */
  prefix: string;
  socket: RelaySocket;
  /** Whether the SW controls the page (img/iframe loads are tunnelled). */
  swControlled: boolean;
}

/**
 * Pick a device, connect, and install the tunnel. Resolves `null` when there
 * is nothing to connect to (the page then shows the pairing pointer and the
 * app does not boot).
 */
export async function bootRelayTransport(mount: string): Promise<RelayBoot | null> {
  let raw: string | null = null;
  let chosenId: string | null = null;
  const adoptedId = adoptPairingFromHash({
    hash: window.location.hash,
    readBook: () => {
      try {
        return window.localStorage.getItem(DEVICE_BOOK_KEY);
      } catch {
        return null;
      }
    },
    writeBook: persistBook,
    scrub: () =>
      history.replaceState(null, "", window.location.pathname + window.location.search),
    newId: randomId,
    labelPrefix: /^zh/i.test(navigator.language || "") ? "设备" : "Device",
    now: Date.now(),
  });
  if (adoptedId) {
    try {
      // The link names one Mac: connect to it, even when others are paired.
      window.localStorage.setItem(CHOSEN_DEVICE_KEY, adoptedId);
    } catch {
      /* not remembered; the picker asks */
    }
  }
  try {
    raw = window.localStorage.getItem(DEVICE_BOOK_KEY);
    chosenId = window.localStorage.getItem(CHOSEN_DEVICE_KEY);
  } catch {
    /* storage blocked: treated as no devices */
  }
  const devices = relayDevicesFromBook(raw);
  if (devices.length === 0) {
    showNoDevice();
    return null;
  }
  const forcePick = new URLSearchParams(window.location.search).has("pick");
  const device = preselectDevice(devices, chosenId, forcePick) ?? (await chooseDevice(devices));
  try {
    window.localStorage.setItem(CHOSEN_DEVICE_KEY, device.id);
  } catch {
    /* not remembered; asked again next time */
  }

  const prefix = tunnelPrefix(mount);
  const socket = new RelaySocket(
    device.secret,
    device.relayBase ?? window.location.origin,
    statusBadge(),
  );
  // Bridge before registering, so a request the SW sends the moment it
  // claims this page already has a listener.
  attachSwBridge(socket, prefix);
  installTunnelFetch(socket, prefix);
  setWebApiPrefix(prefix);
  socket.connect();

  const [swControlled] = await Promise.all([
    registerTunnelWorker(mount, 4000),
    // Do not hold the boot hostage to a Mac that is offline: requests made
    // before auth fail fast and the UI degrades as it does for a server error.
    socket.waitAuthed(10_000).catch(() => {}),
  ]);
  if (!swControlled) {
    console.warn("[relay] page is not SW-controlled — images and iframes will not load");
  }
  (window as unknown as Record<string, unknown>).__relaySocket = socket;
  return { prefix, socket, swControlled };
}
