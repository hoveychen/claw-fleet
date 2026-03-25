import { chromium } from "playwright";

const BASE_URL = "http://localhost:5199/?mock";

async function main() {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    colorScheme: "dark",
  });

  const page = await context.newPage();

  // Capture all console messages
  page.on("console", (msg) => {
    console.log(`[${msg.type()}] ${msg.text()}`);
  });

  // Capture page errors
  page.on("pageerror", (err) => {
    console.error(`[PAGE ERROR] ${err.message}`);
  });

  await page.goto(BASE_URL);
  await page.waitForTimeout(3000);

  // Check what's on the page
  const bodyHTML = await page.evaluate(() => document.body.innerHTML.substring(0, 2000));
  console.log("\n=== BODY HTML ===");
  console.log(bodyHTML);

  // Check if root has content
  const rootHTML = await page.evaluate(() => {
    const root = document.getElementById("root");
    return root ? root.innerHTML.substring(0, 1000) : "NO ROOT";
  });
  console.log("\n=== ROOT HTML ===");
  console.log(rootHTML);

  await browser.close();
}

main().catch(console.error);
