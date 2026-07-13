import React from "react";
import { spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, ease, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { Badge, SessionCard } from "../ui";
import { TipShell } from "./TipShell";

// Stages render inside <Sequence from={56}> — local frame 0 = pain swept away.

// ── Tip #5 — Dispatch from anywhere ───────────────────────────────────────
const Tip5Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const pillIn = spring({ frame: frame - 4, fps, config: { damping: 13 } });
  const morph = ease(clamp01((frame - 28) / 20));
  const launched = spring({ frame: frame - 86, fps, config: { damping: 11, mass: 0.6 } });
  const w = lerp(560, 900, morph);
  const h = lerp(84, 302, morph);
  return (
    <div
      style={{
        position: "absolute",
        top: 250,
        left: 130,
        width: w,
        height: h,
        background: T.card,
        border: `2px solid ${T.borderStrong}`,
        borderRadius: 20,
        boxShadow: "0 10px 34px rgba(32,28,18,0.14)",
        padding: "22px 30px",
        overflow: "hidden",
        opacity: pillIn,
      }}
    >
      <div style={{ fontSize: 27, color: morph > 0.4 ? T.ink : T.inkDim, fontFamily: FONT.mono }}>
        Ship the onboarding flow revamp…
      </div>
      <div style={{ opacity: clamp01((morph - 0.55) / 0.45) }}>
        <div style={{ display: "flex", gap: 14, marginTop: 26, flexWrap: "wrap" }}>
          {["~/work/web-frontend", "opus · high effort", "plan mode"].map((chip) => (
            <span
              key={chip}
              style={{
                fontFamily: FONT.mono,
                fontSize: 21,
                padding: "8px 18px",
                borderRadius: 100,
                background: T.paperDeep,
                border: `1.5px solid ${T.border}`,
                color: T.inkSecondary,
              }}
            >
              {chip}
            </span>
          ))}
        </div>
        <div style={{ marginTop: 30, display: "flex", alignItems: "center", gap: 18 }}>
          <span
            style={{
              fontSize: 26,
              fontWeight: 650,
              padding: "13px 38px",
              borderRadius: 10,
              background: launched > 0.4 ? "#15803d" : T.coral,
              color: "#fff",
            }}
          >
            {launched > 0.4 ? "Session spawned ✓" : "Launch"}
          </span>
          <span style={{ fontFamily: FONT.mono, fontSize: 20, color: T.inkDim, opacity: launched }}>
            detached · headless · yours to watch
          </span>
        </div>
      </div>
    </div>
  );
};
export const Tip5: React.FC = () => (
  <TipShell
    n={5}
    title="Dispatch from anywhere"
    pain={{
      title: "starting task #7 today",
      lines: ["$ cd ~/work/thing && claude", "$ paste 400 lines of context…", "$ repeat for every single task"],
    }}
    bubble="New deckhand, reporting for duty. Didn't even open a terminal."
    side="right"
    pip={{ src: "footage/t5-dispatch.mp4", startFrom: 270, pos: "tr" }}
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
    pip={{ src: "footage/t6-detail.mp4", startFrom: 240, pos: "br" }}
  >
    <Tip6Stage />
  </TipShell>
);

// ── Tip #7 — Your phone is the bridge now ─────────────────────────────────
const Tip7Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const phoneIn = spring({ frame: frame - 4, fps, config: { damping: 13 } });
  const notifDrop = spring({ frame: frame - 22, fps, config: { damping: 12, mass: 0.5 } });
  const tapped = frame >= 68;
  const done = spring({ frame: frame - 72, fps, config: { damping: 11, mass: 0.5 } });
  return (
    <>
      <div
        style={{
          position: "absolute",
          top: 210,
          left: 250,
          width: 380,
          background: T.card,
          border: `3px solid ${T.borderStrong}`,
          borderRadius: 44,
          padding: "18px 16px 26px",
          boxShadow: "0 20px 60px rgba(32,28,18,0.22)",
          opacity: phoneIn,
          transform: `translateY(${lerp(40, 0, phoneIn)}px)`,
        }}
      >
        <div style={{ width: 100, height: 7, borderRadius: 100, background: T.borderStrong, margin: "0 auto 16px" }} />
        <div
          style={{
            background: T.night,
            color: T.nightText,
            borderRadius: 16,
            padding: "14px 16px",
            marginBottom: 14,
            transform: `translateY(${lerp(-90, 0, notifDrop)}px)`,
            opacity: notifDrop,
          }}
        >
          <div style={{ fontSize: 17, fontWeight: 650 }}>Claw Fleet</div>
          <div style={{ fontSize: 15.5, color: T.nightDim }}>Guard: api-server wants to run a migration</div>
        </div>
        <div
          style={{
            background: T.paper,
            border: `2px solid ${T.border}`,
            borderRadius: 16,
            padding: "16px 18px",
            opacity: clamp01((frame - 40) / 12),
          }}
        >
          <div style={{ fontFamily: FONT.mono, fontSize: 14, color: T.coral, marginBottom: 8 }}>
            GUARD · api-server
          </div>
          <div style={{ fontFamily: FONT.mono, fontSize: 16, background: T.night, color: T.nightText, borderRadius: 8, padding: "10px 12px", marginBottom: 12 }}>
            $ npx prisma migrate deploy
          </div>
          <div style={{ display: "flex", gap: 10 }}>
            <span
              style={{
                fontSize: 17,
                fontWeight: 650,
                padding: "9px 22px",
                borderRadius: 8,
                background: tapped ? "#15803d" : T.coral,
                color: "#fff",
                transform: `scale(${tapped ? lerp(1, 1.05, done) : 1})`,
              }}
            >
              {tapped ? "Allowed ✓" : "Allow"}
            </span>
            <span style={{ fontSize: 17, fontWeight: 650, padding: "9px 22px", borderRadius: 8, background: T.paperDeep, color: T.inkSecondary, border: `1.5px solid ${T.border}` }}>
              Block
            </span>
          </div>
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          top: 620,
          left: 700,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: "#15803d",
          opacity: done,
          transform: `translateX(${lerp(-20, 0, done)}px)`,
        }}
      >
        ✓ desktop session unblocked — agent continues
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
    pip={{ src: "footage/t7-guard.mp4", startFrom: 140, pos: "tr" }}
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
    pip={{ src: "footage/t8-report.mp4", startFrom: 180, pos: "tl" }}
  >
    <Tip8Stage />
  </TipShell>
);
