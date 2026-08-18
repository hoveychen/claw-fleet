// End-to-end user journey for the live-data browser harness.
//
//   scripts/live-ui.sh                       # terminal 1 — probe + vite
//   patchwright-cli -s=fleet run-code "$(cat scripts/live-ui-journey.js)"
//
// Creates a real dsh session through the launcher, watches its conversation
// render, then opens the Token/cost tab — the flow that kept being declared
// "working" from backend evidence alone. Screenshots land in
// /private/tmp/fleet-live-ui/.
//
// Clicks go through `page.evaluate` DOM dispatch rather than Playwright
// locators: the launcher re-renders under its own pollers, and locator
// actionability kept timing out on nodes that were demonstrably present.
async (page) => {
  const shots = "/private/tmp/fleet-live-ui";
  const steps = [];

  const text = () =>
    page.evaluate(() =>
      (document.querySelector("main") || document.body).innerText.replace(/\s+/g, " ").slice(0, 300),
    );
  const clickSel = (sel, nth = 0) =>
    page.evaluate(
      ([s, n]) => {
        const els = Array.from(document.querySelectorAll(s));
        const el = n < 0 ? els[els.length + n] : els[n];
        if (!el) return false;
        el.click();
        return true;
      },
      [sel, nth],
    );
  const clickText = (source) =>
    page.evaluate((src) => {
      const rx = new RegExp(src);
      const el = Array.from(document.querySelectorAll("button,[role=tab],a")).find((b) =>
        rx.test(b.innerText.trim()),
      );
      if (!el) return false;
      el.click();
      return true;
    }, source);

  await page.goto("http://127.0.0.1:5199/?mock&live", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(6000);
  await clickText("^任务");
  await page.waitForTimeout(2500);

  // ── 1. launcher: workspace → agent → prompt ───────────────────────────────
  const launcherUp = async () =>
    (await page.locator('[data-testid="workspace-pill"]').count()) > 0;
  for (let i = 0; i < 6 && !(await launcherUp()); i++) {
    await clickSel('button[title="新建会话"]');
    await page.waitForTimeout(1500);
  }
  if (!(await launcherUp())) throw new Error("launcher never mounted");

  await clickSel('[data-testid="workspace-pill"]', -1);
  await page.waitForTimeout(700);
  const wsInput = page.locator('input[placeholder="输入工作目录的绝对路径"]');
  await wsInput.fill("/tmp");
  await wsInput.press("Enter");
  await page.waitForTimeout(700);

  const agentLabel = () =>
    page.evaluate(() => {
      const el = document.querySelector('[data-testid="agent-pill"]');
      return el ? el.innerText.trim() : "(none)";
    });
  if (!/dsh/.test(await agentLabel())) {
    await clickSel('[data-testid="agent-pill"]', -1);
    await page.waitForTimeout(700);
    await clickText("^dsh$");
    await page.waitForTimeout(700);
  }
  steps.push(`workspace+agent set → agent=${await agentLabel()}`);
  await page.screenshot({ path: `${shots}/j1-launcher.png` });

  const ta = page.locator('textarea[placeholder*="想让 Agent"]').first();
  await ta.fill("用一句话回答:1+1 等于几?");
  await page.waitForTimeout(400);
  await ta.press("Enter");
  steps.push("submitted");

  // ── 2. the new session appears and renders its transcript ─────────────────
  let sawReply = false;
  for (let i = 0; i < 20 && !sawReply; i++) {
    await page.waitForTimeout(2500);
    const body = await text();
    if (/等于\s*2|=\s*2|是\s*2/.test(body)) sawReply = true;
    steps.push(`t+${((i + 1) * 2.5).toFixed(0)}s: ${body.slice(0, 140)}`);
  }
  await page.screenshot({ path: `${shots}/j2-conversation.png` });

  // ── 3. the cost / Token tab for that dsh session ──────────────────────────
  await clickText("^Token$");
  await page.waitForTimeout(5000);
  steps.push("token tab: " + (await text()).slice(0, 220));
  await page.screenshot({ path: `${shots}/j3-token.png` });

  const proxyLog = await page.evaluate(() => {
    const el = document.getElementById("fleet-live-proxy-log");
    const lines = (el ? el.textContent : "").split("\n");
    return lines.filter((l) => /spawn|dsh_|ERR|STALL/.test(l)).slice(-12).join("\n");
  });
  return { steps, sawReply, proxyLog };
}
