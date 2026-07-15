#!/usr/bin/env node
/**
 * Regenerate the landing-page screenshots from Claw Fleet running in mock mode.
 *
 * The GitHub Pages site (docs/index.html) inlines three real app screenshots —
 *   docs/screenshots/01_gallery.png          — desktop: live multi-agent gallery board.
 *   docs/screenshots/02_mobile_decisions.png  — mobile web: a fleet__ask decision card.
 *   docs/screenshots/03_mobile_tasks.png      — mobile web: the live task list.
 * The two mobile shots are framed as device bezels in the "from anywhere" section;
 * they replaced a hand-coded HTML phone mockup that no longer matched the real UI.
 *
 * Prerequisites:
 *   1. Desktop mock server on :5199 — (cd claw-fleet-desktop && npx vite --port 5199 --strictPort)
 *   2. Mobile mock server on :5188  — (cd mobile-web        && npx vite --port 5188 --strictPort)
 *   3. patchwright-cli on PATH      — the repo's browser-automation tool; it owns a
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
const MOBILE_URL = "http://localhost:5188/?mock";
const MOBILE_DECISIONS_FILE = resolve(OUT_DIR, "02_mobile_decisions.png");
const MOBILE_TASKS_FILE = resolve(OUT_DIR, "03_mobile_tasks.png");

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

// ── Mobile shots ──────────────────────────────────────────────────────────
// A phone-sized context (390×844 @3×, matching the device bezels on the landing
// page). Captures the default Decisions tab navigated to the fleet__ask card,
// then the Tasks tab. object-fit:cover on the landing crops each to its top ~72%.
const MOBILE_CAPTURE = `async () => {
  const browser = page.context().browser();
  const ctx = await browser.newContext({
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 3, isMobile: true, hasTouch: true, colorScheme: "light",
  });
  const p = await ctx.newPage();
  const log = [];
  try {
    await p.goto(${JSON.stringify(MOBILE_URL)}, { waitUntil: "networkidle" });
    await p.waitForTimeout(2000);

    // Decisions tab → select the fleet__ask ("Decision card") chip (billing-service).
    const askChip = await p.evaluate(() => {
      for (const el of document.querySelectorAll("button,[role=tab],div,span")) {
        const t = (el.textContent || "").trim();
        if (/billing/i.test(t) && t.length < 60) { el.click(); return t; }
      }
      return "not-found";
    });
    log.push("fleet-ask chip: " + askChip);
    await p.waitForTimeout(1200);
    await p.screenshot({ path: ${JSON.stringify(MOBILE_DECISIONS_FILE)} });
    log.push("saved " + ${JSON.stringify(MOBILE_DECISIONS_FILE)});

    // Bottom-nav → Tasks.
    const tasks = await p.evaluate(() => {
      for (const el of document.querySelectorAll("button,a,div")) {
        if ((el.textContent || "").trim() === "Tasks") { el.click(); return true; }
      }
      return false;
    });
    log.push("tasks tab: " + tasks);
    await p.waitForTimeout(1500);
    await p.screenshot({ path: ${JSON.stringify(MOBILE_TASKS_FILE)} });
    log.push("saved " + ${JSON.stringify(MOBILE_TASKS_FILE)});
  } catch (e) {
    log.push("ERR: " + e.message);
  } finally {
    await ctx.close();
  }
  return log;
}`;

console.log("\n→ opening mock mobile-web:", MOBILE_URL);
pw("open", MOBILE_URL);
try {
  const out = pw("run-code", MOBILE_CAPTURE);
  const m = out.match(/### Result\s*\n(.+)/);
  console.log(m ? m[1] : out);
} finally {
  try { pw("close"); } catch { /* browser already gone */ }
}
console.log(`\n✅ ${MOBILE_DECISIONS_FILE}`);
console.log(`✅ ${MOBILE_TASKS_FILE}`);
