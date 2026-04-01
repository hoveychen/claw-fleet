#!/usr/bin/env node
/**
 * Take screenshots of Claw Fleet running in mock mode.
 * Usage: node scripts/take-screenshots.mjs
 *
 * Captures 8 views:
 *   01_gallery.png          — Gallery view with multi-agent groups
 *   02_session_detail.png   — Session detail with subagent tabs
 *   03_audit.png            — Security audit event list
 *   04_mascot.png           — Sidebar mascot assistant
 *   05_memory.png           — Memory panel expanded
 *   06_notifications.png    — Floating alert notifications
 *   07_report.png           — Insights timeline with AI summaries feed
 *   08_daily_report.png     — Daily report with metrics and AI summary
 */

import { chromium } from "playwright";
import { mkdirSync } from "fs";

const BASE_URL = "http://localhost:5199/?mock";
const OUT_DIR = "docs/screenshots";

mkdirSync(OUT_DIR, { recursive: true });

/** Dismiss all overlays (onboarding, wizard, etc.) */
async function dismissOverlays(page) {
  for (let i = 0; i < 8; i++) {
    for (const text of [
      "Skip", "Dismiss", "Continue", "Done", "Start",
      "Get Started", "Let's Go", "Got it", "Finish", "Next",
    ]) {
      const btn = page.locator(`button >> text="${text}"`).first();
      if (await btn.isVisible({ timeout: 200 }).catch(() => false)) {
        await btn.click({ force: true });
        await page.waitForTimeout(200);
      }
    }
  }
}

/** Click a nav button by its icon text (☰, ⊞, ⛨) */
async function clickNav(page, icon) {
  await page.evaluate((ic) => {
    const spans = document.querySelectorAll("span");
    for (const s of spans) {
      if (s.textContent?.trim() === ic) {
        const btn = s.closest("button");
        if (btn) { btn.click(); return; }
      }
    }
  }, icon);
  await page.waitForTimeout(1000);
}

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

  // Connect locally — button text varies: "Get Started", "Connect Locally", "Local Mode"
  for (const label of ["Get Started", "Connect Locally", "Local Mode"]) {
    const btn = page.locator(`button >> text="${label}"`).first();
    if (await btn.isVisible({ timeout: 500 }).catch(() => false)) {
      await btn.click({ force: true });
      break;
    }
  }
  await page.waitForTimeout(1500);

  // Dismiss overlays
  await dismissOverlays(page);
  await page.waitForTimeout(2000);

  // ── 01. Gallery View ──────────────────────────────────────────────────
  await clickNav(page, "⊞");
  await page.waitForTimeout(1500);

  await page.screenshot({ path: `${OUT_DIR}/01_gallery.png` });
  console.log("✓ 01_gallery.png — Gallery view");

  // ── 02. Session Detail (multi-subagent) ───────────────────────────────
  // Click the claw-fleet group (has the most subagents) to open detail
  await page.evaluate(() => {
    // Find a group header that mentions "claw-fleet"
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      if (el.textContent?.includes("claw-fleet") &&
          el.className && typeof el.className === "string" &&
          (el.className.includes("group_header") || el.className.includes("group_body"))) {
        el.click();
        break;
      }
    }
  });
  await page.waitForTimeout(1000);

  // Also try clicking the card directly if group click didn't open detail
  await page.evaluate(() => {
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      if (el.className && typeof el.className === "string" &&
          el.className.includes("card") && !el.className.includes("footer") &&
          !el.className.includes("nav") &&
          el.textContent?.includes("claw-fleet")) {
        el.click();
        break;
      }
    }
  });
  await page.waitForTimeout(2500);

  await page.screenshot({ path: `${OUT_DIR}/02_session_detail.png` });
  console.log("✓ 02_session_detail.png — Session detail with subagents");

  // Close detail panel
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.trim() === "✕") { b.click(); break; }
    }
  });
  await page.waitForTimeout(500);

  // ── 03. Audit View ────────────────────────────────────────────────────
  await clickNav(page, "⛨");
  await page.waitForTimeout(2000);

  // Click the first event to show the detail panel
  await page.evaluate(() => {
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      if (el.className && typeof el.className === "string" &&
          el.className.includes("event_row")) {
        el.click();
        break;
      }
    }
  });
  await page.waitForTimeout(1000);

  await page.screenshot({ path: `${OUT_DIR}/03_audit.png` });
  console.log("✓ 03_audit.png — Audit view");

  // ── 04. Mascot / Assistant ────────────────────────────────────────────
  // Switch back to list view so sidebar is visible with mascot
  await clickNav(page, "☰");
  await page.waitForTimeout(1000);

  // Scroll sidebar to show mascot
  await page.evaluate(() => {
    const sidebar = document.querySelector('[class*="sidebar_content"]');
    if (sidebar) sidebar.scrollTop = sidebar.scrollHeight;
  });
  await page.waitForTimeout(1500);

  // Crop to mascot region (bottom-left sidebar area)
  // Find the mascot container element and screenshot it with some context
  const mascotBox = await page.evaluate(() => {
    // Find the mascot eyes SVG container
    const mascot = document.querySelector('[class*="mascot_container"]') ||
                   document.querySelector('[class*="mascot_wrap"]') ||
                   document.querySelector('[class*="MascotEyes"]');
    if (mascot) {
      const r = mascot.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    }
    // Fallback: find SVG with the eyes
    const svgs = document.querySelectorAll("svg");
    for (const s of svgs) {
      if (s.querySelector("ellipse") && s.closest('[class*="sidebar"]')) {
        const r = s.getBoundingClientRect();
        return { x: r.x, y: r.y, w: r.width, h: r.height };
      }
    }
    return null;
  });

  if (mascotBox) {
    // Add generous padding around the mascot and include sidebar context
    const pad = 40;
    const clipX = Math.max(0, mascotBox.x - pad);
    const clipY = Math.max(0, mascotBox.y - 60);
    const clipW = Math.min(mascotBox.w + pad * 2, 1440 - clipX);
    const clipH = Math.min(mascotBox.h + 100, 900 - clipY);
    await page.screenshot({
      path: `${OUT_DIR}/04_mascot.png`,
      clip: { x: clipX, y: clipY, width: clipW, height: clipH },
    });
  } else {
    // Fallback: crop to left sidebar bottom area
    await page.screenshot({
      path: `${OUT_DIR}/04_mascot.png`,
      clip: { x: 0, y: 550, width: 320, height: 350 },
    });
  }
  console.log("✓ 04_mascot.png — Mascot assistant (cropped)");

  // ── 05. Memory Panel ──────────────────────────────────────────────────
  // Scroll sidebar to show memory panel
  await page.evaluate(() => {
    const sidebar = document.querySelector('[class*="sidebar_content"]');
    if (sidebar) {
      // Find the memory toggle and scroll to it
      const memoryToggle = document.querySelector('[class*="MemoryPanel"]') ||
                           document.querySelector('button[class*="toggle"]');
      if (memoryToggle) {
        memoryToggle.scrollIntoView({ behavior: "instant", block: "start" });
      } else {
        // Scroll to middle where memory typically is
        sidebar.scrollTop = sidebar.scrollHeight / 3;
      }
    }
  });
  await page.waitForTimeout(1000);

  // Make sure memory panel is expanded (click toggle if collapsed)
  await page.evaluate(() => {
    const buttons = document.querySelectorAll("button");
    for (const b of buttons) {
      if (b.textContent?.includes("Memory") && b.textContent?.includes("▼")) {
        b.click();
        break;
      }
    }
  });
  await page.waitForTimeout(500);

  // Click a memory file to open the detail modal
  await page.evaluate(() => {
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      if (el.className && typeof el.className === "string" &&
          el.className.includes("file_item") &&
          el.textContent?.includes("feedback_backend_sync")) {
        el.click();
        break;
      }
    }
  });
  await page.waitForTimeout(1500);

  await page.screenshot({ path: `${OUT_DIR}/05_memory.png` });
  console.log("✓ 05_memory.png — Memory panel with detail");

  // Close memory modal
  await page.evaluate(() => {
    const btns = document.querySelectorAll("button");
    for (const b of btns) {
      if (b.textContent?.trim() === "✕" || b.textContent?.trim() === "×") {
        b.click(); break;
      }
    }
  });
  await page.waitForTimeout(500);

  // ── 06. Notifications ─────────────────────────────────────────────────
  // The WaitingAlerts component should already be visible as a floating stack
  // Switch to gallery for a nicer background
  await clickNav(page, "⊞");
  await page.waitForTimeout(2000);

  // Crop to the notification toast area only
  // The WaitingAlerts overlay is position:fixed at bottom-right
  const alertClip = await page.evaluate(() => {
    // Find position:fixed elements that contain alert cards
    const allEls = document.querySelectorAll("*");
    for (const el of allEls) {
      const style = window.getComputedStyle(el);
      if (style.position === "fixed" && el.querySelector('[class*="card_content"], [class*="card_workspace"]')) {
        // Found the fixed overlay — now measure all its child cards
        const cards = el.querySelectorAll('[class*="card"]');
        let minX = Infinity, minY = Infinity, maxX = 0, maxY = 0;
        for (const c of cards) {
          const r = c.getBoundingClientRect();
          if (r.width < 10) continue;
          minX = Math.min(minX, r.x);
          minY = Math.min(minY, r.y);
          maxX = Math.max(maxX, r.right);
          maxY = Math.max(maxY, r.bottom);
        }
        if (minX < Infinity) return { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
      }
    }
    return null;
  });

  if (alertClip) {
    const pad = 24;
    const clipX = Math.max(0, alertClip.x - pad);
    const clipY = Math.max(0, alertClip.y - pad);
    const clipW = Math.min(alertClip.w + pad * 2, 1440 - clipX);
    const clipH = Math.min(alertClip.h + pad * 2, 900 - clipY);
    await page.screenshot({
      path: `${OUT_DIR}/06_notifications.png`,
      clip: { x: clipX, y: clipY, width: clipW, height: clipH },
    });
  } else {
    // Fallback: crop to bottom-right corner
    await page.screenshot({
      path: `${OUT_DIR}/06_notifications.png`,
      clip: { x: 1020, y: 680, width: 420, height: 220 },
    });
  }
  console.log("✓ 06_notifications.png — Notification alerts (cropped)");

  // ── 07. Insights Timeline ──────────────────────────────────────────────
  // Click the report nav button (calendar SVG icon)
  await page.evaluate(() => {
    const buttons = document.querySelectorAll("button");
    for (const b of buttons) {
      const svg = b.querySelector("svg");
      if (svg && svg.querySelector("rect") && svg.querySelectorAll("line").length >= 3) {
        if (b.className && typeof b.className === "string" && b.className.includes("nav_item")) {
          b.click();
          break;
        }
      }
    }
  });
  await page.waitForTimeout(3000);

  await page.screenshot({ path: `${OUT_DIR}/07_report.png` });
  console.log("✓ 07_report.png — Insights timeline view");

  // ── 08. Daily Report Detail ────────────────────────────────────────────
  // Click the "Daily Report" tab to switch to daily view
  await page.evaluate(() => {
    const buttons = document.querySelectorAll("button");
    for (const b of buttons) {
      if (b.textContent?.includes("Daily Report") || b.textContent?.includes("日报")) {
        b.click();
        break;
      }
    }
  });
  await page.waitForTimeout(1000);

  // Set date to yesterday
  const yesterday = new Date(Date.now() - 86400000).toISOString().slice(0, 10);
  await page.evaluate((dateVal) => {
    const input = document.querySelector('input[type="date"]');
    if (input) {
      const nativeInputValueSetter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype, "value"
      )?.set;
      if (nativeInputValueSetter) {
        nativeInputValueSetter.call(input, dateVal);
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.dispatchEvent(new Event("change", { bubbles: true }));
      }
    }
  }, yesterday);
  await page.waitForTimeout(2000);

  await page.screenshot({ path: `${OUT_DIR}/08_daily_report.png` });
  console.log("✓ 08_daily_report.png — Daily report detail view");

  // ── Done ──────────────────────────────────────────────────────────────
  await browser.close();
  console.log(`\n✅ All screenshots saved to ${OUT_DIR}/`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
