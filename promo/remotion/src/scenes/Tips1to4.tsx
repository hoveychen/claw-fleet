import React from "react";
import { interpolate, spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, ease, easeOut, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { Badge, ClickRipple, Cursor, SessionCard, WindowFrame } from "../ui";
import { TipShell } from "./TipShell";

// ── Tip #1 — Know who's actually working ──────────────────────────────────
// Three session cards stagger in; the third flips to "Waiting" and pulses.
export const Tip1: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cards = [0, 1, 2].map((i) =>
    spring({ frame: frame - (14 + i * 9), fps, config: { damping: 13, mass: 0.6 } })
  );
  const waitingFlip = frame >= 60;
  const pulse = waitingFlip ? 1 + Math.sin((frame - 60) / 6) * 0.012 : 1;
  return (
    <TipShell
      n={1}
      title="Know who's actually working"
      bubble="Three agents coding. One's been waiting for you since lunch."
      side="right"
    >
      <div style={{ position: "absolute", top: 240, left: 120, display: "grid", gap: 26 }}>
        <SessionCard
          name="api-server"
          badge={{ kind: "executing", label: "Executing" }}
          line="Fix JWT validation in auth middleware…"
          stats="$3.12 · 27.0 tok/s · ctx 42%"
          style={{ opacity: cards[0], transform: `translateY(${lerp(28, 0, cards[0])}px)` }}
        />
        <SessionCard
          name="data-pipeline"
          badge={{ kind: "delegating", label: "Delegating" }}
          line="4 subagents — partitioning strategy rewrite…"
          stats="$6.40 · 84.1 tok/s · ctx 58%"
          style={{ opacity: cards[1], transform: `translateY(${lerp(28, 0, cards[1])}px)` }}
        />
        <SessionCard
          name="web-frontend"
          badge={
            waitingFlip
              ? { kind: "waiting", label: "Waiting for you" }
              : { kind: "thinking", label: "Thinking" }
          }
          line='"Should I proceed with breaking changes?"'
          stats="$1.84 · idle 47 min"
          highlight={waitingFlip}
          style={{ opacity: cards[2], transform: `translateY(${lerp(28, 0, cards[2])}px) scale(${pulse})` }}
        />
      </div>
    </TipShell>
  );
};

// ── Tip #2 — Watch the bill, not the vibes ────────────────────────────────
// $/min ticker climbs, sparkline rises, then a big STOP lands on the loop.
export const Tip2: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 14, fps, config: { damping: 13 } });
  const dollars = interpolate(frame, [20, 92], [0.4, 3.9], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const stamp = spring({ frame: frame - 104, fps, config: { damping: 11, mass: 0.5 } });
  const bars = 14;
  return (
    <TipShell
      n={2}
      title="Watch the bill, not the vibes"
      bubble="That's not a feature shipping. That's $4 a minute of apologies."
      side="left"
      bubbleEnter={104}
    >
      <div
        style={{
          position: "absolute",
          top: 250,
          right: 150,
          width: 760,
          background: T.card,
          border: `2px solid ${T.border}`,
          borderRadius: 18,
          padding: "36px 44px",
          boxShadow: "0 8px 30px rgba(32,28,18,0.12)",
          opacity: cardIn,
          transform: `translateY(${lerp(30, 0, cardIn)}px)`,
        }}
      >
        <div style={{ fontFamily: FONT.mono, fontSize: 24, color: T.inkDim, marginBottom: 8 }}>
          fleet spend · rolling 5 min
        </div>
        <div style={{ fontFamily: FONT.mono, fontSize: 96, fontWeight: 600, color: frame > 80 ? T.coral : T.ink }}>
          ${dollars.toFixed(2)}
          <span style={{ fontSize: 40, color: T.inkDim }}>/min</span>
        </div>
        <div style={{ display: "flex", alignItems: "flex-end", gap: 10, height: 130, marginTop: 22 }}>
          {Array.from({ length: bars }, (_, i) => {
            const grow = clamp01((frame - 20 - i * 5) / 14);
            const h = lerp(8, 14 + Math.pow(i / bars, 2.2) * 116, easeOut(grow));
            return (
              <div
                key={i}
                style={{
                  width: 38,
                  height: h,
                  borderRadius: 6,
                  background: i > bars - 4 ? T.coral : T.paperDeep,
                  border: `1.5px solid ${T.border}`,
                }}
              />
            );
          })}
        </div>
        {/* STOP stamp */}
        <div
          style={{
            position: "absolute",
            top: 40,
            right: 44,
            fontFamily: FONT.mono,
            fontSize: 40,
            fontWeight: 700,
            color: T.red,
            border: `5px solid ${T.red}`,
            borderRadius: 12,
            padding: "6px 22px",
            transform: `rotate(-10deg) scale(${stamp})`,
            opacity: stamp,
            background: "rgba(251,250,247,0.9)",
          }}
        >
          STOPPED
        </div>
      </div>
    </TipShell>
  );
};

// ── Tip #3 — No unsupervised sudo on my ship ──────────────────────────────
// Guard card slides in; cursor glides to Block; click ripple; BLOCKED stamp.
export const Tip3: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 12, fps, config: { damping: 13 } });
  const blocked = spring({ frame: frame - 96, fps, config: { damping: 10, mass: 0.5 } });
  return (
    <TipShell
      n={3}
      title="No unsupervised sudo on my ship"
      bubble="Not on my watch, sailor."
      side="right"
      bubbleEnter={108}
    >
      <div
        style={{
          position: "absolute",
          top: 260,
          left: 130,
          width: 900,
          background: T.card,
          border: `2px solid ${T.border}`,
          borderLeft: `6px solid ${T.coral}`,
          borderRadius: 16,
          padding: "30px 36px",
          boxShadow: "0 8px 30px rgba(32,28,18,0.12)",
          opacity: cardIn,
          transform: `translateY(${lerp(30, 0, cardIn)}px)`,
        }}
      >
        <div style={{ fontFamily: FONT.mono, fontSize: 22, letterSpacing: "0.06em", color: T.coral, marginBottom: 16 }}>
          GUARD · COMMAND APPROVAL · data-pipeline
        </div>
        <div
          style={{
            fontFamily: FONT.mono,
            fontSize: 30,
            background: T.night,
            color: T.nightText,
            borderRadius: 10,
            padding: "18px 24px",
            marginBottom: 24,
          }}
        >
          $ rm -rf <span style={{ color: "#fca5a5" }}>/var/data/prod-cache</span>
        </div>
        <div style={{ display: "flex", gap: 16 }}>
          <span
            style={{
              fontSize: 26,
              fontWeight: 650,
              padding: "12px 34px",
              borderRadius: 10,
              background: frame > 86 ? T.red : T.paperDeep,
              color: frame > 86 ? "#fff" : T.inkSecondary,
              border: `2px solid ${frame > 86 ? T.red : T.border}`,
            }}
          >
            Block
          </span>
          <span
            style={{
              fontSize: 26,
              fontWeight: 650,
              padding: "12px 34px",
              borderRadius: 10,
              background: T.paperDeep,
              color: T.inkSecondary,
              border: `2px solid ${T.border}`,
            }}
          >
            Allow once
          </span>
        </div>
        <div
          style={{
            position: "absolute",
            top: 30,
            right: 46,
            fontFamily: FONT.mono,
            fontSize: 44,
            fontWeight: 700,
            color: T.red,
            border: `6px solid ${T.red}`,
            borderRadius: 12,
            padding: "8px 26px",
            transform: `rotate(8deg) scale(${blocked})`,
            opacity: blocked,
            background: "rgba(251,250,247,0.92)",
          }}
        >
          BLOCKED
        </div>
      </div>
      <Cursor
        keys={[
          { at: 30, x: 1250, y: 850 },
          { at: 66, x: 245, y: 462 },
          { at: 86, x: 245, y: 462, click: true },
        ]}
      />
      <ClickRipple x={260} y={478} start={86} />
    </TipShell>
  );
};

// ── Tip #4 — Read the plan before the chaos ───────────────────────────────
// Plan approval card: steps appear; the reckless one gets edited; Approve.
const PLAN_FIX = "4. Migrate users table (behind flag)";
export const Tip4: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 12, fps, config: { damping: 13 } });
  const steps = ["1. Audit current call sites", "2. Add compat shim", "3. Swap middleware impl"];
  const strike = clamp01((frame - 62) / 12);
  const typedChars = Math.max(0, Math.min(PLAN_FIX.length, Math.floor((frame - 80) / 1.6)));
  const approved = spring({ frame: frame - 132, fps, config: { damping: 11, mass: 0.5 } });
  return (
    <TipShell
      n={4}
      title="Read the plan before the chaos"
      bubble={'Step 4 used to say "rewrite everything". I fixed step 4.'}
      side="left"
      bubbleEnter={118}
    >
      <div
        style={{
          position: "absolute",
          top: 240,
          right: 140,
          width: 860,
          background: T.card,
          border: `2px solid ${T.border}`,
          borderRadius: 16,
          padding: "30px 40px",
          boxShadow: "0 8px 30px rgba(32,28,18,0.12)",
          opacity: cardIn,
          transform: `translateY(${lerp(30, 0, cardIn)}px)`,
        }}
      >
        <div style={{ fontFamily: FONT.mono, fontSize: 22, letterSpacing: "0.06em", color: T.coral, marginBottom: 18 }}>
          PLAN APPROVAL · web-frontend
        </div>
        {steps.map((s, i) => {
          const inAt = clamp01((frame - 22 - i * 8) / 10);
          return (
            <div key={s} style={{ fontSize: 28, marginBottom: 14, opacity: inAt, color: T.ink }}>
              {s} <span style={{ color: T.green, opacity: clamp01((frame - 34 - i * 8) / 8) }}>✓</span>
            </div>
          );
        })}
        {/* the edited line */}
        <div style={{ fontSize: 28, marginBottom: 18, position: "relative" }}>
          <span style={{ color: T.inkDim, textDecoration: strike > 0.5 ? "line-through" : "none" }}>
            4. Drop and recreate users table
          </span>
          {typedChars > 0 ? (
            <div style={{ color: T.coral, fontWeight: 600 }}>
              {PLAN_FIX.slice(0, typedChars)}
              <span style={{ opacity: frame % 16 < 8 ? 1 : 0 }}>▎</span>
            </div>
          ) : null}
        </div>
        <span
          style={{
            display: "inline-block",
            fontSize: 26,
            fontWeight: 650,
            padding: "12px 36px",
            borderRadius: 10,
            background: approved > 0.4 ? T.green : T.coral,
            color: "#fff",
            transform: `scale(${lerp(1, 1.06, approved)})`,
          }}
        >
          {approved > 0.4 ? "Approved ✓" : "Approve plan"}
        </span>
      </div>
    </TipShell>
  );
};
