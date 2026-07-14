// End-to-end encryption for the mobile relay (方案A). The peer of
// claw-fleet-core/src/relay_crypto.rs — every parameter here must match that
// module byte-for-byte or pairing silently fails. Implemented on the browser's
// native SubtleCrypto so there is zero JS crypto dependency.
//
// The pairing secret (from the QR fragment `#k=…`, never sent to the server)
// derives two independent keys via HKDF-SHA256 with domain separation:
//   * channelToken — put in the WS `auth` frame instead of the raw secret, so
//     the relay routes by a one-way function of the secret and never learns it.
//   * encKey (AES-256-GCM) — never leaves this device; seals/opens `msg`
//     payloads into the `{ enc:"box", iv, ct }` envelope the relay forwards
//     verbatim.
//
// See relay_crypto.rs for the shared frozen test vector.

const enc = new TextEncoder();
const dec = new TextDecoder();

// Must equal the Rust constants exactly (UTF-8 bytes).
const HKDF_SALT = enc.encode("fleet-relay/hkdf/v1");
const INFO_CHANNEL = enc.encode("fleet-relay/channel/v1");
const INFO_ENC = enc.encode("fleet-relay/enc/v1");
const AAD = enc.encode("fleet-relay/msg/v1");

/** The sealed wire envelope carried as a `msg` payload. */
export interface SealedBox {
  enc: "box";
  iv: string; // base64 of the 12-byte nonce
  ct: string; // base64 of ciphertext||tag (16-byte GCM tag appended)
}

/** Whether a parsed payload looks like a sealed envelope. */
export function isSealed(v: unknown): v is SealedBox {
  return (
    typeof v === "object" &&
    v !== null &&
    (v as { enc?: unknown }).enc === "box" &&
    typeof (v as { iv?: unknown }).iv === "string" &&
    typeof (v as { ct?: unknown }).ct === "string"
  );
}

/** Keys derived from the pairing secret. `channelToken` is safe to hand the
 *  relay; `encKey` is a non-extractable CryptoKey that stays on this device. */
export interface RelayKeys {
  channelToken: string;
  encKey: CryptoKey;
}

function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
}

function b64encode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Derive the relay keys from the pairing secret. Deterministic: this device
 *  and the desktop that share the same secret derive identical keys. */
export async function deriveKeys(secret: string): Promise<RelayKeys> {
  const base = await crypto.subtle.importKey("raw", enc.encode(secret), "HKDF", false, [
    "deriveBits",
  ]);
  const channelBits = await crypto.subtle.deriveBits(
    { name: "HKDF", hash: "SHA-256", salt: HKDF_SALT, info: INFO_CHANNEL },
    base,
    256,
  );
  const encBits = await crypto.subtle.deriveBits(
    { name: "HKDF", hash: "SHA-256", salt: HKDF_SALT, info: INFO_ENC },
    base,
    256,
  );
  const encKey = await crypto.subtle.importKey(
    "raw",
    encBits,
    { name: "AES-GCM", length: 256 },
    false, // non-extractable
    ["encrypt", "decrypt"],
  );
  return { channelToken: toHex(new Uint8Array(channelBits)), encKey };
}

/** Encrypt a plaintext string under `encKey` with a fresh random 12-byte IV. */
export async function seal(encKey: CryptoKey, plaintext: string): Promise<SealedBox> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: AAD, tagLength: 128 },
    encKey,
    enc.encode(plaintext),
  );
  return { enc: "box", iv: b64encode(iv), ct: b64encode(new Uint8Array(ct)) };
}

/** Decrypt a sealed envelope back into its plaintext string. Rejects on a bad
 *  key, tampered ciphertext, or malformed base64 — the caller drops the frame. */
export async function open(encKey: CryptoKey, sealed: SealedBox): Promise<string> {
  const iv = b64decode(sealed.iv);
  if (iv.length !== 12) throw new Error(`iv must be 12 bytes, got ${iv.length}`);
  const ct = b64decode(sealed.ct);
  const pt = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv, additionalData: AAD, tagLength: 128 },
    encKey,
    ct,
  );
  return dec.decode(pt);
}
