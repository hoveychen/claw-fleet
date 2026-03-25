#!/usr/bin/env node
/**
 * Take screenshots of Claude Fleet running in mock mode.
 * Usage: node scripts/take-screenshots.mjs
 */

import { chromium } from "playwright";
import { mkdirSync } from "fs";

const BASE_URL = "http://localhost:5199/?mock";
const OUT_DIR = "docs/screenshots";

mkdirSync(OUT_DIR, { recursive: true });

async function main() {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    colorScheme: "dark",
  });

  const page = await context.newPage();
  page.on("pageerror", (err) => console.error(`[PAGE ERROR] ${err.message}`));

  // ── Load app ──────────────────────────────────────────────────────────
  await page.goto(BASE_URL);
  await page.waitForTimeout(2000);

  // ── 0. Connection dialog ──────────────────────────────────────────────
  await page.screenshot({ path: `${OUT_DIR}/00_connection.png` });
  console.log("✓ 00_connection.png");

  // Connect locally
  await page.click("text=Connect Locally", { force: true });
  await page.waitForTimeout(1500);

  // Dismiss overlays (onboarding, wizard)
  for (let i = 0; i < 3; i++) {
    for (const text of ["Skip", "Dismiss", "Continue", "Done", "Start", "Get Started", "Let's Go", "Got it", "Finish"]) {
      const btn = page.locator(`button >> text="${text}"`).first();
      if (await btn.isVisible({ timeout: 300 }).catch(() => false)) {
        await btn.click({ force: true });
        await page.waitForTimeout(300);
      }
    }
  }

  await page.waitForTimeout(2500);

  // ── 1. Gallery View ──────────────────────────────────────────────────
  // Ensure gallery mode
  await page.evaluate(() => {
    // Find the gallery nav button (the one with ⊞)
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.includes("⊞")) { b.click(); break; }
    }
  });
  await page.waitForTimeout(1500);

  await page.screenshot({ path: `${OUT_DIR}/01_gallery_view.png` });
  console.log("✓ 01_gallery_view.png — Gallery view");

  // ── 2. List View ─────────────────────────────────────────────────────
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.includes("☰")) { b.click(); break; }
    }
  });
  await page.waitForTimeout(1000);

  // Show all sessions
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.includes("Show All") || b.textContent?.includes("show all")) {
        b.click(); break;
      }
    }
  });
  await page.waitForTimeout(500);

  await page.screenshot({ path: `${OUT_DIR}/02_list_view.png` });
  console.log("✓ 02_list_view.png — List view");

  // ── 3. Session Detail (click a session in list view) ─────────────────
  // Click the first session card by finding elements with className containing "card"
  await page.evaluate(() => {
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      if (el.className && typeof el.className === "string" && el.className.includes("card") && !el.className.includes("footer")) {
        // It's a card element — click it
        el.click();
        break;
      }
    }
  });
  await page.waitForTimeout(2500);

  await page.screenshot({ path: `${OUT_DIR}/03_session_detail.png` });
  console.log("✓ 03_session_detail.png — Session detail");

  // Close detail panel
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.trim() === "✕") { b.click(); break; }
    }
  });
  await page.waitForTimeout(500);

  // ── 4. Gallery + Detail ──────────────────────────────────────────────
  // Switch back to gallery
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.includes("⊞")) { b.click(); break; }
    }
  });
  await page.waitForTimeout(1500);

  // Click a card in gallery
  await page.evaluate(() => {
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      if (el.className && typeof el.className === "string" && el.className.includes("card") && !el.className.includes("footer") && !el.className.includes("nav")) {
        el.click();
        break;
      }
    }
  });
  await page.waitForTimeout(2500);

  await page.screenshot({ path: `${OUT_DIR}/04_gallery_with_detail.png` });
  console.log("✓ 04_gallery_with_detail.png — Gallery + detail");

  // Close detail
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.trim() === "✕") { b.click(); break; }
    }
  });
  await page.waitForTimeout(500);

  // ── 5. Settings panel ────────────────────────────────────────────────
  // Click the footer button that has ⚙
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.className.includes("footer") || b.textContent?.includes("⚙")) {
        b.click();
        break;
      }
    }
  });
  await page.waitForTimeout(1500);

  await page.screenshot({ path: `${OUT_DIR}/05_settings.png` });
  console.log("✓ 05_settings.png — Settings panel");

  // Close settings
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.trim() === "✕" || b.textContent?.trim() === "×") {
        b.click();
        break;
      }
    }
  });
  await page.waitForTimeout(500);

  // ── 6. Light theme via evaluate ──────────────────────────────────────
  // Close settings first by clicking the Done/Close button
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.trim() === "Done" || b.textContent?.trim() === "Close" || b.textContent?.trim() === "✕") {
        b.click(); break;
      }
    }
  });
  await page.waitForTimeout(500);

  // Set light theme directly
  await page.evaluate(() => {
    document.documentElement.setAttribute("data-theme", "light");
  });
  await page.waitForTimeout(500);

  await page.screenshot({ path: `${OUT_DIR}/06_light_theme.png` });
  console.log("✓ 06_light_theme.png — Light theme");

  // ── Done ──────────────────────────────────────────────────────────────
  await browser.close();
  console.log(`\n✅ All screenshots saved to ${OUT_DIR}/`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
