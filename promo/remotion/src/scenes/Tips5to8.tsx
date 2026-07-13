import React from "react";
import { spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, ease, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { Badge, PhonePip, SessionCard } from "../ui";
import { TipShell } from "./TipShell";

// Stages render inside <Sequence from={56}> — local frame 0 = pain swept away.

// ── Tip #5 — One composer, whole fleet dispatched ──────────────────────────
const DISPATCHES = [
  { ws: "billing-service", prompt: "Alert when the 5h window crosses 80%" },
  { ws: "search-service", prompt: "Reindex the v2.4 facets on staging" },
  { ws: "notif-service", prompt: "Coalesce pushes into 500ms batches" },
];
const DISPATCH_AT = [6, 44, 82]; // local frame each prompt starts typing
const Tip5Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const composerIn = spring({ frame: frame - 2, fps, config: { damping: 13 } });
  const active = frame >= DISPATCH_AT[2] ? 2 : frame >= DISPATCH_AT[1] ? 1 : 0;
  const d = DISPATCHES[active];
  const chars = Math.max(0, Math.min(d.prompt.length, Math.floor((frame - DISPATCH_AT[active]) * 2.4)));
  const caption = clamp01((frame - 116) / 12);
  return (
    <>
      <div
        style={{
          position: "absolute",
          top: 260,
          left: 120,
          width: 680,
          background: T.card,
          border: `2px solid ${T.borderStrong}`,
          borderRadius: 20,
          boxShadow: "0 10px 34px rgba(32,28,18,0.14)",
          padding: "24px 30px",
          opacity: composerIn,
          transform: `translateY(${lerp(30, 0, composerIn)}px)`,
        }}
      >
        <div style={{ display: "flex", gap: 12, marginBottom: 18 }}>
          <span
            style={{
              fontFamily: FONT.mono,
              fontSize: 20,
              padding: "7px 18px",
              borderRadius: 100,
              background: T.coralSoft,
              border: `1.5px solid ${T.coral}`,
              color: T.coral,
            }}
          >
            📁 {d.ws}
          </span>
          <span
            style={{
              fontFamily: FONT.mono,
              fontSize: 20,
              padding: "7px 18px",
              borderRadius: 100,
              background: T.paperDeep,
              border: `1.5px solid ${T.border}`,
              color: T.inkSecondary,
            }}
          >
            opus · high effort
          </span>
        </div>
        <div style={{ fontFamily: FONT.mono, fontSize: 26, color: T.ink, minHeight: 38 }}>
          {d.prompt.slice(0, chars)}
          <span style={{ opacity: frame % 16 < 8 ? 1 : 0, color: T.coral }}>▍</span>
        </div>
        <div style={{ marginTop: 16, fontFamily: FONT.mono, fontSize: 18, color: T.inkDim }}>
          ⏎ Enter to dispatch — session #{active + 1} of 3
        </div>
      </div>
      {DISPATCHES.map((disp, i) => {
        const pop = spring({ frame: frame - (DISPATCH_AT[i] + 30), fps, config: { damping: 12, mass: 0.6 } });
        if (frame < DISPATCH_AT[i] + 26) return null;
        return (
          <div
            key={disp.ws}
            style={{
              position: "absolute",
              top: 236 + i * 118,
              left: 900,
              width: 480,
              background: T.card,
              border: `2px solid ${T.border}`,
              borderLeft: `5px solid ${T.coral}`,
              borderRadius: 14,
              padding: "16px 22px",
              boxShadow: "0 8px 26px rgba(32,28,18,0.12)",
              opacity: pop,
              transform: `translateX(${lerp(46, 0, pop)}px)`,
            }}
          >
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <span style={{ fontSize: 24, fontWeight: 700 }}>{disp.ws}</span>
              <Badge kind="thinking" label="Thinking" />
            </div>
            <div style={{ fontFamily: FONT.mono, fontSize: 17, color: T.inkDim, marginTop: 8 }}>
              spawned ✓ · detached · headless
            </div>
          </div>
        );
      })}
      <div
        style={{
          position: "absolute",
          top: 610,
          left: 120,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: T.green,
          opacity: caption,
          transform: `translateX(${lerp(-16, 0, caption)}px)`,
        }}
      >
        ✓ three sessions launched — zero terminals opened
      </div>
    </>
  );
};
export const Tip5: React.FC = () => (
  <TipShell
    n={5}
    title="Dispatch the whole fleet from one place"
    pain={{
      title: "starting task #7 today",
      lines: ["$ cd ~/work/thing && claude", "$ paste 400 lines of context…", "$ repeat for every single task"],
    }}
    bubble="Three new deckhands from one composer. Didn't even open a terminal."
    side="right"
    pip={{ src: "footage/t5-dispatch.mp4", startFrom: 565, pos: "bl", zoom: { scale: 1.8, origin: "28% 22%" } }}
  >
    <Tip5Stage />
  </TipShell>
);

// ── Tip #6 — Long task? Pass the baton ────────────────────────────────────
const Tip6Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const aIn = spring({ frame: frame - 4, fps, config: { damping: 13 } });
  const pass = ease(clamp01((frame - 40) / 28));
  const bIn = spring({ frame: frame - 56, fps, config: { damping: 12, mass: 0.6 } });
  const noteX = lerp(330, 950, pass);
  return (
    <>
      <div style={{ position: "absolute", top: 280, left: 130, opacity: aIn, filter: `grayscale(${pass * 0.7})` }}>
        <SessionCard
          name="auth-refactor · #1"
          badge={{ kind: "streaming", label: "ctx 96%" }}
          line="P1–P3 done, tests green…"
          stats="fleet handoff --note '…' --next P4"
        />
      </div>
      <div
        style={{
          position: "absolute",
          top: 280,
          left: 760,
          opacity: bIn,
          transform: `translateX(${lerp(60, 0, bIn)}px)`,
        }}
      >
        <SessionCard
          name="auth-refactor · #2"
          badge={{ kind: "executing", label: "Executing" }}
          line="Picked up P4: swap middleware impl"
          stats="接力 2/2 · fresh context · same TASKS.md"
          highlight
        />
      </div>
      <div
        style={{
          position: "absolute",
          top: lerp(420, 370, Math.sin(pass * Math.PI)),
          left: noteX,
          width: 240,
          background: "#fff8dc",
          border: `2px solid ${T.amber}`,
          borderRadius: 10,
          padding: "12px 16px",
          fontFamily: FONT.mono,
          fontSize: 17,
          color: T.amber,
          boxShadow: "0 8px 22px rgba(32,28,18,0.18)",
          transform: `rotate(${lerp(-6, 4, pass)}deg)`,
          opacity: frame > 36 ? 1 : 0,
        }}
      >
        note: "P3 done, gotcha in
        <br />
        session.rs:214 — continue P4"
      </div>
    </>
  );
};
export const Tip6: React.FC = () => (
  <TipShell
    n={6}
    title="Long task? Pass the baton"
    pain={{
      title: "context window: 98%",
      lines: ["agent: I forgot what we were doing", "you: we've been at this 3 hours", "agent: shall I summarize… again?"],
    }}
    bubble="Context window's full. The mission isn't. Relay!"
    side="left"
    pip={{ src: "footage/t6-chains.mp4", startFrom: 210, pos: "br", zoom: { scale: 2.0, origin: "70% 45%" } }}
  >
    <Tip6Stage />
  </TipShell>
);

// ── Tip #7 — Your phone is the bridge now ─────────────────────────────────
// The phone IS the live capture: mobile-web demo footage inside a bezel.
const Tip7Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const line1 = spring({ frame: frame - 34, fps, config: { damping: 12, mass: 0.5 } });
  const done = spring({ frame: frame - 96, fps, config: { damping: 11, mass: 0.5 } });
  return (
    <>
      <PhonePip src="footage/t7-mobile.mp4" startFrom={115} enter={4} left={270} top={200} width={330} />
      <div
        style={{
          position: "absolute",
          top: 330,
          left: 700,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: T.inkSecondary,
          opacity: line1,
          transform: `translateX(${lerp(-20, 0, line1)}px)`,
        }}
      >
        guard card → your pocket, AI risk analysis included
      </div>
      <div
        style={{
          position: "absolute",
          top: 400,
          left: 700,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: "#15803d",
          opacity: done,
          transform: `translateX(${lerp(-20, 0, done)}px)`,
        }}
      >
        ✓ one tap — desktop session unblocked
      </div>
    </>
  );
};
export const Tip7: React.FC = () => (
  <TipShell
    n={7}
    title="Your phone is the bridge now"
    pain={{
      title: "you left for lunch",
      lines: ["12:04 agent: may I run the migration?", "13:37 agent: still waiting…", "14:52 agent: …hello?"],
    }}
    bubble="Approved from the beach. The fleet never knew I left."
    side="right"
  >
    <Tip7Stage />
  </TipShell>
);

// ── Tip #8 — The standup writes itself ────────────────────────────────────
const REPORT_LINES = [
  "· Shipped onboarding revamp (web-frontend, 14 commits)",
  "· Fixed JWT validation + 3 flaky tests (api-server)",
  "· data-pipeline stuck twice on schema drift — see lesson",
];
const Tip8Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 4, fps, config: { damping: 13 } });
  const copied = spring({ frame: frame - 92, fps, config: { damping: 11, mass: 0.5 } });
  return (
    <div
      style={{
        position: "absolute",
        top: 240,
        right: 140,
        width: 880,
        background: T.card,
        border: `2px solid ${T.border}`,
        borderRadius: 16,
        padding: "30px 40px",
        boxShadow: "0 8px 30px rgba(32,28,18,0.12)",
        opacity: cardIn,
        transform: `translateY(${lerp(30, 0, cardIn)}px)`,
      }}
    >
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 20 }}>
        <span style={{ fontFamily: FONT.display, fontSize: 36, fontWeight: 600 }}>Daily Report · Jul 13</span>
        <Badge kind="streaming" label="AI generated" />
      </div>
      {REPORT_LINES.map((line, i) => {
        const chars = Math.max(0, Math.min(line.length, Math.floor((frame - 12 - i * 16) / 0.5)));
        return (
          <div key={i} style={{ fontSize: 26, color: T.inkSecondary, marginBottom: 12, minHeight: 34, fontFamily: FONT.body }}>
            {line.slice(0, chars)}
          </div>
        );
      })}
      <div
        style={{
          marginTop: 8,
          fontFamily: FONT.mono,
          fontSize: 20,
          color: T.coral,
          background: T.coralSoft,
          border: `1.5px solid ${T.coral}`,
          borderRadius: 8,
          padding: "10px 16px",
          display: "inline-block",
          opacity: clamp01((frame - 78) / 10),
        }}
      >
        lesson → CLAUDE.md: "never trust schema drift checks to luck"
      </div>
      <div style={{ marginTop: 24 }}>
        <span
          style={{
            fontSize: 24,
            fontWeight: 650,
            padding: "11px 30px",
            borderRadius: 10,
            background: copied > 0.4 ? "#15803d" : T.coral,
            color: "#fff",
          }}
        >
          {copied > 0.4 ? "Copied ✓ — paste into Slack" : "Copy as Markdown"}
        </span>
      </div>
    </div>
  );
};
export const Tip8: React.FC = () => (
  <TipShell
    n={8}
    title="The standup writes itself"
    pain={{
      title: "standup in 10 minutes",
      lines: ["you: what did we ship yesterday?", "scrollback: 40,000 lines", "you: 'various improvements.'"],
    }}
    bubble="Eight hours of fleet work, summarized before your coffee's cold."
    side="left"
    pip={{ src: "footage/t8-report.mp4", startFrom: 180, pos: "tl", zoom: { scale: 1.5, origin: "50% 40%" } }}
  >
    <Tip8Stage />
  </TipShell>
);
