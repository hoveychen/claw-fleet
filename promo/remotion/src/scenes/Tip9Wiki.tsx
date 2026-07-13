import React from "react";
import { spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { WindowFrame } from "../ui";
import { TipShell } from "./TipShell";

// Stage renders inside <Sequence from={56}> — local frame 0 = pain swept away.

// ── Tip #9 — Research it once, cite it forever ─────────────────────────────
const PUBLISH_CMD = "$ fleet wiki publish perf.html --slug perf/launchpad";
const LIBRARY: { header?: string; title?: string; slug?: string; hot?: boolean }[] = [
  { header: "arch" },
  { title: "Architecture Overview", slug: "arch/overview" },
  { title: "Backend Trait: Local & Remote", slug: "arch/backend-trait" },
  { header: "research" },
  { title: "Token Usage Deep-Dive", slug: "research/token-usage" },
  { header: "perf" },
  { title: "Launchpad Performance Analysis", slug: "perf/launchpad", hot: true },
];
const Tip9Stage: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const termIn = spring({ frame: frame - 2, fps, config: { damping: 13, mass: 0.6 } });
  const chars = Math.max(0, Math.min(PUBLISH_CMD.length, Math.floor((frame - 6) * 2.6)));
  const ok = clamp01((frame - 34) / 10);
  const cite = spring({ frame: frame - 68, fps, config: { damping: 11, mass: 0.5 } });
  const caption = clamp01((frame - 86) / 12);
  return (
    <>
      <WindowFrame
        title="api-server · session #12"
        width={680}
        style={{
          position: "absolute",
          top: 250,
          left: 120,
          opacity: termIn,
          transform: `translateY(${lerp(30, 0, termIn)}px)`,
        }}
      >
        <div style={{ padding: "22px 26px", fontFamily: FONT.mono, fontSize: 23, lineHeight: 1.85 }}>
          <div style={{ color: T.nightText, minHeight: 42 }}>
            {PUBLISH_CMD.slice(0, chars)}
            <span style={{ opacity: frame % 16 < 8 ? 1 : 0, color: T.coralNight }}>▍</span>
          </div>
          <div style={{ color: "#4ade80", opacity: ok }}>
            ✓ published — v3 (2 earlier versions kept)
          </div>
        </div>
      </WindowFrame>
      {/* The fleet's library: folder tree fills in doc by doc */}
      <div
        style={{
          position: "absolute",
          top: 200,
          left: 900,
          width: 540,
          background: T.card,
          border: `2px solid ${T.border}`,
          borderRadius: 16,
          padding: "22px 28px",
          boxShadow: "0 10px 34px rgba(32,28,18,0.14)",
        }}
      >
        <div style={{ fontFamily: FONT.display, fontSize: 30, fontWeight: 600, marginBottom: 14 }}>
          Wiki · the ship's library
        </div>
        {LIBRARY.map((row, i) => {
          const pop = spring({ frame: frame - (18 + i * 7), fps, config: { damping: 13, mass: 0.5 } });
          if (row.header) {
            return (
              <div
                key={i}
                style={{
                  fontFamily: FONT.mono,
                  fontSize: 17,
                  color: T.inkDim,
                  margin: "10px 0 4px",
                  opacity: pop,
                }}
              >
                ▾ {row.header}
              </div>
            );
          }
          return (
            <div
              key={i}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 12,
                padding: "7px 12px",
                borderRadius: 10,
                border: row.hot ? `2px solid ${T.coral}` : "2px solid transparent",
                background: row.hot ? T.coralSoft : "transparent",
                opacity: pop,
                transform: `translateX(${lerp(26, 0, pop)}px)`,
              }}
            >
              <span
                style={{
                  fontFamily: FONT.mono,
                  fontSize: 13,
                  fontWeight: 700,
                  color: "#15803d",
                  background: "#dcfce7",
                  borderRadius: 5,
                  padding: "2px 7px",
                }}
              >
                MD
              </span>
              <span style={{ fontSize: 22, fontWeight: 600 }}>{row.title}</span>
              {row.hot ? (
                <span style={{ fontFamily: FONT.mono, fontSize: 15, color: T.coral, marginLeft: "auto" }}>
                  just in
                </span>
              ) : null}
            </div>
          );
        })}
      </div>
      {/* Any later session cites it with a [[slug]] */}
      <div
        style={{
          position: "absolute",
          top: 560,
          left: 120,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: T.ink,
          background: T.paperDeep,
          border: `2px solid ${T.borderStrong}`,
          borderRadius: 12,
          padding: "14px 22px",
          opacity: cite,
          transform: `scale(${lerp(0.9, 1, cite)})`,
          transformOrigin: "top left",
        }}
      >
        next week, any session: “see <span style={{ color: T.coral, fontWeight: 700 }}>[[perf/launchpad]]</span>” — no re-research
      </div>
      <div
        style={{
          position: "absolute",
          top: 650,
          left: 120,
          fontFamily: FONT.mono,
          fontSize: 24,
          color: T.green,
          opacity: caption,
          transform: `translateX(${lerp(-16, 0, caption)}px)`,
        }}
      >
        ✓ versioned · full-text searchable · cross-linked
      </div>
    </>
  );
};
export const Tip9: React.FC = () => (
  <TipShell
    n={9}
    title="Research it once, cite it forever"
    pain={{
      title: "that report from last week",
      lines: ["you: where's the perf analysis?", "$ ls ~/Downloads | wc -l  →  3,842", "agent: happy to re-research! (~2M tokens)"],
    }}
    bubble="Every report goes in the ship's log. Next voyage just cites the page."
    side="right"
    pip={{ src: "footage/t9-wiki.mp4", startFrom: 430, pos: "bl", rate: 2.2, zoom: { scale: 1.5, origin: "42% 30%" } }}
  >
    <Tip9Stage />
  </TipShell>
);
