async (page) => {
  const FEATURES = ["appearance","notifications","hooks_guard_elicitation","global_ask","prd_discipline","wiki_guidance","model_guidance","skill_interop"];
  await page.goto("http://localhost:5237/?mock", { waitUntil: "domcontentloaded" });
  // Pre-seed the @tauri-apps/plugin-store mock (localStorage "mock-store:" prefix)
  // so the app skips onboarding / What's-New and lands on the main UI.
  await page.evaluate((features) => {
    localStorage.setItem("mock-store:onboarding-dismissed", "true");
    localStorage.setItem("mock-store:onboarding-seen-features", JSON.stringify(features));
  }, FEATURES);
  await page.reload({ waitUntil: "networkidle" });
  await page.waitForTimeout(1500);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: "/private/tmp/claude-501/-Users-hoveychen-workspace-claude-fleet/48b9100f-0fab-45cb-a440-4a799441cd7c/scratchpad/asd_probe.png" });
  // Report some anchors we can click to open the detail.
  const buttons = await page.$$eval("button", (els) =>
    els.slice(0, 40).map((e) => (e.textContent || "").trim().slice(0, 30)).filter(Boolean)
  );
  const hasMain = await page.getByText("Implement mock mode", { exact: false }).count();
  return JSON.stringify({ hasMain, buttons }, null, 0);
}
