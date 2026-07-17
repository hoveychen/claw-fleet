async (page) => {
  const DIR = "/private/tmp/claude-501/-Users-hoveychen-workspace-claude-fleet/48b9100f-0fab-45cb-a440-4a799441cd7c/scratchpad/";
  await page.setViewportSize({ width: 1440, height: 900 });
  // Open the main session ("claw-fleet" card, +4 subagents) into the detail pane.
  await page.getByText("Implement mock mode for demo screenshots", { exact: false }).first().click();
  await page.waitForTimeout(900);

  // The agent scope trigger is the only header button carrying the ◈ glyph.
  const trigger = page.locator("button", { hasText: "◈" }).first();
  await trigger.waitFor({ state: "visible", timeout: 5000 });

  // Shot 1 — dropdown closed: the view-tab row is now the only tab strip,
  // uncrowded, with the agent scope tag sitting up in the header meta row.
  await page.screenshot({ path: DIR + "asd_detail_closed.png" });

  // Shot 2 — dropdown open: the full agent family, growing downward.
  await trigger.click();
  await page.waitForTimeout(400);
  await page.screenshot({ path: DIR + "asd_detail_open.png" });

  const triggerText = (await trigger.textContent()) || "";
  const menuItems = await page.$$eval(
    "body > div:last-child button",
    (els) => els.map((e) => (e.textContent || "").trim()).filter(Boolean)
  ).catch(() => []);
  return JSON.stringify({ triggerText: triggerText.trim(), menuItems });
}
