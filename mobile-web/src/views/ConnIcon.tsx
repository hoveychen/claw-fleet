// Header connection mark — replaces the old dot + text label ("桌面端在线" /
// "在线 · 网络拥挤" / …) with a glyph, so the banner's right edge stays legible
// on a narrow phone without spending a third of the header width on prose.
//
// Two dimensions have to survive the collapse to an icon:
//   1. link strength — how congested this phone↔relay↔desktop round trip is
//   2. reachability  — relay socket down (connecting), or up but the desktop
//                      agent isn't there
// (1) is a 3-bar signal glyph with 1/2/3 bars lit; (2) gets its own shape, so
// "the desktop is off" never reads as "your signal is weak". The text lives on
// as `title` + `aria-label`, so nothing is actually lost — it just stops
// occupying pixels.

import type { Congestion } from "../connQuality";

/** Which glyph the header should draw. Pure so the mapping is unit-testable —
 *  the rendering is not. */
export type ConnIconKind =
  | "connecting" // link not up yet, still retrying
  | "offline" // link down and not coming back on its own (Fleet Cloud only)
  | "desktop-offline" // relay up, desktop agent absent
  | "good"
  | "fair"
  | "congested";

export function connIconKind(
  connected: boolean,
  agentOnline: boolean,
  congestion: Congestion,
): ConnIconKind {
  if (!connected) return "connecting";
  if (!agentOnline) return "desktop-offline";
  return congestion;
}

/** Bars lit, out of 3, for the signal glyph. */
const LIT: Record<Exclude<ConnIconKind, "connecting" | "offline" | "desktop-offline">, number> = {
  good: 3,
  fair: 2,
  congested: 1,
};

const SIZE = 16;

/** Three ascending bars; `lit` of them use the state colour, the rest are drawn
 *  faintly so the glyph keeps the same silhouette at every level (a shrinking
 *  icon reads as "moved", not as "weaker"). */
function SignalBars({ lit }: { lit: number }) {
  // x, y, height — ascending staircase inside a 16×16 box.
  const bars = [
    { x: 1, y: 10, h: 5 },
    { x: 6, y: 6.5, h: 8.5 },
    { x: 11, y: 3, h: 12 },
  ];
  return (
    <svg viewBox="0 0 16 16" width={SIZE} height={SIZE} aria-hidden>
      {bars.map((b, i) => (
        <rect
          key={b.x}
          x={b.x}
          y={b.y}
          width={4}
          height={b.h}
          rx={1.2}
          fill="currentColor"
          opacity={i < lit ? 1 : 0.22}
        />
      ))}
    </svg>
  );
}

/** Empty bars with a slash — the socket isn't up. The caller animates it. */
function SignalOff() {
  return (
    <svg viewBox="0 0 16 16" width={SIZE} height={SIZE} aria-hidden>
      <rect x={1} y={10} width={4} height={5} rx={1.2} fill="currentColor" opacity={0.22} />
      <rect x={6} y={6.5} width={4} height={8.5} rx={1.2} fill="currentColor" opacity={0.22} />
      <rect x={11} y={3} width={4} height={12} rx={1.2} fill="currentColor" opacity={0.22} />
      <path
        d="M2 14.5 L14.5 2"
        stroke="currentColor"
        strokeWidth={1.6}
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}

/** A monitor with its screen struck through: the link is fine, the desktop end
 *  is not. Deliberately a different silhouette from the bars. */
function DesktopOff() {
  return (
    <svg viewBox="0 0 16 16" width={SIZE} height={SIZE} aria-hidden>
      <rect
        x={1.2}
        y={2.5}
        width={13.6}
        height={9}
        rx={1.6}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.4}
      />
      <path d="M5.5 14h5" stroke="currentColor" strokeWidth={1.4} strokeLinecap="round" />
      <path
        d="M4.5 9.5 L11.5 4.5"
        stroke="currentColor"
        strokeWidth={1.4}
        strokeLinecap="round"
      />
    </svg>
  );
}

export function ConnIcon({ kind }: { kind: ConnIconKind }) {
  if (kind === "connecting" || kind === "offline") return <SignalOff />;
  if (kind === "desktop-offline") return <DesktopOff />;
  return <SignalBars lit={LIT[kind]} />;
}
