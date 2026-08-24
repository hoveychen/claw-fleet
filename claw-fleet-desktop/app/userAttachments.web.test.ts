// @vitest-environment jsdom
//
// Split from `userAttachments.test.ts` (which runs in the node environment)
// because these cases need a `window.location` to resolve an origin against.
import { describe, expect, it, vi } from "vitest";

/**
 * `hostEnv`'s flag is one-way by design — production code sets it once at boot
 * and never clears it — so switching hosts between cases means a fresh module
 * registry rather than a reset hook that only tests would ever call.
 */
async function loadFor(host: "desktop" | "web") {
  vi.resetModules();
  if (host === "web") (await import("./hostEnv")).markWebBuild();
  return import("./userAttachments");
}

describe("userAttachmentUrl in the browser build", () => {
  /**
   * `fleet-attachment://` is a Tauri custom protocol — registered on the
   * webview, unknown to a plain tab, so an `<img src>` built from it fails to
   * load with nothing but a net error to show for it. The same bytes are
   * already reachable over HTTP: `/user_attachment?key=&name=` is the route
   * `RemoteBackend` and the mobile relay read attachments through.
   */
  it("serves the store over HTTP instead of the custom protocol", async () => {
    const p = "/home/u/.fleet/user-attachments/ab12cd34ef567890/shot.png";

    const desktop = await loadFor("desktop");
    expect(desktop.userAttachmentUrl(p)).toBe(
      "fleet-attachment://localhost/ab12cd34ef567890/shot.png",
    );

    const web = await loadFor("web");
    expect(web.userAttachmentUrl(p)).toBe(
      `${window.location.origin}/user_attachment?key=ab12cd34ef567890&name=shot.png`,
    );
  });

  // Absolute, not root-relative: `markdown/localImages` only leaves a src alone
  // when it matches `^(?:https?|data|blob|fleet-[a-z-]+):`, and a bare
  // `/user_attachment?…` would instead be taken for a host path to resolve.
  it("returns an absolute URL so the markdown pass leaves it alone", async () => {
    const { userAttachmentUrl } = await loadFor("web");
    expect(userAttachmentUrl("/h/.fleet/user-attachments/k/a.png")).toMatch(/^https?:\/\//);
  });

  it("still percent-encodes and still refuses a path outside the store", async () => {
    const { userAttachmentUrl } = await loadFor("web");
    expect(userAttachmentUrl("/h/.fleet/user-attachments/ab12/my shot.png")).toBe(
      `${window.location.origin}/user_attachment?key=ab12&name=my%20shot.png`,
    );
    expect(userAttachmentUrl("/h/project/shot.png")).toBeNull();
  });

  // Pre-store pastes are served under a reserved key; that indirection belongs
  // to the route, not to the protocol, so it has to survive the swap.
  it("keeps the legacy paste key", async () => {
    const { userAttachmentUrl } = await loadFor("web");
    expect(userAttachmentUrl("/tmp/T/fleet-pasted/paste-1.png")).toBe(
      `${window.location.origin}/user_attachment?key=_pasted&name=paste-1.png`,
    );
  });
});
