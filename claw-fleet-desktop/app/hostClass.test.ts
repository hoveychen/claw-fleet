import { describe, it, expect } from "vitest";
import { hostClasses } from "./hostClass";

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WIN_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36";

describe("hostClasses", () => {
  it("tags macOS without os-windows", () => {
    // App.css scopes the ::-webkit-scrollbar override to .os-windows. If macOS
    // ever picked that class up, every list would lose the native overlay
    // scrollbar and grow a permanent 6px gutter.
    const cls = hostClasses(MAC_UA, true);
    expect(cls).toContain("os-macos");
    expect(cls).not.toContain("os-windows");
  });

  it("tags Windows without os-macos", () => {
    const cls = hostClasses(WIN_UA, true);
    expect(cls).toContain("os-windows");
    expect(cls).not.toContain("os-macos");
  });

  it("omits tauri-host outside a Tauri webview", () => {
    expect(hostClasses(MAC_UA, false)).not.toContain("tauri-host");
    expect(hostClasses(MAC_UA, true)).toContain("tauri-host");
  });
});
