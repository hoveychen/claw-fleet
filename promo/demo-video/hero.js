// Promo screencast hero script (plan promo-mock-demo).
//
// Drives the desktop app in ?mock&demo through: the busy board → dispatching a
// big task → the 5-hop REST→gRPC handoff relay streaming one session at a time
// → a quick feature tour. No voiceover; subtitles are burned in post from the
// measured timeline this script returns (offsets are relative to screencast
// start, so P5's SRT never drifts).
//
// Run:  patchwright-cli run-code --filename promo/demo-video/hero.js
// (from a cwd where you want out.webm written). Records 2560x1440 — the
// resolution the app layout actually fills (true-4K viewport leaves it sparse;
// upscale 1440p→2160p with ffmpeg lanczos in post if a 4K file is wanted).
async (page) => {
  const T = (ms) => page.waitForTimeout(ms);
  const BASE = "http://localhost:1425/?mock&demo";

  // View prefs before boot: gallery board (the "mission control" look), English.
  await page.setViewportSize({ width: 2560, height: 1440 });
  // Short default timeout: a missed selector should cost ~5s of dead air, not
  // Playwright's 30s default (which would blow the recording's pacing).
  page.setDefaultTimeout(5000);
  await page.addInitScript(() => {
    try {
      // List view: full-width session rows that fill 2560px, and the verified
      // streaming layout (left list + right detail pane).
      localStorage.setItem("mock-store:viewMode", "list");
      localStorage.setItem("mock-store:lang", "en");
    } catch (e) {}
  });

  await page.goto(BASE);
  await page.waitForTimeout(3000);

  // ── subtitle timeline (offset from screencast start) ──
  const marks = [];
  let t0 = 0;
  const mark = (sub) => marks.push({ ms: Date.now() - t0, sub });

  await page.screencast.start({
    path: "out.webm",
    size: { width: 2560, height: 1440 },
  });
  t0 = Date.now();

  // Sidebar nav items can carry a badge count ("Audit6", "Tasks5"), so match
  // starts-with and take the first hit (the sidebar item precedes the page
  // header in DOM order).
  const goNav = async (name) => {
    await page.getByText(new RegExp("^" + name)).first().click().catch(() => {});
  };
  // Open a relay hop's *streaming detail* by clicking its card body — matched on
  // a hop-unique preview string. (Clicking the "Relay N/5" chip instead would
  // open the handoff-chain modal, not the detail.)
  const HOP_PREVIEW = {
    1: /endpoint inventory/i,
    2: /protos.*TASKS\.md written/i,
    3: /core services on gRPC/i,
    4: /clients migrated, shim live/i,
    5: /Fixing the last integration test/i,
  };
  const openRelay = async (n) => {
    await page.getByText(HOP_PREVIEW[n]).first().click().catch(() => {});
  };

  // 1 — the board
  mark("One developer. A whole fleet of Claude Code agents.");
  await T(5500);
  mark("Every session live — thinking, tokens, cost, one board.");
  await page.mouse.wheel(0, 700);
  await T(2500);
  await page.mouse.wheel(0, -700);
  await T(2500);

  // 2 — dispatch a big task
  mark("Start a big one: migrate 43 REST endpoints to gRPC.");
  await goNav("Tasks");
  await T(1200);
  await page.locator("[data-wizard=new-session-btn]").first().click().catch(() => {});
  await T(1000);
  const ta = page.locator("textarea").first();
  await ta.fill("");
  await ta.pressSequentially(
    "Migrate the API layer from REST to gRPC — 43 endpoints, keep a REST shim. Big job; take it as far as one session can, then hand off.",
    { delay: 12 },
  );
  await T(1500);
  mark("Dispatch it — Claude picks it up in the background.");
  // Enter submits the composer (Shift+Enter would newline). More robust than
  // locating the send button.
  await ta.press("Enter").catch(() => {});
  await T(3500);

  // 3 — the relay
  mark("A job this big can't fit one context window.");
  await goNav("Sessions");
  await T(1500);
  await page.getByText(/Show All/i).first().click().catch(() => {});
  await T(1500);
  mark("So Fleet relays it — session to session, baton passed.");
  await page.mouse.wheel(0, 900);
  await T(3000);
  await page.mouse.wheel(0, -900);
  await T(600);

  // 4 — hop 1
  mark("Hop 1 — audit every endpoint and its callers.");
  await openRelay(1);
  await T(9000);
  mark("Context full at 96% — it hands off with notes.");
  await T(3500);

  // Hops 2-5: the left session list stays visible beside the open detail, so
  // click the next relay card directly — never re-navigate (that would reset
  // "Show All" and re-hide the idle hops).

  // 5 — hop 2
  mark("Hop 2 — design the protos, one decision for you.");
  await openRelay(2);
  await T(11000);

  // 6 — hop 3
  mark("Hop 3 — implement the gRPC service handlers.");
  await openRelay(3);
  await T(10500);

  // 7 — hop 4
  mark("Hop 4 — migrate the clients, stand up the REST shim.");
  await openRelay(4);
  await T(10500);

  // 8 — hop 5 (the live one)
  mark("Hop 5 — tests green, docs, ready to merge.");
  await openRelay(5);
  await T(11000);

  mark("Five sessions. One migration. Zero lost context.");
  await page.mouse.wheel(0, -400);
  await T(3500);

  // 9 — feature tour
  mark("Daily reports of everything your agents shipped.");
  await goNav("Report");
  await T(4500);
  mark("Every risky command, audited before it runs.");
  await goNav("Audit");
  await T(4000);
  mark("A shared wiki and long-term memory, built in.");
  await goNav("Wiki");
  await T(3200);
  await goNav("Memory");
  await T(3200);
  mark("Answer decisions from your phone, too.");
  await goNav("Mobile");
  await T(4000);

  // 10 — outro
  mark("Claw Fleet — run your agents like a fleet.");
  await goNav("Sessions");
  await T(4500);

  await page.screencast.stop();
  return JSON.stringify(marks, null, 0);
}
