import type { CapacitorConfig } from "@capacitor/cli";

// Native shell around the existing mobile-web React app. The bundled build is
// served from the app package (no `server.url`), so `window.location.origin`
// inside the shell is `http://localhost` / `capacitor://localhost` — NOT the
// relay. `relayHttpBase()` falls back to `origin`, so shell builds MUST be
// produced with `VITE_RELAY_URL` set to the real relay, otherwise the app
// silently tries to talk to itself. See `scripts/build-shell.sh`.
const config: CapacitorConfig = {
  appId: "com.hoveychen.clawfleet",
  appName: "Fleet",
  webDir: "dist",
  android: {
    // Release builds are signed with .signing/fleet-release.jks, whose SHA-256
    // is registered in AppGallery Connect — Push Kit rejects tokens from a
    // package whose signature doesn't match. Debug builds use the default
    // debug keystore and will NOT receive pushes.
    buildOptions: {
      keystorePath: "../../../.signing/fleet-release.jks",
      keystoreAlias: "fleet",
    },
  },
};

export default config;
