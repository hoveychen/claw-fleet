// Does a real dsh session show a real spend figure in the Token panel?
//
// The acceptance test for the "every dsh session says 'no model calls'" bug:
// backend evidence (a live OpenRouter lookup returning USD) is not the claim —
// the claim is that the panel a user opens shows the number.
//
//   FLEET_BIN=target/debug/fleet-cli \
//   FLEET_DSH_BIN=/opt/homebrew/bin/dsh \
//   FLEET_HOME=/tmp/fleet-live-ui/fleet-home \
//   scripts/live-ui.sh
//   patchwright-cli -s=cost run-code "$(cat scripts/live-ui-dsh-cost.js)"
//
// Note the REAL dsh binary, not the delay fixture the sibling scripts use: the
// generation ids have to be ones OpenRouter can price. `FLEET_HOME` still points
// at a throwaway dir, which only means the cost cache starts cold.
//
// The roster is walked rather than targeted by title, because a dsh session's
// title projection is frequently absent (measured: 2 of the 5 newest had none).
// So: open cards in order, look for the Token tab, and stop at the first session
// whose panel shows a priced figure.
async (page) => {
  const shots = "/private/tmp/fleet-live-ui";
  const steps = [];

  // `document.body`, not `main`: the session detail pane (and the Token panel in
  // it) renders outside `main`, so scoping to `main` reports a false negative on
  // a panel that is plainly on screen.
  const bodyText = () =>
    page.evaluate(() => document.body.innerText.replace(/\s+/g, " "));
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
  // `SessionCard` stamps the session id on its root — the only stable hook on a
  // roster card (its class names are CSS-module hashes).
  const CARDS = "[data-session-id]";
  const cardIds = () =>
    page.evaluate(
      (sel) => Array.from(document.querySelectorAll(sel)).map((c) => c.getAttribute("data-session-id")),
      CARDS,
    );
  const clickCardById = (id) =>
    page.evaluate(
      ([sel, want]) => {
        const el = Array.from(document.querySelectorAll(sel)).find(
          (c) => c.getAttribute("data-session-id") === want,
        );
        if (!el) return false;
        el.click();
        return true;
      },
      [CARDS, id],
    );

  await page.goto("http://127.0.0.1:5199/?mock&live", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(3000);
  await clickText("Got it|开始|走起");
  await page.waitForTimeout(3000);
  await clickText("^(Sessions|会话)$");
  await page.waitForTimeout(2500);
  // Idle sessions are hidden behind the default "Active only" filter, and every
  // finished dsh session is idle.
  await clickText("^(Show All|全部)$");
  await page.waitForTimeout(2500);

  const ids = await cardIds();
  steps.push(`${ids.length} cards in the roster`);
  await page.screenshot({ path: `${shots}/cost-0-roster.png` });

  // Try the sessions the backend was verified against first, then fall back to
  // walking the roster — a card is only in the DOM if the roster scan surfaced
  // it. Both pricing paths are represented, official first: it is the one whose
  // figure comes from a published rate table rather than a provider receipt.
  const KNOWN = [
    "session-7c50f2aa-62f5-47f8-b8f6-8d98ee789454", // deepseek-official, table-priced
    "session-edde1334-3364-47bc-8428-6bd38553f8ff", // openrouter, receipt-priced
  ];
  const known = KNOWN.filter((k) => ids.includes(k));
  const order = [...known, ...ids.filter((i) => !known.includes(i))];

  // "no model calls in this session yet" is the note the bug produced. A panel
  // still showing it has not been fixed — or that session genuinely never called
  // a model, which is why a priced one is what the run looks for.
  const NOTE = /no model calls/i;
  const PRICED = /\$\s?\d+\.\d{2,}/;

  let hit = null;
  for (let i = 0; i < Math.min(order.length, 12) && !hit; i++) {
    const id = order[i];
    if (!(await clickCardById(id))) continue;
    await page.waitForTimeout(1200);
    // Only dsh sessions render the Token panel; others skip on its absence.
    if (!(await clickText("^(Tokens?|令牌)$"))) {
      steps.push(`${id}: no Token tab (not a dsh session)`);
      continue;
    }
    await page.waitForTimeout(4000);
    const body = await bodyText();
    const priced = PRICED.test(body);
    const noted = NOTE.test(body);
    steps.push(`${id}: priced=${priced} note=${noted}`);
    if (priced && !noted) {
      hit = { id, body: body.slice(0, 500) };
    }
  }

  await page.screenshot({
    path: `${shots}/cost-1-token-${hit ? "priced" : "unpriced"}.png`,
  });
  return { steps, priced: !!hit, hit };
}
