import React from "react";
import { spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { TipShell } from "./TipShell";

// ── Tip #5 — Every decision, one inbox ────────────────────────────────────
// Four different decision cards stack into one queue; the counter drains
// 4 → 0 as each gets answered with a single tap.
const CARDS = [
  { kind: "GUARD", icon: "⛨", text: "api-server: allow `npx prisma migrate`?", answer: "Allowed" },
  { kind: "QUESTION", icon: "?", text: '"Feature flag, or ship it straight?"', answer: "Flag it" },
  { kind: "PLAN", icon: "☰", text: "6-step migration plan awaits review", answer: "Approved" },
  { kind: "PERMISSION", icon: "✓", text: "Headless session requests WebFetch", answer: "Allowed" },
];
export const Tip5Inbox: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const resolvedCount = CARDS.filter((_, i) => frame >= 76 + i * 16).length;
  return (
    <TipShell
      n={5}
      title="Every decision, one inbox"
      bubble="Five terminals used to yell at me. Now it's one polite line."
      side="right"
      bubbleEnter={112}
      pip={{ src: "footage/detail.mp4", startFrom: 300, pos: "tr" }}
    >
      {/* queue counter */}
      <div
        style={{
          position: "absolute",
          top: 246,
          left: 120,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: resolvedCount === 4 ? T.green : T.coral,
          fontWeight: 600,
        }}
      >
        {resolvedCount === 4 ? "inbox zero — fleet unblocked ✓" : `${4 - resolvedCount} waiting for you`}
      </div>
      <div style={{ position: "absolute", top: 300, left: 120, display: "grid", gap: 18, width: 700 }}>
        {CARDS.map((c, i) => {
          const inS = spring({ frame: frame - (16 + i * 10), fps, config: { damping: 13, mass: 0.55 } });
          const resolved = frame >= 76 + i * 16;
          return (
            <div
              key={c.kind}
              style={{
                display: "flex",
                gap: 16,
                alignItems: "center",
                background: T.card,
                border: `2px solid ${resolved ? "rgba(21,128,61,0.45)" : T.border}`,
                borderRadius: 14,
                padding: "18px 22px",
                boxShadow: "0 2px 10px rgba(32,28,18,0.09)",
                opacity: inS * (resolved ? 0.82 : 1),
                transform: `translateY(${lerp(26, 0, inS)}px)`,
              }}
            >
              <span
                style={{
                  width: 44,
                  height: 44,
                  borderRadius: 10,
                  display: "grid",
                  placeItems: "center",
                  background: resolved ? "rgba(21,128,61,0.12)" : T.coralSoft,
                  color: resolved ? T.green : T.coral,
                  fontSize: 21,
                  flexShrink: 0,
                }}
              >
                {resolved ? "✓" : c.icon}
              </span>
              <div style={{ minWidth: 0, flex: 1 }}>
                <div style={{ fontFamily: FONT.mono, fontSize: 16, letterSpacing: "0.05em", color: T.inkDim, marginBottom: 3 }}>
                  {c.kind}
                </div>
                <div style={{ fontSize: 23, color: T.ink, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                  {c.text}
                </div>
              </div>
              <span
                style={{
                  fontFamily: FONT.mono,
                  fontSize: 18,
                  fontWeight: 600,
                  color: resolved ? T.green : T.inkDim,
                  opacity: resolved ? 1 : 0.4,
                  whiteSpace: "nowrap",
                }}
              >
                {resolved ? `${c.answer} ✓` : "· · ·"}
              </span>
            </div>
          );
        })}
      </div>
    </TipShell>
  );
};
