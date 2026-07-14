import { describe, expect, it } from "vitest";
import { deriveKeys, isSealed, open, seal, type SealedBox } from "./relayCrypto";

// Frozen cross-endpoint vector — MUST match
// claw-fleet-core/src/relay_crypto.rs (`frozen_derivation_vector` /
// `frozen_sealed_vector_cross_decrypt`). If either side changes a
// salt/info/AAD byte, the derived material diverges and these fail.
const VECTOR_SECRET = "fleet-relay-test-vector-secret-01";
const VECTOR_CHANNEL_TOKEN =
  "8095a4c3f2f314a969bc27c09592f694ec65d7155c5ffdd1666aa175c46bf4df";
// A ciphertext the RUST side produced (fixed all-zero nonce, test only).
const RUST_SEALED: SealedBox = {
  enc: "box",
  iv: "AAAAAAAAAAAAAAAA",
  ct: "KsdkF5r5NnPWZQZAuXoc6xkhMFX2E0j5wTu8WKJvGY21lDRTiUAwuSWET1lNbSK8Ex4AtjMw2LnSUj8=",
};
const RUST_PLAINTEXT = '{"event":"decision_created","kind":"guard"}';

describe("relayCrypto", () => {
  it("derives the frozen channel token (matches Rust)", async () => {
    const keys = await deriveKeys(VECTOR_SECRET);
    expect(keys.channelToken).toBe(VECTOR_CHANNEL_TOKEN);
  });

  it("decrypts a ciphertext sealed by the Rust peer (cross-endpoint AEAD interop)", async () => {
    const keys = await deriveKeys(VECTOR_SECRET);
    const plaintext = await open(keys.encKey, RUST_SEALED);
    expect(plaintext).toBe(RUST_PLAINTEXT);
  });

  it("round-trips seal → open", async () => {
    const keys = await deriveKeys("roundtrip-secret-xxxxxxxxxxxxxxxx");
    const msg = '{"event":"answer","kind":"guard","id":"abc"}';
    const sealed = await seal(keys.encKey, msg);
    expect(sealed.enc).toBe("box");
    expect(await open(keys.encKey, sealed)).toBe(msg);
  });

  it("uses a fresh nonce each seal", async () => {
    const keys = await deriveKeys("nonce-secret-xxxxxxxxxxxxxxxxxxxx");
    const a = await seal(keys.encKey, "same plaintext");
    const b = await seal(keys.encKey, "same plaintext");
    expect(a.iv).not.toBe(b.iv);
    expect(a.ct).not.toBe(b.ct);
  });

  it("rejects a wrong key", async () => {
    const good = await deriveKeys("good-secret-xxxxxxxxxxxxxxxxxxxx");
    const bad = await deriveKeys("bad-secret-xxxxxxxxxxxxxxxxxxxxx");
    const sealed = await seal(good.encKey, "secret data");
    await expect(open(bad.encKey, sealed)).rejects.toThrow();
  });

  it("rejects tampered ciphertext (GCM tag)", async () => {
    const keys = await deriveKeys("tamper-secret-xxxxxxxxxxxxxxxxxx");
    const sealed = await seal(keys.encKey, "secret data");
    const bytes = atob(sealed.ct)
      .split("")
      .map((c) => c.charCodeAt(0));
    bytes[0] ^= 0x01;
    const tampered: SealedBox = {
      ...sealed,
      ct: btoa(String.fromCharCode(...bytes)),
    };
    await expect(open(keys.encKey, tampered)).rejects.toThrow();
  });

  it("isSealed discriminates envelopes", () => {
    expect(isSealed({ enc: "box", iv: "x", ct: "y" })).toBe(true);
    expect(isSealed({ event: "answer" })).toBe(false);
    expect(isSealed(null)).toBe(false);
  });
});
