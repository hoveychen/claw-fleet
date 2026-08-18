import { cryptoFramework } from '@kit.CryptoArchitectureKit';
import { util } from '@kit.ArkTS';

/// Derives the relay **channel token** from the pairing secret, byte-for-byte
/// compatible with claw-fleet-core::relay_crypto::derive_keys (HKDF-SHA256).
///
/// The relay routes by `SHA256(auth.secret)`, and both the desktop and this
/// atomic service put the *derived* channel token — not the raw pairing secret
/// — in the `auth` frame, so we must reproduce the exact HKDF parameters or the
/// service lands on a different channel and its push subscription never reaches
/// the desktop.
///
/// HKDF-Expand for a 32-byte output is a single block (T(1)), so we implement
/// HKDF on top of HMAC-SHA256, and HMAC on top of the SHA-256 digest primitive
/// (the most stable crypto API surface) rather than the KDF/Mac helpers.

// Must match relay_crypto.rs exactly.
const HKDF_SALT = 'fleet-relay/hkdf/v1';
const INFO_CHANNEL = 'fleet-relay/channel/v1';
const SHA256_BLOCK = 64;
const SHA256_LEN = 32;

function utf8(s: string): Uint8Array {
  return new util.TextEncoder().encodeInto(s);
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function toHex(bytes: Uint8Array): string {
  let s = '';
  for (let i = 0; i < bytes.length; i++) {
    s += bytes[i].toString(16).padStart(2, '0');
  }
  return s;
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  const md = cryptoFramework.createMd('SHA256');
  await md.update({ data });
  const digest = await md.digest();
  return digest.data;
}

/// HMAC-SHA256 built from the digest primitive (RFC 2104).
async function hmacSha256(key: Uint8Array, msg: Uint8Array): Promise<Uint8Array> {
  let k = key;
  if (k.length > SHA256_BLOCK) {
    k = await sha256(k);
  }
  const block = new Uint8Array(SHA256_BLOCK); // zero-padded to the block size
  block.set(k, 0);
  const ipad = new Uint8Array(SHA256_BLOCK);
  const opad = new Uint8Array(SHA256_BLOCK);
  for (let i = 0; i < SHA256_BLOCK; i++) {
    ipad[i] = block[i] ^ 0x36;
    opad[i] = block[i] ^ 0x5c;
  }
  const inner = await sha256(concat(ipad, msg));
  return await sha256(concat(opad, inner));
}

/// HKDF-SHA256 for a single-block (32-byte) output, keyed by the pairing secret.
/// `okm = HMAC(PRK, info || 0x01)` where `PRK = HMAC(HKDF_SALT, secret)`.
/// Byte-for-byte the peer of claw-fleet-core::relay_crypto::derive_keys.
export async function hkdf32(secret: string, info: string): Promise<Uint8Array> {
  const prk = await hmacSha256(utf8(HKDF_SALT), utf8(secret));
  const t1Input = concat(utf8(info), new Uint8Array([0x01]));
  const okm = await hmacSha256(prk, t1Input);
  return okm.subarray(0, SHA256_LEN);
}

/// Lowercase hex of a byte array.
export function bytesToHex(bytes: Uint8Array): string {
  return toHex(bytes);
}

/// channel_token = hex( HKDF-SHA256(secret, salt, INFO_CHANNEL, 32) ).
export async function deriveChannelToken(secret: string): Promise<string> {
  return toHex(await hkdf32(secret, INFO_CHANNEL));
}
