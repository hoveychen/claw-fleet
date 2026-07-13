import React from "react";
import { spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { ClickRipple, Cursor } from "../ui";
import { TipShell } from "./TipShell";

// ── Tip #4 — Answer with a click, not an essay ────────────────────────────
// The agent proactively asks "what next?" WITH options. One click, it sails on.
const OPTIONS = [
  { label: "Ship it behind a flag", desc: "Deploy dark, flip later" },
  { label: "Write the migration tests", desc: "Cover the risky path first" },
  { label: "Refactor the session store", desc: "Pay the debt now" },
];
const PICK = 1; // option index the cursor picks
const Tip4Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cardIn = spring({ frame: frame - 4, fps, config: { damping: 13 } });
  const picked = frame >= 64;
  const continueS = spring({ frame: frame - 78, fps, config: { damping: 12, mass: 0.5 } });
  return (
    <>
      <div
        style={{
          position: "absolute",
          top: 236,
          left: 120,
          width: 780,
          background: T.card,
          border: `2px solid ${T.border}`,
          borderLeft: `6px solid ${T.coral}`,
          borderRadius: 16,
          padding: "28px 34px",
          boxShadow: "0 8px 30px rgba(32,28,18,0.12)",
          opacity: cardIn,
          transform: `translateY(${lerp(30, 0, cardIn)}px)`,
        }}
      >
        <div style={{ fontFamily: FONT.mono, fontSize: 21, letterSpacing: "0.06em", color: T.coral, marginBottom: 10 }}>
          AGENT QUESTION · claw-fleet
        </div>
        <div style={{ fontSize: 30, fontWeight: 650, marginBottom: 20 }}>
          P3 is green. Pick my next move:
        </div>
        <div style={{ display: "grid", gap: 14 }}>
          {OPTIONS.map((o, i) => {
            const inS = clamp01((frame - 12 - i * 7) / 10);
            const isPick = i === PICK && picked;
            return (
              <div
                key={o.label}
                style={{
                  border: `2px solid ${isPick ? T.coral : T.border}`,
                  background: isPick ? T.coralSoft : T.paperDeep,
                  borderRadius: 12,
                  padding: "14px 20px",
                  opacity: inS,
                  transform: `translateY(${lerp(14, 0, inS)}px)`,
                }}
              >
                <div style={{ fontSize: 24, fontWeight: 650, color: isPick ? T.coral : T.ink }}>
                  {o.label} {isPick ? "✓" : ""}
                </div>
                <div style={{ fontSize: 19, color: T.inkSecondary }}>{o.desc}</div>
              </div>
            );
          })}
        </div>
        <div
          style={{
            marginTop: 20,
            fontFamily: FONT.mono,
            fontSize: 22,
            color: T.green,
            opacity: continueS,
            transform: `translateX(${lerp(-16, 0, continueS)}px)`,
          }}
        >
          ✓ one click — agent continues with the tests
        </div>
      </div>
      <Cursor
        keys={[
          { at: 26, x: 1150, y: 900 },
          { at: 50, x: 480, y: 560 },
          { at: 64, x: 480, y: 560, click: true },
        ]}
      />
      <ClickRipple x={495} y={575} start={64} />
    </>
  );
};
export const Tip4: React.FC = () => (
  <TipShell
    n={4}
    title="Answer with a click, not an essay"
    pain={{
      title: "typing on a phone keyboard",
      lines: [
        "agent: what should I do next?",
        "you: typing… (paragraph 2 of 3)",
        "autocorrect: 'migrate' → 'migraine'",
      ],
    }}
    bubble="It asks. You click. It sails on. No essays."
    side="right"
    pip={{ src: "footage/t4-ask.mp4", startFrom: 230, pos: "tr", zoom: { scale: 1.3, origin: "50% 94%" } }}
  >
    <Tip4Stage />
  </TipShell>
);
