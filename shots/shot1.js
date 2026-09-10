async (page) => {
  const rail = await page.locator("[class*='_rail_'] [class*='agent_card']").first();
  await rail.screenshot({ path: 'shots/agent-card.png' });
  await page.screenshot({ path: 'shots/rail-full.png' });
  await rail.click({ button: 'right' });
  await page.waitForTimeout(400);
  const menu = page.locator("[class*='menu']").last();
  const txt = await menu.innerText().catch(() => 'NO MENU');
  await page.screenshot({ path: 'shots/agent-menu.png' });
  return txt;
}
