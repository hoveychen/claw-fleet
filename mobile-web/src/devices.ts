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

import { parseRelayParam } from "./relayBase";
import { extractSecretFromUrl, openDb } from "./secretStore";

/** Key for storing the book locally (localStorage and IDB share the same key name). */
const BOOK_KEY = "fleet-devices";
/** Key from single-device era. Read once during migration, never written again. */
const LEGACY_SECRET_KEY = "fleet-relay-secret";


/** How this device connects: via relay pairing, or direct HTTP backend.
 *
 *  `http` has **no corresponding "add device" entry** — it carries the same-origin deployment:
 *  in same-origin products (`fleet webui` / cloud container `/m/`), the book stays empty; the App
 *  synthesizes an http device with empty baseUrl to mean "ask the origin that served this page"
 *  (the App's `SAME_ORIGIN_DEVICE`), so "dispatch transport by device kind" covers same-origin
 *  without a special case. Old http records in the book were added when manual direct connection
 *  existed; deserialization still accepts them — users who added one shouldn't lose connectivity
 *  just because the entry point was removed. */
export type DeviceKind = "relay" | "http";

interface DeviceCommon {
  /** Stable id generated locally. Use this instead of the secret as a foreign key, so the
   *  secret doesn't leak into cache keys, route params, or React keys (visible in logs/devtools). */
  id: string;
  /** User-editable display name. */
  label: string;
  /** Whether the name was auto-generated. `true` = not yet named by user; when the desktop
   *  reports its hostname, it can override this (`applyHostIdentity`); once the user renames,
   *  this becomes permanently `false`.
   *
   *  Store this boolean rather than "check if the name looks like the default", because the
   *  latter leads to false positives: a user who genuinely named their machine "Device 2"
   *  would have it reset on reconnect. */
  auto?: boolean;
  /** Platform string self-reported by the host (`macos` / `windows` / `linux` etc., see
   *  `host_identity.rs` in core). Used only for icon selection; absent if never connected. */
  platform?: string;
  addedAt: number;
}

/** Desktop connected via relay pairing. */
export interface RelayDevice extends DeviceCommon {
  kind: "relay";
  /** Pairing secret. channelToken and encKey are both HKDF-derived from this (relayCrypto.ts). */
  secret: string;
  /** The relay origin this pairing targets; `null` means use build default. */
  relayBase: string | null;
}

/** Direct HTTP host (`fleet webui` or cloud container). */
export interface HttpDevice extends DeviceCommon {
  kind: "http";
  /** Host address (origin, may include path prefix). Cross-origin access, so must be absolute. */
  baseUrl: string;
  /** Access token. Server accepts both `Authorization: Bearer` and `?token=` (the latter for
   *  EventSource, which can't carry headers). `null` = endpoint has no token gate. */
  token: string | null;
}

/** A registered device. */
export type PairedDevice = RelayDevice | HttpDevice;


export interface DeviceBook {
  devices: PairedDevice[];
  /** Current scoped device (wiki / usage pages show this one). `null` = no devices paired. */
  activeId: string | null;
}

export function emptyBook(): DeviceBook {
  return { devices: [], activeId: null };
}

/** Lenient parsing. Storage may contain entries from old versions, modified by other tools,
 *  or incomplete — any malformed record is dropped alone, not the whole book (that would silently
 *  wipe all pairings). */
export function parseBook(raw: unknown): DeviceBook | null {
  let value = raw;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (typeof value !== "object" || value === null) return null;
  const list = (value as { devices?: unknown }).devices;
  if (!Array.isArray(list)) return null;
  const devices: PairedDevice[] = [];
  for (const item of list) {
    if (typeof item !== "object" || item === null) continue;
    const d = item as Record<string, unknown>;
    if (typeof d.id !== "string" || !d.id) continue;
    const label = typeof d.label === "string" ? d.label : "";
    const addedAt = typeof d.addedAt === "number" ? d.addedAt : 0;
    const platform = typeof d.platform === "string" && d.platform ? d.platform : undefined;
    // `auto` is a new field. Old records lack it, but they're exactly the ones this change
    // rescues (all named "Device 1", "Device 2" etc.), so infer on absence: if it looks like
    // a default name, treat it as not yet named by the user.
    const auto = typeof d.auto === "boolean" ? d.auto : looksAutoLabel(label);
    // Records without `kind` predate relay-only days — they're all relay devices.
    // Use "has secret" rather than "kind absent" as the check, so a record missing both kind
    // and secret is dropped, not turned into an unreachable ghost device.
    if (d.kind === "http") {
      if (typeof d.baseUrl !== "string" || !d.baseUrl) continue;
      devices.push({
        kind: "http",
        id: d.id,
        label,
        auto,
        platform,
        addedAt,
        baseUrl: d.baseUrl,
        token: typeof d.token === "string" && d.token ? d.token : null,
      });
      continue;
    }
    if (typeof d.secret !== "string" || !d.secret) continue;
    devices.push({
      kind: "relay",
      id: d.id,
      label,
      auto,
      platform,
      addedAt,
      secret: d.secret,
      relayBase: typeof d.relayBase === "string" ? d.relayBase : null,
    });
  }
  if (devices.length === 0) return null;
  const activeRaw = (value as { activeId?: unknown }).activeId;
  const activeId =
    typeof activeRaw === "string" && devices.some((d) => d.id === activeRaw)
      ? activeRaw
      : devices[0].id;
  return { devices, activeId };
}

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

/** Does this label look like an auto-generated default ("Device 2")?
 *
 *  **Use only when migrating old records**: the `auto` field is new; old books lack it.
 *  This heuristic is the only way to distinguish "never named by user" from "user-chosen name".
 *  Once the field exists, read it directly. */

export function looksAutoLabel(label: string): boolean {
  const trimmed = label.trim();
  return trimmed === "" || /^(设备|Device)\s*\d+$/i.test(trimmed) || looksMacAddressLabel(trimmed);
}

/** Is this label a bare MAC address (`de:e8:92:d6:ca:71`)?
 *
 *  Such a label is never something a person typed — it is what a host reported as its hostname
 *  when the transient hostname had drifted to the network-assigned one (see the `scutil` comment
 *  in core's `host_identity.rs`). The host side no longer reports those, but a phone that paired
 *  before that fix still has the MAC address stored as the device's name, and `applyHostIdentity`
 *  would normally protect it as a user-chosen name. Recognizing the shape is what lets the record
 *  heal itself the next time the desktop reports a real name. */
export function looksMacAddressLabel(label: string): boolean {
  return /^[0-9a-f]{2}([:-][0-9a-f]{2}){5}$/i.test(label.trim());
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

/** Default name for the next device: `<prefix> N` where N is "smallest unused number",
 *  so deleting one and adding another doesn't cause collisions. */
export function nextDeviceLabel(book: DeviceBook, prefix: string): string {
  const used = new Set(book.devices.map((d) => d.label));
  for (let n = 1; ; n++) {
    const candidate = `${prefix} ${n}`;
    if (!used.has(candidate)) return candidate;
  }
}

export interface AddDeviceInput {
  secret: string;
  /** The relay this pairing targets; omit/`null` = use build default. */
  relayBase?: string | null;

  label: string;
  id: string;
  now: number;
}

export interface AddDeviceResult {
  book: DeviceBook;
  device: PairedDevice;
  /** This secret was already registered — the same QR code was scanned again. */
  deduped: boolean;
}


/** Add a device (or recognize it was already paired). Both add and rescan make it active —
 *  the user just scanned, so they want to see that device.
 *
 *  Dedup by **secret**, not channelToken: token is HKDF-derived from secret (relayCrypto.ts),
 *  they're one-to-one, and token derivation is async SubtleCrypto. Comparing secrets gives
 *  the same result and keeps this function purely synchronous.
 *
 *  On rescan **keep the old label** (user may have renamed it), but update relayBase —
 *  the latter says "which relay this pairing now hangs on"; when the desktop changes relay
 *  address and re-issues a QR, the new one is correct. */
export function addDevice(book: DeviceBook, input: AddDeviceInput): AddDeviceResult {
  const existing = book.devices.find(
    (d): d is RelayDevice => d.kind === "relay" && d.secret === input.secret,
  );
  if (existing) {
    const relayBase = input.relayBase ?? existing.relayBase;
    const updated: PairedDevice = { ...existing, relayBase };
    return {
      book: {
        devices: book.devices.map((d) => (d.id === existing.id ? updated : d)),
        activeId: existing.id,
      },
      device: updated,
      deduped: true,
    };
  }
  const device: PairedDevice = {
    kind: "relay",
    id: input.id,
    label: input.label,
    // Freshly scanned names are always "Device N" — the desktop hasn't had a chance to report
    // its hostname yet. Mark as auto, let it be overridden when connected (applyHostIdentity).
    auto: true,
    secret: input.secret,
    relayBase: input.relayBase ?? null,
    addedAt: input.now,
  };
  return {
    book: { devices: [...book.devices, device], activeId: device.id },
    device,
    deduped: false,
  };
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

/** Dual write. localStorage is the synchronous source of truth, IDB is a fire-and-forget mirror. */
export function persistBook(book: DeviceBook): void {
  const json = JSON.stringify(book);
  try {
    localStorage.setItem(BOOK_KEY, json);
  } catch {
    // Storage full / private mode — IDB below might still succeed
  }
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction("kv", "readwrite");
          tx.objectStore("kv").put(json, BOOK_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
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
  persistBook(next);
  return { book: next, device, added: !deduped };
}

/** "Add device" from address bar fragment: `#k=<secret>&relay=<url>` — relay-paired desktop
 *  (this is what desktop QR codes contain).
 *
 *  All three clients have only this path: system camera opens relay-hosted PWA, installed PWA
 *  reopens, or native shell scans and passes the URL to the page. */
export type HashPairing = {
  kind: "relay";
  secret: string;
  relayBase: string | null;
  boot: boolean;
};

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

  const secret = extractSecretFromUrl(hash);
  if (secret) {
    const relayBase = parseRelayParam(hash);
    const boot = /[#&]boot=1\b/.test(hash);
    scrub();
    return { kind: "relay", secret, relayBase, boot };
  }

  return null;
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
