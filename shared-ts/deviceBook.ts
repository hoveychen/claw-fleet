// The paired-device book (`fleet-devices` in localStorage, mirrored to IndexedDB).
//
// mobile-web owns the book; the desktop bundle served at `<relay>/desktop/` shares
// the same origin and both reads it and, when opened with a `#k=` pairing link,
// writes to it. Everything that decides the record format lives here so the two
// pages cannot drift: a pairing taken on either one is the same record to the other.

/** Key for storing the book locally (localStorage and IDB share the same key name). */
export const DEVICE_BOOK_KEY = "fleet-devices";
/** IndexedDB database/store mobile-web mirrors the book (and other small values) into. */
export const DEVICE_DB_NAME = "fleet-relay";
export const DEVICE_DB_STORE = "kv";

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

/** Open the IndexedDB mirror. Shared with mobile-web's secretStore/sessionCache,
 *  which keep other keys in this same DB/store. */
export function openDeviceDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DEVICE_DB_NAME, 1);
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(DEVICE_DB_STORE)) {
        req.result.createObjectStore(DEVICE_DB_STORE);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/** Dual write. localStorage is the synchronous source of truth, IDB is a fire-and-forget mirror. */
export function persistBook(book: DeviceBook): void {
  const json = JSON.stringify(book);
  try {
    localStorage.setItem(DEVICE_BOOK_KEY, json);
  } catch {
    // Storage full / private mode — IDB below might still succeed
  }
  void openDeviceDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(DEVICE_DB_STORE, "readwrite");
          tx.objectStore(DEVICE_DB_STORE).put(json, DEVICE_BOOK_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}
