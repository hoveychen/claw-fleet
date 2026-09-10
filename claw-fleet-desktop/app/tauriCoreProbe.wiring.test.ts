/**
 * Guards the *wiring* that makes `tauriCoreProbe.ts` reachable.
 *
 * The probe it replaced was not broken by a bad implementation — it was
 * correct and simply never installed, and nothing failed when that happened.
 * It stayed a no-op through every build for two days, and the only way to
 * notice was to go looking for missing log lines. So the resolution rules get
 * a test of their own: if an alias or the importer rule is dropped, this goes
 * red instead of the log going quiet.
 */
import { describe, expect, it } from "vitest";
import { sep } from "path";

import { TAURI_CORE_SHIM, tauriCoreProbe, tauriCoreProbePlugin } from "../vite.config";

const inApiPackage = (file: string) =>
  ["", "node_modules", "@tauri-apps", "api", file].join(sep);

function aliasFor(specifier: string): string | null {
  for (const { find, replacement } of tauriCoreProbe) {
    if (find.test(specifier)) return replacement;
  }
  return null;
}

describe("tauri core probe wiring", () => {
  it("sends the bare specifier — what app code and the plugin packages use — to the shim", () => {
    expect(aliasFor("@tauri-apps/api/core")).toBe(TAURI_CORE_SHIM);
  });

  it("does not send the shim's own escape hatch back to the shim", () => {
    const real = aliasFor("@tauri-apps/api/core-real");
    expect(real).not.toBe(TAURI_CORE_SHIM);
    expect(real).toMatch(/@tauri-apps.api.core\.js$/);
  });

  it("leaves unrelated api modules alone", () => {
    expect(aliasFor("@tauri-apps/api/event")).toBeNull();
    expect(aliasFor("@tauri-apps/api/path")).toBeNull();
  });

  const plugin = tauriCoreProbePlugin();

  it("redirects the api package's own relative ./core.js imports", () => {
    // `listen`/`emit` reach invoke this way; the bare-specifier alias misses it.
    expect(plugin.resolveId("./core.js", inApiPackage("event.js"))).toBe(TAURI_CORE_SHIM);
    expect(plugin.resolveId("./core.js", inApiPackage("window.js"))).toBe(TAURI_CORE_SHIM);
  });

  it("does not redirect a ./core.js that belongs to some other package", () => {
    expect(
      plugin.resolveId("./core.js", ["", "node_modules", "some-lib", "index.js"].join(sep)),
    ).toBeNull();
  });

  it("does not redirect anything else, entrypoint-less imports included", () => {
    expect(plugin.resolveId("./event.js", inApiPackage("index.js"))).toBeNull();
    expect(plugin.resolveId("./core.js", undefined)).toBeNull();
  });
});
