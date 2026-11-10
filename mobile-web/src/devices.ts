// Device registry: every Fleet desktop this phone has paired.
//
// Previously "pairing" was singular: one `fleet-relay-secret` for one desktop. The reason
// multi-device doesn't simply store the key as an array is that **each pairing carries its own
// relay address**: a desktop on a self-hosted relay and one on the default relay must coexist,
// so relay affinity follows the device, not a module-level constant (see RELAY_BASE in relay.ts).
//
// Persistence mirrors secretStore.ts (localStorage + IndexedDB copy) for the same reason:
// on non-A2HS iOS Safari, script-writable storage gets wiped 7 days after last access, and
// the two stores are independently garbage-collected, so whichever survives gets used. IDB
// reuses secretStore's library/store, just under a different key — no schema change.
//
// Deliberately split into two layers: **pure functions** (book CRUD, migration check) and
// **storage I/O**. Test environments (Node) have no IndexedDB, so all assertable semantics
// live in the pure layer; I/O only shuttles the book in/out, any failure degrades to "this
// store had nothing", never throws.

import {
  DEVICE_BOOK_KEY as BOOK_KEY,
  addDevice,
  emptyBook,
  looksMacAddressLabel,
  openDeviceDb as openDb,
  parseBook,
  persistBook,
  type DeviceBook,
  type PairedDevice,
} from "../../shared-ts/deviceBook";
import { parseHashPairing, type HashPairing } from "../../shared-ts/pairingLink";

// The record format and its pure operations live in shared-ts so the desktop
// bundle served at `<relay>/desktop/` writes the same records (deviceBook.ts).
export {
  addDevice,
  emptyBook,
  looksAutoLabel,
  looksMacAddressLabel,
  nextDeviceLabel,
  parseBook,
  persistBook,
} from "../../shared-ts/deviceBook";
export type {
  AddDeviceInput,
  AddDeviceResult,
  DeviceBook,
  DeviceKind,
  HttpDevice,
  PairedDevice,
  RelayDevice,
} from "../../shared-ts/deviceBook";
export type { HashPairing } from "../../shared-ts/pairingLink";

/** Key for storing the book locally (localStorage and IDB share the same key name). */
/** Key from single-device era. Read once during migration, never written again. */
const LEGACY_SECRET_KEY = "fleet-relay-secret";



/** Single-device-era secret → single-entry book. */
export function bookFromLegacySecret(secret: string, opts: DeviceMint): DeviceBook {
  const device: PairedDevice = {
    kind: "relay",
    id: opts.id,
    label: opts.label,
    auto: true,
    secret,
    // The migrated device never recorded a relay: it always used the build default
    // (RELAY_BASE in old code), so null here means "use default", not "unknown".
    relayBase: null,
    addedAt: opts.now,
  };
  return { devices: [device], activeId: device.id };
}

export function activeDevice(book: DeviceBook): PairedDevice | null {
  if (!book.activeId) return null;
  return book.devices.find((d) => d.id === book.activeId) ?? null;
}

export function deviceById(book: DeviceBook, id: string): PairedDevice | null {
  return book.devices.find((d) => d.id === id) ?? null;
}


/** The naming-usable part of the host-reported identity.
 *
 *  Use only hostname: it's the name users know the machine by elsewhere ("Harrys-MacBook-Pro").
 *  Platform and OS version do **not** participate in naming — "macOS device" is no better than
 *  "Device 2" when multiple Macs are paired, it just confuses more. Keep those fields for
 *  icons and details. Return `null` if no hostname, leaving the default "Device N" instead of
 *  fabricating one.
 *
 *  A hostname that is a bare MAC address is rejected as if it were absent: a desktop old enough
 *  to still read its drifting transient hostname reports one over a phone hotspot, and "Device 2"
 *  is a better name than a MAC address. */
export function hostDisplayName(identity: { hostname?: string | null } | null): string | null {
  const raw = identity?.hostname?.trim();
  if (!raw || looksMacAddressLabel(raw)) return null;
  return raw;
}

/** The desktop reported its identity — store its name and platform in the book.
 *
 *  Three rules:
 *  1. **Only override auto names** (`auto !== false`). A name the user edited in "More"
 *     is explicit intent; can't be clobbered by reconnection. The one exception is a label that
 *     is a bare MAC address: no user types that, it can only have come from a host whose
 *     transient hostname had drifted, and without this exception such a record stays poisoned
 *     forever — the phone keeps showing `de:e8:92:d6:ca:71` even after the host learns its
 *     real name.
 *  2. **Disambiguate duplicates**. When two hostnames collide (both "mac-mini"), give
 *     the later one a number, or the device switcher shows two identical entries.
 *  3. **Platform always updates**, exempt from rule 1 — it only drives the icon,
 *     doesn't conflict with user-chosen names. */
export function applyHostIdentity(
  book: DeviceBook,
  id: string,
  identity: { hostname?: string | null; platform?: string | null },
): DeviceBook {
  const target = book.devices.find((d) => d.id === id);
  if (!target) return book;
  const platform = identity.platform?.trim() || target.platform;
  const name = hostDisplayName(identity);
  const userNamed = target.auto === false && !looksMacAddressLabel(target.label);
  const keepLabel = userNamed || !name;
  const label = keepLabel ? target.label : uniqueLabel(book, id, name);
  if (label === target.label && platform === target.platform) return book;
  return {
    ...book,
    devices: book.devices.map((d) =>
      d.id === id ? { ...d, label, platform, auto: keepLabel ? d.auto : true } : d,
    ),
  };
}

/** `name`, or `name 2`, `name 3`... if already in use by another device. */
function uniqueLabel(book: DeviceBook, selfId: string, name: string): string {
  const used = new Set(book.devices.filter((d) => d.id !== selfId).map((d) => d.label));
  if (!used.has(name)) return name;
  for (let n = 2; ; n++) {
    const candidate = `${name} ${n}`;
    if (!used.has(candidate)) return candidate;
  }
}


/** Remove a device. If the deleted device was active, move focus to the first remaining
 *  device (or `null` if none left, returning to unpaired state). */
export function removeDevice(book: DeviceBook, id: string): DeviceBook {
  const devices = book.devices.filter((d) => d.id !== id);
  if (devices.length === book.devices.length) return book;
  const activeId =
    book.activeId === id ? (devices[0]?.id ?? null) : book.activeId;
  return { devices, activeId };
}

/** Rename a device. Blank names are ignored (otherwise the list shows a nameless device).
 *
 *  Also set `auto` to `false`: this device now has a user-chosen name and won't be
 *  overridden by hostname on reconnection. */
export function renameDevice(book: DeviceBook, id: string, label: string): DeviceBook {
  const trimmed = label.trim();
  if (!trimmed) return book;
  return {
    ...book,
    devices: book.devices.map((d) => (d.id === id ? { ...d, label: trimmed, auto: false } : d)),
  };
}

/** Point a relay device at another relay host. The official hosts
 *  (shared-ts/relayPresets.ts) front the same relay container, so the pairing secret
 *  stays valid; a custom host only works when the desktop is on it too. HTTP devices
 *  and unknown IDs are ignored; an unchanged host returns the same book so callers can
 *  skip a write. */
export function setDeviceRelay(book: DeviceBook, id: string, relayBase: string): DeviceBook {
  const target = book.devices.find((d) => d.id === id);
  if (!target || target.kind !== "relay" || target.relayBase === relayBase) return book;
  return {
    ...book,
    devices: book.devices.map((d) => (d.id === id ? { ...target, relayBase } : d)),
  };
}

/** Switch active device. Ignores unknown device IDs. */
export function setActiveDevice(book: DeviceBook, id: string): DeviceBook {
  if (!book.devices.some((d) => d.id === id)) return book;
  return { ...book, activeId: id };
}

// ── Storage I/O ────────────────────────────────────────────────────────────────

function readLocal(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** Synchronously available book: localStorage book, or migrated from single-device-era secret.
 *
 *  Migration is **write-back**: the migrated book is persisted immediately, so the next
 *  startup reads the new format. Old key is deliberately **not deleted** — if the new format
 *  fails to write for any reason, the old key remains and the user doesn't lose pairing;
 *  it just won't be written to again. */
export function loadBookSync(mint: DeviceMint): DeviceBook {
  const parsed = parseBook(readLocal(BOOK_KEY));
  if (parsed) return parsed;
  const legacy = readLocal(LEGACY_SECRET_KEY);
  if (legacy) {
    const book = bookFromLegacySecret(legacy, mint);
    persistBook(book);
    return book;
  }
  return emptyBook();
}

/** Three local fields for a new device. Caller (App) generates them because default names
 *  go through i18n and this layer deliberately doesn't know about it. */
export interface DeviceMint {
  id: string;
  label: string;
  now: number;
}

/** IndexedDB fallback: the path when localStorage is wiped but IDB survives.
 *  Also covers old keys (old versions mirrored secret into IDB too). */
export async function loadBookFromIdb(mint: DeviceMint): Promise<DeviceBook | null> {
  const raw = await idbGet(BOOK_KEY);
  const parsed = parseBook(raw);
  if (parsed) return parsed;
  const legacy = await idbGet(LEGACY_SECRET_KEY);
  if (typeof legacy === "string" && legacy) {
    return bookFromLegacySecret(legacy, mint);
  }
  return null;
}

function idbGet(key: string): Promise<unknown> {
  return openDb()
    .then(
      (db) =>
        new Promise<unknown>((resolve) => {
          const tx = db.transaction("kv", "readonly");
          const req = tx.objectStore("kv").get(key);
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => resolve(null);
        }),
    )
    .catch(() => null);
}


/** One QR scan landing: merge secret into book and **persist immediately**.
 *
 *  Two pairing entries (PWA's `#k=…` fragment, native shell's Universal/App Link) must use
 *  the same function — they each had "store and make active" logic, but multi-device added
 *  dedup, preserve user renames, and focus transfer, so duplicating would let them drift.
 *
 *  Returns the new book; `added` is false if this code was already paired (second scan of same). */
export function adoptScannedDevice(
  book: DeviceBook,
  secret: string,
  mint: DeviceMint,
  relayBase?: string | null,
  opts?: { focus?: boolean },
): { book: DeviceBook; device: PairedDevice; added: boolean } {
  const { book: added, device, deduped } = addDevice(book, {
    secret,
    relayBase,
    id: mint.id,
    label: mint.label,
    now: mint.now,
  });
  // The native shell reinjects stored pairing on every startup (see mobile-harmony WebShell.ets).
  // That's not a "scan", so shouldn't grab focus back to that device — otherwise a user who
  // switched to another device in the list gets reset to it every app restart. Shell uses `&boot=1`
  // to say "startup reinject, not a fresh scan".
  const keepFocus = opts?.focus === false && deduped;
  const next = keepFocus ? { ...added, activeId: book.activeId ?? added.activeId } : added;
  // A real scan (not the shell's boot re-inject) brings a removed pairing back for good.
  if (opts?.focus !== false) unmarkSecretRemoved(secret);
  persistBook(next);
  return { book: next, device, added: !deduped };
}

/** "Add device" from address bar fragment: `#k=<secret>&relay=<url>` — relay-paired desktop
 *  (this is what desktop QR codes contain).
 *
 *  All three clients have only this path: system camera opens relay-hosted PWA, installed PWA
 *  reopens, or native shell scans and passes the URL to the page. */
// `HashPairing` (the parsed fragment) is defined in shared-ts/pairingLink.ts and re-exported above.

/** Is this pairing the shell's boot re-inject of a device the user removed? Such a pairing
 *  must not be adopted — see "Removed pairings" below. A device still in the book is never
 *  suppressed (the re-inject also refreshes its relay address). */
export function isRemovedBootPairing(book: DeviceBook, pairing: HashPairing): boolean {
  if (!pairing.boot) return false;
  if (book.devices.some((d) => d.kind === "relay" && d.secret === pairing.secret)) return false;
  return loadRemovedSecrets().includes(pairing.secret);
}

/** Extract "add device" from fragment and **immediately wipe the fragment from the address bar** —
 *  secrets and tokens shouldn't stay there to be screenshot or captured in history.
 *
 *  Read once only: after calling, hash is cleared; second call returns `null`. Native shell uses
 *  deepLink.ts and nativeScan.ts, which eventually route here.
 *
 *  One-shot read is mandatory, not optional: after wiping, no module can read it again, so relay's
 *  `&relay=` and shell's `&boot=1` must be extracted here (relay.ts's module-load-time RELAY_BASE
 *  constant exists for exactly this reason). */
export function consumeHashPairing(): HashPairing | null {
  const hash = window.location.hash;
  const scrub = () => history.replaceState(null, "", window.location.pathname);

  const pairing = parseHashPairing(hash);
  if (pairing) scrub();
  return pairing;
}

// ── Pending unsubscribe ────────────────────────────────────────────────────────
//
// When removing a device, we should tell its relay channel "stop pushing to me". This
// can fail (relay unreachable, phone offline), and the consequence is the user deletes
// a device but keeps getting its notifications — opening them shows no card. So we
// record failed unsubscribes and retry on next startup.
//
// We record secret + relayBase because unsubscribe must connect as that channel
// (channel token is derived from secret). They already exist in the same store
// (device book), so this adds no new exposure surface.

const PENDING_UNSUB_KEY = "fleet-pending-unsub";
/** Give up retry after this duration: the desktop may no longer be in use, and a
 *  permanently failing record shouldn't try unreachable addresses on every startup. */
const PENDING_UNSUB_TTL_MS = 7 * 24 * 60 * 60 * 1000;

export interface PendingUnsub {
  secret: string;
  relayBase: string | null;
  at: number;
}

export function loadPendingUnsub(now: number): PendingUnsub[] {
  const raw = readLocal(PENDING_UNSUB_KEY);
  if (!raw) return [];
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  return value.filter((v): v is PendingUnsub => {
    if (typeof v !== "object" || v === null) return false;
    const e = v as Record<string, unknown>;
    if (typeof e.secret !== "string" || !e.secret) return false;
    if (typeof e.at !== "number") return false;
    return now - e.at < PENDING_UNSUB_TTL_MS;
  });
}

function savePendingUnsub(list: PendingUnsub[]): void {
  try {
    if (list.length === 0) localStorage.removeItem(PENDING_UNSUB_KEY);
    else localStorage.setItem(PENDING_UNSUB_KEY, JSON.stringify(list));
  } catch {
    // Storage full / private mode — unsubscribe falls back to user manual notifications off, not worth failing removal
  }
}

/** Record a failed unsubscribe. Keep only the latest entry per secret. */
export function addPendingUnsub(entry: PendingUnsub): void {
  const rest = loadPendingUnsub(entry.at).filter((e) => e.secret !== entry.secret);
  savePendingUnsub([...rest, entry]);
}

/** Clear the entry after successful unsubscribe. */
export function dropPendingUnsub(secret: string, now: number): void {
  savePendingUnsub(loadPendingUnsub(now).filter((e) => e.secret !== secret));
}

// ── Removed pairings ───────────────────────────────────────────────────────────
//
// The Harmony shell keeps the last scanned pairing in its own preferences and re-injects it as
// `#k=…&boot=1` on every app start (mobile-harmony WebShell.ets). The page never tells the shell
// about removals, so without this list a removed device came straight back on the next launch.
// A boot re-inject whose secret is listed here is ignored; a fresh scan of the same code takes it
// off the list, because scanning again is the user asking for the device back.
//
// Lives in localStorage only: if the WebView's storage is wiped, the book is gone too, and then
// the shell's re-inject is exactly the recovery we want.

const REMOVED_SECRETS_KEY = "fleet-removed-secrets";
/** Bound the list: each entry is one explicit removal, so this is plenty. */
const REMOVED_SECRETS_MAX = 32;

export function loadRemovedSecrets(): string[] {
  const raw = readLocal(REMOVED_SECRETS_KEY);
  if (!raw) return [];
  try {
    const value: unknown = JSON.parse(raw);
    return Array.isArray(value) ? value.filter((s): s is string => typeof s === "string" && !!s) : [];
  } catch {
    return [];
  }
}

function saveRemovedSecrets(list: string[]): void {
  try {
    if (list.length === 0) localStorage.removeItem(REMOVED_SECRETS_KEY);
    else localStorage.setItem(REMOVED_SECRETS_KEY, JSON.stringify(list));
  } catch {
    // Storage full / private mode — worst case the shell's re-inject brings the device back
  }
}

/** Remember that the user removed these pairings. */
export function markSecretsRemoved(secrets: string[]): void {
  if (secrets.length === 0) return;
  const rest = loadRemovedSecrets().filter((s) => !secrets.includes(s));
  saveRemovedSecrets([...rest, ...secrets].slice(-REMOVED_SECRETS_MAX));
}

/** The user scanned this code again — it is wanted back. */
export function unmarkSecretRemoved(secret: string): void {
  const list = loadRemovedSecrets();
  if (list.includes(secret)) saveRemovedSecrets(list.filter((s) => s !== secret));
}

/** Clear all pairings ("re-pair" entry point). Also clear old keys, or the migration path
 *  above would resurrect them on next startup. */
export function clearBook(): void {
  try {
    localStorage.removeItem(BOOK_KEY);
    localStorage.removeItem(LEGACY_SECRET_KEY);
  } catch {
    // ignore
  }
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction("kv", "readwrite");
          tx.objectStore("kv").delete(BOOK_KEY);
          tx.objectStore("kv").delete(LEGACY_SECRET_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}
