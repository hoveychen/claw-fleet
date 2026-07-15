#!/usr/bin/env node
/**
 * Regenerate the landing-page screenshot from Claw Fleet running in mock mode.
 *
 * The GitHub Pages site (docs/index.html) inlines exactly one app screenshot —
 *   docs/screenshots/01_gallery.png  — the live multi-agent gallery board.
 * (The old 02..08 shots were orphaned when the landing page was rebuilt around
 * the four-pillar narrative + the interactive Remotion player, so this script
 * no longer produces them.)
 *
 * Prerequisites:
 *   1. Mock dev server on :5199 —  (cd claw-fleet-desktop && npx vite --port 5199 --strictPort)
 *   2. patchwright-cli on PATH   —  the repo's browser-automation tool; it owns a
 *      persistent Chromium the app UI evolved past the old `playwright` devDep,
 *      which was removed, so we drive the installed CLI instead of launch()ing.
 *
 * Usage: node scripts/take-screenshots.mjs
 */

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { mkdirSync } from "node:fs";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = resolve(REPO_ROOT, "docs/screenshots");
const OUT_FILE = resolve(OUT_DIR, "01_gallery.png");
const BASE_URL = "http://localhost:5199/?mock";

mkdirSync(OUT_DIR, { recursive: true });

const pw = (...args) =>
  execFileSync("patchwright-cli", args, { cwd: REPO_ROOT, encoding: "utf8" });

// Runs inside patchwright's browser. Opens a crisp 2× dark context (matching the
// landing page's framed "app window" style), dismisses the onboarding dialog and
// the feature-tour wizard, flips to the Gallery grid, then captures the board.
const CAPTURE = `async () => {
  const browser = page.context().browser();
  const ctx = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    colorScheme: "dark",
  });
  const p = await ctx.newPage();
  const log = [];
  try {
    await p.goto(${JSON.stringify(BASE_URL)}, { waitUntil: "networkidle" });
    await p.waitForTimeout(2500);

    const gotit = p.getByRole("button", { name: /Got it, let's go/i }).first();
    if (await gotit.isVisible({ timeout: 3000 }).catch(() => false)) {
      await gotit.click();
      log.push("dismissed onboarding");
    }
    await p.waitForTimeout(1500);

    // The feature-tour wizard dims the grid until skipped.
    for (let i = 0; i < 3; i++) {
      const skip = p.getByRole("button", { name: /^Skip$/i }).first();
      if (await skip.isVisible({ timeout: 1000 }).catch(() => false)) {
        await skip.click();
        log.push("skipped wizard");
        await p.waitForTimeout(600);
      } else break;
    }
    await p.waitForTimeout(1500);

    // Flip to Gallery via the LayoutGrid toggle in the sessions banner.
    const toggled = await p.evaluate(() => {
      for (const b of document.querySelectorAll("button")) {
        const t = (b.getAttribute("title") || b.getAttribute("aria-label") || "").toLowerCase();
        if (t.includes("gallery") || t.includes("grid")) { b.click(); return t; }
      }
      return "not-found";
    });
    log.push("gallery toggle: " + toggled);
    await p.waitForTimeout(2500);

    await p.screenshot({ path: ${JSON.stringify(OUT_FILE)} });
    log.push("saved " + ${JSON.stringify(OUT_FILE)});
  } catch (e) {
    log.push("ERR: " + e.message);
  } finally {
    await ctx.close();
  }
  return log;
}`;

console.log("→ opening mock app:", BASE_URL);
pw("open", BASE_URL);
try {
  const out = pw("run-code", CAPTURE);
  const m = out.match(/### Result\s*\n(.+)/);
  console.log(m ? m[1] : out);
} finally {
  try { pw("close"); } catch { /* browser already gone */ }
}
console.log(`\n✅ ${OUT_FILE}`);
