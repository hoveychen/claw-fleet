import { describe, expect, it } from "vitest";
import mainSrc from "./main.tsx?raw";
import pushSrc from "./push.ts?raw";
import appSrc from "./App.tsx?raw";

// The constraint "webui artifact must not contain relay" has a subtle failure mode:
// the code uses dynamic import and is logically unreachable, yet Rollup still
// emits a relay-*.js chunk. Seen twice: first when main.tsx used hostMode's IS_WEBUI
// as a guard, second when push.ts guarded `import("./relay")` with SUPPORTS_PUSH —
// both times `resolveRelayBase` and `fleet-relay/hkdf/v1` appeared in the artifact.
//
// The cause: vite's define only replaces **literal** `import.meta.env.VITE_FLEET_HOST`.
// After a single const indirection, Rollup stops folding the condition, so it
// doesn't eliminate the chunk.
//
// So these condition checks must be written as the define's raw expression. This test
// guards exactly that — it reads source text because no behavior test will fail just
// because "the artifact has one extra chunk". Real artifact validation happens in P3's
// build check (grep dist-webui); this is its early sentinel.
describe("same-origin build must not include relay — guard patterns sensitive to constant folding", () => {
  it("main.tsx chooses transport layer using define expression directly, not via hostMode", () => {
    expect(mainSrc).toContain('import.meta.env.VITE_FLEET_HOST === "webui"');
    // Also guard the inverse: using IS_WEBUI for this guard would resurrect the relay chunk.
    expect(mainSrc).not.toMatch(/IS_WEBUI\s*[?&|]/);
  });

  // push.ts once dynamically imported the entire relay client to fetch the VAPID public key,
  // guarded by define's raw expression so Rollup would eliminate it. After multi-device support,
  // VAPID is **per relay** and the address is caller-provided, so that import doesn't exist
  // at all now — even more thorough. We just need to ensure it doesn't come back:
  // `./relayBase` is allowed (pure leaf module, no module load-time side effects, no WebSocket
  // or crypto), but `./relay` and `./relayCrypto` are forbidden.
  it("push.ts must not import relay client (only relayBase pure leaf is allowed)", () => {
    const imports = [...pushSrc.matchAll(/from\s+"([^"]+)"/g)].map((m) => m[1]);
    const dynamic = [...pushSrc.matchAll(/import\(\s*"([^"]+)"/g)].map((m) => m[1]);
    const relayish = [...imports, ...dynamic].filter((spec) => /(^|\/)relay/i.test(spec));
    expect(relayish).toEqual(["./relayBase"]);
  });

  // App.tsx once imported mock/relay.ts just to read a query param, and that file
  // extends RelayClient — one static import chain pulled the entire relay dependency tree into the same-origin artifact.
  it("App.tsx must not statically import any relay-side modules", () => {
    const imports = [...appSrc.matchAll(/from\s+"([^"]+)"/g)].map((m) => m[1]);
    expect(imports.filter((s) => /relay/i.test(s))).toEqual([]);
  });
});
