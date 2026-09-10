async (page) => {
  await page.keyboard.press('Escape');
  const chips = page.locator("[class*='path_chip'], [class*='pathLink'], [data-path]");
  const n = await chips.count();
  const wiki = page.locator("[class*='wiki_ref'], [class*='wikiLink']");
  return { pathChips: n, wikiRefs: await wiki.count() };
}
