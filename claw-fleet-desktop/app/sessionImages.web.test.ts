// @vitest-environment jsdom
//
// Needs a `window.location` to resolve an origin against, hence jsdom.
import { describe, expect, it, vi } from "vitest";

/**
 * `hostEnv`'s flag is one-way by design — production code sets it once at boot
 * and never clears it — so switching hosts between cases means a fresh module
 * registry rather than a reset hook that only tests would ever call.
 */
async function loadFor(host: "desktop" | "web") {
  vi.resetModules();
  if (host === "web") (await import("./hostEnv")).markWebBuild();
  return import("./sessionImages");
}

describe("sessionImageUrl", () => {
  /**
   * `fleet-genimage://` is a Tauri custom protocol — registered on the webview,
   * unknown to a plain tab, so an `<img src>` built from it fails to load with
   * nothing but a net error to show for it. The same bytes are reachable over
   * HTTP at `/session_image?session=&name=`, the route RemoteBackend already
   * reads them through.
   */
  it("serves over HTTP in the browser build and the protocol on desktop", async () => {
    const id = "01a06fe0-ad88-7af1-954a-df3f0e6fa22d";
    const name = "exec-a6f72302.png";

    const desktop = await loadFor("desktop");
    expect(desktop.sessionImageUrl(id, name)).toBe(
      `fleet-genimage://localhost/${id}/${name}`,
    );

    const web = await loadFor("web");
    expect(web.sessionImageUrl(id, name)).toBe(
      `${window.location.origin}/session_image?session=${id}&name=${name}`,
    );
  });

  /**
   * The route decodes with `percent_decode_str`, which leaves `+` alone — so a
   * `URLSearchParams`-encoded space would be looked up verbatim and 404. Every
   * such image would render blank with no error surfaced.
   */
  it("percent-encodes spaces rather than form-encoding them", async () => {
    const web = await loadFor("web");
    const url = web.sessionImageUrl("thread one", "my shot.png");
    expect(url).toContain("session=thread%20one");
    expect(url).toContain("name=my%20shot.png");
    expect(url).not.toContain("+");
  });
});

describe("imageFileName", () => {
  it("takes the last segment of posix and windows paths alike", async () => {
    const { imageFileName } = await loadFor("desktop");
    expect(imageFileName("/Users/u/.codex/generated_images/t/exec-1.png")).toBe(
      "exec-1.png",
    );
    expect(imageFileName("C:\\Users\\u\\.codex\\generated_images\\t\\exec-1.png")).toBe(
      "exec-1.png",
    );
    expect(imageFileName("bare.png")).toBe("bare.png");
  });
});
