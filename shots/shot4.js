async (page) => {
  const card = page.locator("text=Implement mock mode for demo screenshots").first();
  await card.click();
  await page.waitForTimeout(2500);
  // The transcript's ingest card is the way an artifact doc gets opened.
  const ingest = page.locator("[class*='ingest']");
  const n = await ingest.count();
  if (n === 0) {
    return { ingest: 0, body: (await page.locator('body').innerText()).slice(0, 600) };
  }
  await ingest.first().scrollIntoViewIfNeeded();
  await page.waitForTimeout(300);
  const cls = await ingest.first().getAttribute('class');
  return { ingest: n, cls };
}
