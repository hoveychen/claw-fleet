import { cryptoFramework } from '@kit.CryptoArchitectureKit';
import { util } from '@kit.ArkTS';
import { bytesToHex, hkdf32 } from './Hkdf';

/// End-to-end encryption for the mobile relay (方案A) — the native ArkTS peer of
/// mobile-web/src/relayCrypto.ts and claw-fleet-core/src/relay_crypto.rs. Every
/// parameter here must match those byte-for-byte or pairing silently fails.
///
/// The pairing secret derives two independent keys via HKDF-SHA256 (see Hkdf.ets,
/// already proven byte-aligned by the on-device P5 push flow):
///   * channelToken — hex, put in the WS `auth` frame instead of the raw secret.
///   * encKey — 32-byte AES-256-GCM key; never leaves the device. Seals/opens
///     `msg` payloads into the `{ enc:"box", iv, ct }` envelope the relay
///     forwards verbatim. `ct` = ciphertext || 16-byte GCM tag (both the Rust
///     `aes-gcm` crate and WebCrypto append the tag; HarmonyOS returns the tag
///     from the encrypt `doFinal`, so we append it ourselves).
///
/// See relay_crypto.rs's frozen test vector — `selfTest()` below checks against
/// the exact same values.

// Must equal the Rust/JS constants exactly (UTF-8 bytes).
const INFO_ENC = 'fleet-relay/enc/v1';
const AAD_STR = 'fleet-relay/msg/v1';
const GCM_TAG_LEN = 16;
const GCM_IV_LEN = 12;

/// The sealed wire envelope carried as a `msg` payload.
export interface SealedBox {
  enc: string; // always the literal "box"
  iv: string; // base64 of the 12-byte nonce
  ct: string; // base64 of ciphertext || 16-byte tag
  z?: boolean; // set by the desktop when the plaintext was gzipped before sealing
}

/// Keys derived from the pairing secret.
export interface RelayKeys {
  channelToken: string;
  encKey: Uint8Array;
}

const b64 = new util.Base64Helper();

function utf8(s: string): Uint8Array {
  return new util.TextEncoder().encodeInto(s);
}

function decodeUtf8(bytes: Uint8Array): string {
  return util.TextDecoder.create('utf-8').decodeToString(bytes);
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

const AAD = utf8(AAD_STR);

/// Whether a parsed payload looks like a sealed envelope (`enc == "box"`).
export function isSealed(enc?: string, iv?: string, ct?: string): boolean {
  return enc === 'box' && typeof iv === 'string' && typeof ct === 'string';
}

/// Derive the relay keys from the pairing secret. Deterministic across the
/// desktop and every paired device that share the secret.
export async function deriveKeys(secret: string): Promise<RelayKeys> {
  const channel = await hkdf32(secret, 'fleet-relay/channel/v1');
  const enc = await hkdf32(secret, INFO_ENC);
  return { channelToken: bytesToHex(channel), encKey: enc };
}

async function importAesKey(encKey: Uint8Array): Promise<cryptoFramework.SymKey> {
  const gen = cryptoFramework.createSymKeyGenerator('AES256');
  const blob: cryptoFramework.DataBlob = { data: encKey };
  return gen.convertKey(blob);
}

function gcmParams(iv: Uint8Array, authTag: Uint8Array): cryptoFramework.GcmParamsSpec {
  const ivBlob: cryptoFramework.DataBlob = { data: iv };
  const aadBlob: cryptoFramework.DataBlob = { data: AAD };
  const tagBlob: cryptoFramework.DataBlob = { data: authTag };
  return { iv: ivBlob, aad: aadBlob, authTag: tagBlob, algName: 'GcmParamsSpec' };
}

/// Encrypt raw bytes under `encKey` with a fresh random 12-byte IV. Returns the
/// sealed envelope; `ct` carries ciphertext||tag. Peer of Rust `seal`.
export async function sealBytes(encKey: Uint8Array, plaintext: Uint8Array): Promise<SealedBox> {
  const iv = cryptoFramework.createRandom().generateRandomSync(GCM_IV_LEN).data;
  return sealBytesIv(encKey, plaintext, iv);
}

/// Seal with a caller-supplied IV — used only by selfTest to check harmony's
/// encrypt output byte-for-byte against the Rust frozen vector (the one direction
/// the PASS path never exercised). Never use a fixed IV in production: GCM
/// security requires a unique nonce per message.
async function sealBytesIv(encKey: Uint8Array, plaintext: Uint8Array, iv: Uint8Array): Promise<SealedBox> {
  const symKey = await importAesKey(encKey);
  const cipher = cryptoFramework.createCipher('AES256|GCM|NoPadding');
  // authTag on encrypt is an output placeholder; its length must be the tag len.
  await cipher.init(cryptoFramework.CryptoMode.ENCRYPT_MODE, symKey, gcmParams(iv, new Uint8Array(GCM_TAG_LEN)));
  const body: cryptoFramework.DataBlob = { data: plaintext };
  const cipherOut = await cipher.update(body);
  // For GCM, doFinal returns the 16-byte authentication tag.
  const tagOut = await cipher.doFinal(null);
  const tag = tagOut?.data ?? new Uint8Array(0);
  const ctPlusTag = concat(cipherOut.data, tag);
  return { enc: 'box', iv: b64.encodeToStringSync(iv), ct: b64.encodeToStringSync(ctPlusTag) };
}

export async function sealString(encKey: Uint8Array, plaintext: string): Promise<SealedBox> {
  return sealBytes(encKey, utf8(plaintext));
}

/// Decrypt a sealed envelope back into raw plaintext bytes. Peer of Rust `open`.
/// Rejects on a bad key, tampered ciphertext (GCM tag), or malformed base64.
export async function openBytes(encKey: Uint8Array, sealed: SealedBox): Promise<Uint8Array> {
  const iv = b64.decodeSync(sealed.iv);
  if (iv.length !== GCM_IV_LEN) {
    throw new Error(`iv must be ${GCM_IV_LEN} bytes, got ${iv.length}`);
  }
  const ctPlusTag = b64.decodeSync(sealed.ct);
  if (ctPlusTag.length < GCM_TAG_LEN) {
    throw new Error(`ciphertext shorter than the ${GCM_TAG_LEN}-byte tag`);
  }
  const body = ctPlusTag.slice(0, ctPlusTag.length - GCM_TAG_LEN);
  const tag = ctPlusTag.slice(ctPlusTag.length - GCM_TAG_LEN);
  const symKey = await importAesKey(encKey);
  const cipher = cryptoFramework.createCipher('AES256|GCM|NoPadding');
  await cipher.init(cryptoFramework.CryptoMode.DECRYPT_MODE, symKey, gcmParams(iv, tag));
  const bodyBlob: cryptoFramework.DataBlob = { data: body };
  const plainOut = await cipher.update(bodyBlob);
  const finalOut = await cipher.doFinal(null);
  const tail = finalOut?.data ?? new Uint8Array(0);
  return tail.length === 0 ? plainOut.data : concat(plainOut.data, tail);
}

export async function openString(encKey: Uint8Array, sealed: SealedBox): Promise<string> {
  return decodeUtf8(await openBytes(encKey, sealed));
}

/// Runtime byte-alignment check against relay_crypto.rs's frozen vector. Returns
/// 'PASS' or a 'FAIL …' diagnostic. Call once on device (see EntryAbility) and
/// read the verdict from hilog — the only way to prove AES-GCM interop without a
/// second endpoint. Compile-green alone does NOT prove the cryptoFramework calls
/// produce Rust/WebCrypto-compatible output.
export async function selfTest(): Promise<string> {
  const VECTOR_SECRET = 'fleet-relay-test-vector-secret-01';
  const VECTOR_CHANNEL = '8095a4c3f2f314a969bc27c09592f694ec65d7155c5ffdd1666aa175c46bf4df';
  const RUST_IV = 'AAAAAAAAAAAAAAAA';
  const RUST_CT = 'KsdkF5r5NnPWZQZAuXoc6xkhMFX2E0j5wTu8WKJvGY21lDRTiUAwuSWET1lNbSK8Ex4AtjMw2LnSUj8=';
  const RUST_PLAINTEXT = '{"event":"decision_created","kind":"guard"}';
  try {
    const keys = await deriveKeys(VECTOR_SECRET);
    if (keys.channelToken !== VECTOR_CHANNEL) {
      return `FAIL channel_token=${keys.channelToken}`;
    }
    const rustSealed: SealedBox = { enc: 'box', iv: RUST_IV, ct: RUST_CT };
    const opened = await openString(keys.encKey, rustSealed);
    if (opened !== RUST_PLAINTEXT) {
      return `FAIL cross-open=${opened}`;
    }
    const box = await sealString(keys.encKey, RUST_PLAINTEXT);
    const back = await openString(keys.encKey, box);
    if (back !== RUST_PLAINTEXT) {
      return `FAIL roundtrip=${back}`;
    }
    // Definitive encrypt-interop check: seal the frozen plaintext with the frozen
    // IV and compare to the Rust ct. This is the ONLY assertion proving harmony's
    // seal is desktop-openable (the request path depends on it).
    const fixed = await sealBytesIv(keys.encKey, utf8(RUST_PLAINTEXT), b64.decodeSync(RUST_IV));
    if (fixed.ct !== RUST_CT) {
      return `FAIL seal-compat ct=${fixed.ct}`;
    }
    return 'PASS';
  } catch (e) {
    return `FAIL threw ${JSON.stringify(e)}`;
  }
}
