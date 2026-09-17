import { describe, expect, it } from "vitest";
// Use `?raw` instead of node:fs—this tsconfig only includes vite/client types, not node types.
import src from "./httpTransport.ts?raw";

// The constraint "webui does not depend on relay" cannot be enforced by code review:
// it breaks when someone casually adds `import { someConstant } from "./relay"` in httpTransport.ts—type check passes,
// unit tests pass, UI still runs normally. The only consequence is the browser build gains a relay client
// that executes resolveRelayBase() at module load time, parsing an address it will never use.
// No behavior test would fail because of this.
//
// So this guard checks source text, not behavior. It deliberately checks naively:
// adjacent imports like `./relayCrypto` and `./relayHttpBase` are relay-side modules too, blocked together.
describe("httpTransport 与 relay 的隔离", () => {
  it("httpTransport.ts 不得 import 任何 relay 侧模块", () => {
    const imports = [...src.matchAll(/from\s+"([^"]+)"/g)].map((m) => m[1]);

    const relayish = imports.filter((spec) => /(^|\/)relay/i.test(spec));

    expect(relayish).toEqual([]);
    // Also verify it actually uses the shared layer—a file that imports nothing could still pass the check above,
    // which would indicate it reimplemented error handling instead of reusing the shared layer.
    expect(imports).toContain("./transport");
  });
});
