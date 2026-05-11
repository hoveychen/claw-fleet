// V1 task-as-unit E2E smoke spec.
//
// Status: scaffold. Playwright tooling is not yet wired into CI (no
// `playwright.config.*` / `@playwright/test` dependency in
// `claw-fleet-desktop/package.json`). This file captures the **intended**
// smoke flow so a future CI patch can drop in the Playwright runner without
// re-deriving the test surface. Until then, treat this as executable
// documentation.
//
// Per TASKS P17 acceptance:
//
//   开 Fleet → 点 [+ 新任务] → 拖一个小文件 → 起草 plan → 启动 task →
//   等 kanban 全 ✓ → 看 PR 生成
//
// Real-agent execution is unstable for V1 (P18 dogfood covers it manually);
// this smoke uses a mock-agent stub that responds to `claude --print` with
// pre-baked dispatch / mark-done events. Wiring the stub lives in
// `e2e/fixtures/mock-agent.ts` (TODO with Playwright integration).
//
// Time budget: < 60s.
//
// To enable in CI:
//   1. `pnpm add -D @playwright/test` in `claw-fleet-desktop/`
//   2. Add `playwright.config.ts` with `testDir: '../e2e'`
//   3. Bundle `mock-agent.ts` and inject via `FLEET_AGENT_BINARY_OVERRIDE`
//   4. Spawn Fleet via Tauri's WebDriver harness or run `claude_fleet_desktop` in `--no-tauri` web mode

import { expect, test } from '@playwright/test';

test.describe('Task-as-Unit V1 smoke', () => {
  test.skip('happy path: new task → plan → start → kanban green', async ({ page }) => {
    // 1. Open Fleet (web build).
    await page.goto('http://localhost:1420/');
    await expect(page.locator('[data-wizard="view-toggle"]')).toBeVisible();

    // 2. Click `Tasks` nav button → click `+ New task`.
    await page.getByRole('button', { name: /Tasks$/ }).first().click();
    await page.getByRole('button', { name: /\+ New task/ }).click();

    // 3. Inbox dialog: fill title + drop a small file.
    await page.getByPlaceholder('Task title').fill('smoke task');
    await page.getByPlaceholder(/description/i).fill('smoke description');
    // Drop a fixture file. Tauri webview doesn't expose <input type=file>
    // via Playwright; for the web-mode smoke we paste content into the
    // textarea instead.
    await page.getByRole('button', { name: /Create/ }).click();

    // 4. Wait for plan-drafted notification, then click `▶ Start`.
    await expect(page.getByText('smoke task')).toBeVisible();
    await page.getByRole('button', { name: /▶ Start/ }).click();

    // 5. Watch the kanban. mock-agent should march through P-items and
    //    every card should land in the "Done" column within the budget.
    await expect(page.locator('.column_done .card_done')).toHaveCount(2, {
      timeout: 30_000,
    });

    // 6. Master exits → task status flips to "Done".
    await expect(page.getByText('done')).toBeVisible({ timeout: 30_000 });
  });
});
