// Does picking a dsh model + effort in the launcher actually reach dsh?
//
// The acceptance test for the model picker. Backend evidence (the /dsh_models
// route returning 278 real models) is not the claim — the claim is that a user
// who opens the launcher, switches to dsh, picks `DeepSeek-V4-Pro` and effort
// `max`, and hits send gets a session dsh runs on *that* pair. So this drives
// the real pills and returns the spawned session id; the wire is then checked
// against `session.history`'s `request/header` outside the browser.
//
//   cargo build -p fleet-cli          # ← the UI reads the binary, not the source
//   FLEET_BIN=target/debug/fleet-cli \
//   FLEET_DSH_BIN=/opt/homebrew/bin/dsh \
//   DSH_HOME=$HOME/.dsh \
//   FLEET_HOME=/tmp/fleet-live-ui/fleet-home \
//   scripts/live-ui.sh
//   patchwright-cli -s=picker open "http://localhost:5199/?mock&live"
//   patchwright-cli -s=picker run-code "$(cat scripts/live-ui-dsh-model-picker.js)"
//
// `DSH_HOME` is not optional: `real_home_dir()` honours FLEET_HOME, so a
// throwaway FLEET_HOME points `get_dsh_dir()` at a `.dsh` that does not exist
// and no provider credential resolves.
//
// Pills are driven by DOM dispatch rather than Playwright locators: the roster
// re-renders on a poll, which makes locator clicks time out.
async (page) => {
  const shots = "/private/tmp/fleet-live-ui";
  const steps = [];

  const bodyText = () => page.evaluate(() => document.body.innerText.replace(/\s+/g, " "));
  const clickText = (src, sel = "button,[role=tab],a") =>
    page.evaluate(
      ([s, q]) => {
        const rx = new RegExp(s);
        const el = Array.from(document.querySelectorAll(q)).find((b) =>
          rx.test((b.textContent ?? "").trim()),
        );
        if (!el) return false;
        el.click();
        return true;
      },
      [src, sel],
    );
  // The pills carry stable testIds; their labels change with the selection and
  // their class names are CSS-module hashes.
  const clickPill = (testId) =>
    page.evaluate((id) => {
      const el = document.querySelector(`[data-testid="${id}"]`);
      if (!el) return false;
      el.click();
      return true;
    }, testId);
  // `textContent`, not `innerText`: innerText is layout-dependent and comes back
  // empty for a popover the browser has not laid out yet, which made this run
  // report nine blank rows for a menu that was plainly populated.
  const menuRows = () =>
    page.evaluate(() =>
      Array.from(document.querySelectorAll('button[role="menuitem"]')).map((b) =>
        (b.textContent ?? "").trim(),
      ),
    );
  const clickRow = (label) =>
    page.evaluate((want) => {
      const el = Array.from(document.querySelectorAll('button[role="menuitem"]')).find(
        (b) => (b.textContent ?? "").trim() === want,
      );
      if (!el) return false;
      el.click();
      return true;
    }, label);
  const pillLabel = (testId) =>
    page.evaluate(
      (id) => (document.querySelector(`[data-testid="${id}"]`)?.textContent ?? "").trim() || null,
      testId,
    );
  /** Wait until the open menu actually has labelled rows. */
  const waitRows = async () => {
    for (let i = 0; i < 20; i++) {
      const r = await menuRows();
      if (r.length && r.some((x) => x)) return r;
      await page.waitForTimeout(250);
    }
    return await menuRows();
  };

  await page.goto("http://localhost:5199/?mock&live", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(3000);
  await clickText("Got it|开始|走起");
  await page.waitForTimeout(2500);
  // The launcher lives on the **Tasks** page (`HistoryView`, `view="history"`),
  // not the Sessions page: Tasks lists the sessions Fleet itself spawned, which
  // is what the New Session button adds to. Driving "Sessions" lands on the
  // full-scan roster, which has no launcher.
  await clickText("^(Tasks|任务)$");
  await page.waitForTimeout(3000);

  // Open the launcher. `data-wizard` is the stable hook — the button is icon +
  // label, so its innerText carries a newline in some locales.
  const opened = await page.evaluate(() => {
    const b = document.querySelector('[data-wizard="new-session-btn"]');
    if (!b) return false;
    b.click();
    return true;
  });
  if (!opened) {
    return { ok: false, steps, why: "no New Session button" };
  }
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${shots}/picker-0-launcher.png` });

  // Switch the agent to dsh — this is what makes the catalogue fetch fire.
  if (!(await clickPill("agent-pill"))) return { ok: false, steps, why: "no agent pill" };
  await page.waitForTimeout(400);
  steps.push(`agent menu: ${(await waitRows()).join(" | ")}`);
  if (!(await clickRow("dsh"))) return { ok: false, steps, why: "dsh not offered" };
  await page.waitForTimeout(3000); // the first llm.models call may start dsh web

  // The model menu: top level.
  if (!(await clickPill("model-pill"))) return { ok: false, steps, why: "no model pill" };
  await page.waitForTimeout(600);
  const topRows = await waitRows();
  steps.push(`model menu L1 (${topRows.length}): ${topRows.join(" | ")}`);
  await page.screenshot({ path: `${shots}/picker-1-model-l1.png` });

  // The folded second level, proving a vendor row does not dismiss the popover.
  const vendorRow = topRows.find((r) => /^anthropic \(\d+\)$/.test(r));
  if (vendorRow) {
    await clickRow(vendorRow);
    await page.waitForTimeout(500);
    const sub = await waitRows();
    steps.push(`model menu L2 ${vendorRow} (${sub.length}): ${sub.slice(0, 8).join(" | ")}`);
    await page.screenshot({ path: `${shots}/picker-2-model-l2.png` });
    // Back out to the top level.
    await clickRow(vendorRow);
    await page.waitForTimeout(400);
  } else {
    steps.push("no anthropic folder at L1 (catalogue smaller than the fold cap?)");
  }

  // Pick the model the plan names.
  if (!(await clickRow("DeepSeek-V4-Pro"))) {
    return { ok: false, steps, why: "DeepSeek-V4-Pro not in the menu", rows: topRows };
  }
  await page.waitForTimeout(600);
  steps.push(`model pill now: ${await pillLabel("model-pill")}`);

  // The effort menu must now be dsh's own ladder for *this* model.
  if (!(await clickPill("effort-pill"))) return { ok: false, steps, why: "no effort pill" };
  await page.waitForTimeout(500);
  const effortRows = await waitRows();
  steps.push(`effort menu: ${effortRows.join(" | ")}`);
  await page.screenshot({ path: `${shots}/picker-3-effort.png` });
  // "max" exists in dsh's scale and in Claude's but not Codex's; "medium" exists
  // in Claude's and Codex's but NOT in deepseek-official's. Seeing max without
  // medium is the fingerprint of the catalogue's own ladder.
  const dshLadder = effortRows.includes("max") && !effortRows.includes("medium");
  if (!(await clickRow("max"))) return { ok: false, steps, why: "max not offered", effortRows };
  await page.waitForTimeout(500);
  steps.push(`effort pill now: ${await pillLabel("effort-pill")}`);

  // Workspace. The path input lives in the workspace pill's own popover
  // (`menuHeader`), so it does not exist until that pill is open, and it commits
  // on Enter rather than on change.
  if (!(await clickPill("workspace-pill"))) {
    return { ok: false, steps, why: "no workspace pill" };
  }
  await page.waitForTimeout(400);
  const wsSet = await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll("input")).find((i) =>
      /Absolute path|绝对路径/.test(i.placeholder ?? ""),
    );
    if (!el) return false;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    el.focus();
    setter.call(el, "/tmp");
    el.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  });
  if (!wsSet) return { ok: false, steps, why: "no workspace path input in the pill" };
  await page.keyboard.press("Enter");
  await page.waitForTimeout(600);
  steps.push(`workspace pill now: ${await pillLabel("workspace-pill")}`);

  const promptSet = await page.evaluate(() => {
    const el = document.querySelector("textarea");
    if (!el) return false;
    const setter = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    ).set;
    setter.call(el, "只回答两个字：收到");
    el.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  });
  if (!promptSet) return { ok: false, steps, why: "no prompt textarea" };
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${shots}/picker-4-ready.png` });

  const before = await page.evaluate(() =>
    Array.from(document.querySelectorAll("[data-session-id]")).map((c) =>
      c.getAttribute("data-session-id"),
    ),
  );

  // Enter submits the composer (Shift+Enter is the newline).
  await page.evaluate(() => document.querySelector("textarea")?.focus());
  await page.keyboard.press("Enter");
  await page.waitForTimeout(12000);
  await page.screenshot({ path: `${shots}/picker-5-spawned.png` });

  const after = await page.evaluate(() =>
    Array.from(document.querySelectorAll("[data-session-id]")).map((c) =>
      c.getAttribute("data-session-id"),
    ),
  );
  const fresh = after.filter((id) => !before.includes(id) && id?.startsWith("session-"));
  steps.push(`new dsh cards: ${fresh.join(", ") || "(none yet)"}`);

  return {
    ok: fresh.length > 0 && dshLadder,
    dshLadder,
    spawned: fresh,
    steps,
    body: (await bodyText()).slice(0, 400),
  };
}
