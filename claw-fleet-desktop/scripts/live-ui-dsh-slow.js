// Does the 对话 tab still get its data when dsh is slow?
//
// Drives the REAL frontend (vite + `?mock&live`) against a `fleet serve` whose
// dsh is the delay-injecting fixture, and times the conversation from click to
// content. Pair it with the fixture server so the roster poll is slow while the
// interactive read stays cheap:
//
//   FLEET_BIN=target/debug/fleet-cli \
//   FLEET_DSH_BIN=claw-fleet-core/tests/fixtures/fake-dsh.js \
//   FAKE_DSH_LIST_DELAY_MS=5000 FAKE_DSH_HISTORY_DELAY_MS=200 \
//   FAKE_DSH_SESSION_CWD=$PWD FLEET_HOME=/tmp/fleet-live-ui/fleet-home \
//   scripts/live-ui.sh
//   patchwright-cli -s=starve run-code "$(cat scripts/live-ui-dsh-slow.js)"
//
// Measured across the dsh-read-starvation fix (5s dsh, same script):
//   before — roster listed at 42.7s, conversation fell open (stalled) after 20.1s
//   after  — roster listed at 11.1s, conversation had data in 0.65s
//
// Then across the serve worker-pool fix, same binary, arms selected with
// FLEET_SERVE_WORKERS (sampling /health every 0.5s alongside):
//   1 worker  — roster 11.2s, conversation 2.71s, /health max 9.82s
//   8 workers — roster 11.1s, conversation 0.67s, /health max 1.2ms
// Do NOT read the roster number as a backend latency. The four fixed waits
// below (3000 + 4000 + 2500 + 1500) are 11s on their own, so "roster listed at
// 11.1s" means the session was already there the first time the loop looked —
// the metric is saturated and cannot show further improvement. It only
// distinguishes "data arrived within the scripted walk-through" (11s) from "it
// did not" (42.7s, pre-dsh-fix). For backend latency, sample the endpoints:
// with a 5s dsh, /sessions went 5.0s per poll → 1.2ms (max, n=57) once the
// session list moved to a background-refreshed snapshot, and dsh saw 6
// session.list calls across the whole run instead of one per poll.
//
// Clicks go through `page.evaluate` DOM dispatch rather than Playwright locators;
// see the note in live-ui-journey.js for why.
async (page) => {
  const shots = "/private/tmp/fleet-live-ui";
  const steps = [];
  const t0 = Date.now();
  const at = () => ((Date.now() - t0) / 1000).toFixed(1) + "s";

  const bodyText = () =>
    page.evaluate(() =>
      (document.querySelector("main") || document.body).innerText.replace(/\s+/g, " ").slice(0, 400),
    );
  const clickText = (src, sel = "button,[role=tab],a") =>
    page.evaluate(
      ([s, q]) => {
        const rx = new RegExp(s);
        const el = Array.from(document.querySelectorAll(q)).find((b) => rx.test(b.innerText.trim()));
        if (!el) return false;
        el.click();
        return true;
      },
      [src, sel],
    );
  const clickContaining = (needle) =>
    page.evaluate((n) => {
      const els = Array.from(document.querySelectorAll("*")).filter(
        (e) => e.children.length === 0 && e.textContent && e.textContent.includes(n),
      );
      const el = els[els.length - 1];
      if (!el) return false;
      let node = el;
      for (let i = 0; i < 6 && node; i++) {
        node.click();
        node = node.parentElement;
      }
      return true;
    }, needle);

  // Seed the onboarding flag before the app boots, or the first screen is the
  // welcome wizard rather than the session roster.
  await page.goto("http://127.0.0.1:5199/?mock&live", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(3000);
  // Dismiss the onboarding panel the way a user would, then go to the session
  // roster (the Tasks page only lists Fleet-spawned sessions).
  await clickText("Got it|开始|走起");
  await page.waitForTimeout(4000);
  await clickText("^(Sessions|会话)$");
  await page.waitForTimeout(2500);
  // The roster defaults to "Active only"; an idle session needs the wider view.
  await clickText("^(Show All|全部)$");
  await page.waitForTimeout(1500);

  // The fake dsh session must show up in the roster first.
  let listed = false;
  for (let i = 0; i < 40 && !listed; i++) {
    listed = await page.evaluate(() => document.body.innerText.includes("慢 dsh 探针会话"));
    if (!listed) await page.waitForTimeout(1500);
  }
  steps.push(`${at()} session listed: ${listed}`);
  await page.screenshot({ path: `${shots}/p5-1-roster.png` });
  if (!listed) return { steps, listed, body: await bodyText() };

  const tOpen = Date.now();
  await clickContaining("慢 dsh 探针会话");
  await page.waitForTimeout(600);
  await clickText("^对话$");
  steps.push(`${at()} opened detail + 对话 tab`);

  // Either the conversation renders, or the deadline fires and the UI falls open.
  let outcome = "timeout";
  for (let i = 0; i < 200; i++) {
    const state = await page.evaluate(() => {
      const t = document.body.innerText;
      return {
        gotData: t.includes("对话区没有卡在加载中") || t.includes("fake dsh"),
        stalled: !!document.querySelector('[data-testid="conversation-stalled"]'),
      };
    });
    if (state.gotData) {
      outcome = "data";
      break;
    }
    if (state.stalled) {
      outcome = "stalled";
      break;
    }
    await page.waitForTimeout(250);
  }
  const elapsedMs = Date.now() - tOpen;
  steps.push(`${at()} outcome=${outcome} after ${(elapsedMs / 1000).toFixed(2)}s`);
  // Name the shot after the outcome *and* the measurement: naming it after the
  // outcome alone still collides when both arms of an A/B end in `data` (a
  // 0.67s conversation and a 2.71s one are the same word), and the second run
  // then overwrites the first arm's evidence.
  await page.screenshot({
    path: `${shots}/p5-conversation-${outcome}-${elapsedMs}ms.png`,
  });

  return { steps, listed, outcome, elapsedMs, body: await bodyText() };
}
