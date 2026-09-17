import { describe, expect, it } from "vitest";
import { showsMobilePanel } from "./hostEnv";

// Visibility of the Mobile panel. The answers for all three origin types comprise the entirety of this check,
// and the differences between them are not stylistic: if you give a pairing code to a host that a phone cannot reach,
// after scanning you'll only get a device stuck on Connecting…, with the reason invisible in the UI.
describe("showsMobilePanel", () => {
  it("Desktop always shows it, ignores origin", () => {
    expect(showsMobilePanel(false, "https:", "fleet.example.com")).toBe(true);
    // Tauri webview pages are neither https nor a real hostname, yet should still appear.
    expect(showsMobilePanel(false, "tauri:", "localhost")).toBe(true);
    expect(showsMobilePanel(false, "http:", "127.0.0.1")).toBe(true);
  });

  it("Cloud deployment (https + real hostname) ⇒ shows", () => {
    expect(showsMobilePanel(true, "https:", "fleet.example.com")).toBe(true);
    expect(showsMobilePanel(true, "https:", "fleet-cloud.muveeai.com")).toBe(true);
  });

  it("Local webui ⇒ doesn't show", () => {
    // `fleet webui` binds here by default.
    expect(showsMobilePanel(true, "http:", "127.0.0.1")).toBe(false);
    expect(showsMobilePanel(true, "http:", "localhost")).toBe(false);
    // Tunneling to localhost also counts as local: that origin is unreachable from a phone.
    expect(showsMobilePanel(true, "https:", "localhost")).toBe(false);
    expect(showsMobilePanel(true, "https:", "127.0.0.1")).toBe(false);
    expect(showsMobilePanel(true, "https:", "127.1.2.3")).toBe(false);
    expect(showsMobilePanel(true, "https:", "[::1]")).toBe(false);
    expect(showsMobilePanel(true, "https:", "app.localhost")).toBe(false);
  });

  it("Plain http with real hostname ⇒ doesn't show", () => {
    // The page on the phone is served over https; the browser won't allow it to connect to plain http.
    expect(showsMobilePanel(true, "http:", "fleet.example.com")).toBe(false);
    expect(showsMobilePanel(true, "http:", "192.168.1.5")).toBe(false);
  });

  it("Hostname case doesn't affect loopback detection", () => {
    expect(showsMobilePanel(true, "https:", "LOCALHOST")).toBe(false);
  });
});
