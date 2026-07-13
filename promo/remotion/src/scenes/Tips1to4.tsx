import React from "react";
import { interpolate, spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, ease, easeOut, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { ClickRipple, Cursor, SessionCard } from "../ui";
import { TipShell } from "./TipShell";

// NOTE: every stage below renders inside <Sequence from={56}> (TipShell),
// so local frame 0 = the moment the pain window is swept away.

// ── Tip #1 — Know who's actually working ──────────────────────────────────
const Tip1Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cards = [0, 1, 2].map((i) =>
    spring({ frame: frame - (6 + i * 8), fps, config: { damping: 13, mass: 0.6 } })
  );
  const waitingFlip = frame >= 46;
  const pulse = waitingFlip ? 1 + Math.sin((frame - 46) / 6) * 0.012 : 1;
  return (
    <div style={{ position: "absolute", top: 240, left: 120, display: "grid", gap: 24 }}>
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
  );
};
export const Tip1: React.FC = () => (
  <TipShell
    n={1}
    title="Know who's actually working"
    pain={{
      title: "6:02 PM — you finally check",
      lines: [
        "[web-frontend] waiting for input… 47 min",
        "[data-pipeline] $6.40 and climbing",
        "you: which terminal was that again?",
      ],
    }}
    bubble="Three agents coding. One's been waiting since lunch. Now you can see it."
    side="right"
    pip={{ src: "footage/t1-board.mp4", startFrom: 240, pos: "tr" }}
  >
    <Tip1Stage />
  </TipShell>
);

// ── Tip #2 — Watch the bill, not the vibes ────────────────────────────────
const Tip2Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 6, fps, config: { damping: 13 } });
  const dollars = interpolate(frame, [10, 66], [0.4, 3.9], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const stamp = spring({ frame: frame - 82, fps, config: { damping: 11, mass: 0.5 } });
  const bars = 14;
  return (
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
      <div style={{ fontFamily: FONT.mono, fontSize: 96, fontWeight: 600, color: frame > 56 ? T.coral : T.ink }}>
        ${dollars.toFixed(2)}
        <span style={{ fontSize: 40, color: T.inkDim }}>/min</span>
      </div>
      <div style={{ display: "flex", alignItems: "flex-end", gap: 10, height: 130, marginTop: 22 }}>
        {Array.from({ length: bars }, (_, i) => {
          const grow = clamp01((frame - 10 - i * 4) / 12);
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
  );
};
export const Tip2: React.FC = () => (
  <TipShell
    n={2}
    title="Watch the bill, not the vibes"
    pain={{
      title: "the bill arrives friday",
      lines: ["you: how much did today cost?", "provider: :) see invoice", "you: …how much is it costing RIGHT NOW?"],
    }}
    bubble="That's not a feature shipping. That's $4 a minute of apologies."
    side="left"
    pip={{ src: "footage/t2-bill.mp4", startFrom: 210, pos: "tl" }}
  >
    <Tip2Stage />
  </TipShell>
);

// ── Tip #3 — No unsupervised sudo on my ship ──────────────────────────────
const Tip3Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 4, fps, config: { damping: 13 } });
  const blocked = spring({ frame: frame - 72, fps, config: { damping: 10, mass: 0.5 } });
  return (
    <>
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
              background: frame > 62 ? T.red : T.paperDeep,
              color: frame > 62 ? "#fff" : T.inkSecondary,
              border: `2px solid ${frame > 62 ? T.red : T.border}`,
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
          { at: 14, x: 1250, y: 850 },
          { at: 44, x: 245, y: 482 },
          { at: 62, x: 245, y: 482, click: true },
        ]}
      />
      <ClickRipple x={260} y={498} start={62} />
    </>
  );
};
export const Tip3: React.FC = () => (
  <TipShell
    n={3}
    title="No unsupervised sudo on my ship"
    pain={{
      title: "it already ran",
      lines: ["$ rm -rf /var/data/prod-cache", "✓ done in 0.3s", "you: WAIT—"],
    }}
    bubble="Not on my watch, sailor."
    side="right"
    pip={{ src: "footage/t3-audit.mp4", startFrom: 210, pos: "bl" }}
  >
    <Tip3Stage />
  </TipShell>
);
