import React from "react";
import {
  AbsoluteFill,
  Img,
  OffthreadVideo,
  interpolate,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { clamp01, ease, easeOut, lerp } from "./helpers";
import { FONT, T } from "./tokens";

// ── Stage: warm paper background with the product's subtle grain ──────────
export const PaperStage: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <AbsoluteFill style={{ background: T.paper, fontFamily: FONT.body, color: T.ink }}>
    {children}
  </AbsoluteFill>
);

// ── Tip header card: "Tip #N — title" slides in from top-left ─────────────
export const TipHeader: React.FC<{ n: number; title: string; enter?: number }> = ({
  n,
  title,
  enter = 0,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({ frame: frame - enter, fps, config: { damping: 14, mass: 0.6 } });
  return (
    <div
      style={{
        position: "absolute",
        top: 70,
        left: 90,
        display: "flex",
        alignItems: "baseline",
        gap: 26,
        transform: `translateX(${lerp(-80, 0, s)}px)`,
        opacity: s,
      }}
    >
      <span
        style={{
          fontFamily: FONT.mono,
          fontSize: 30,
          fontWeight: 600,
          color: T.coral,
          background: T.coralSoft,
          border: `2px solid ${T.coral}`,
          borderRadius: 12,
          padding: "8px 22px",
        }}
      >
        Tip #{n}
      </span>
      <span
        style={{
          fontFamily: FONT.display,
          fontSize: 58,
          fontWeight: 600,
          letterSpacing: "-0.01em",
        }}
      >
        {title}
      </span>
    </div>
  );
};

// ── Captain porthole: circular crop of the mascot art, spring pop ─────────
export const Porthole: React.FC<{
  size?: number;
  enter?: number;
  side?: "left" | "right";
  bottom?: number;
}> = ({ size = 300, enter = 8, side = "right", bottom = 70 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const pop = spring({ frame: frame - enter, fps, config: { damping: 10, mass: 0.7 } });
  const bob = Math.sin((frame - enter) / 22) * 5;
  return (
    <div
      style={{
        position: "absolute",
        bottom,
        [side]: 80,
        width: size,
        height: size,
        borderRadius: "50%",
        overflow: "hidden",
        border: `10px solid ${T.night}`,
        boxShadow: "0 18px 50px rgba(32,28,18,0.3)",
        transform: `scale(${pop}) translateY(${bob}px)`,
        background: T.night,
      }}
    >
      <Img src={staticFile("mascot-captain.png")} style={{ width: "100%", height: "100%" }} />
    </div>
  );
};

// ── Comic speech bubble anchored next to the porthole ─────────────────────
export const Bubble: React.FC<{
  text: string;
  enter?: number;
  side?: "left" | "right";
  bottom?: number;
  width?: number;
}> = ({ text, enter = 26, side = "right", bottom = 210, width = 560 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({ frame: frame - enter, fps, config: { damping: 12, mass: 0.5 } });
  const tailSide = side === "right" ? { right: 60 } : { left: 60 };
  return (
    <div
      style={{
        position: "absolute",
        bottom,
        [side]: 320,
        width,
        background: T.card,
        border: `3px solid ${T.ink}`,
        borderRadius: 22,
        padding: "24px 30px",
        fontSize: 30,
        lineHeight: 1.4,
        fontWeight: 550,
        boxShadow: "6px 6px 0 rgba(32,28,18,0.18)",
        transform: `scale(${s})`,
        transformOrigin: side === "right" ? "bottom right" : "bottom left",
        opacity: s,
      }}
    >
      {text}
      <div
        style={{
          position: "absolute",
          bottom: -19,
          ...tailSide,
          width: 32,
          height: 32,
          background: T.card,
          borderRight: `3px solid ${T.ink}`,
          borderBottom: `3px solid ${T.ink}`,
          transform: "rotate(45deg) skew(12deg, 12deg)",
        }}
      />
    </div>
  );
};

// ── Status badge (app-identical) ───────────────────────────────────────────
export const Badge: React.FC<{ kind: keyof typeof T.badge; label: string }> = ({ kind, label }) => (
  <span
    style={{
      fontFamily: FONT.mono,
      fontSize: 22,
      fontWeight: 600,
      padding: "6px 18px",
      borderRadius: 100,
      background: T.badge[kind].bg,
      color: T.badge[kind].fg,
      whiteSpace: "nowrap",
    }}
  >
    {label}
  </span>
);

// ── Session card (rebuilt app UI) ──────────────────────────────────────────
export const SessionCard: React.FC<{
  name: string;
  badge: { kind: keyof typeof T.badge; label: string };
  line: string;
  stats: string;
  style?: React.CSSProperties;
  highlight?: boolean;
}> = ({ name, badge, line, stats, style, highlight }) => (
  <div
    style={{
      background: T.card,
      border: highlight ? `3px solid ${T.coral}` : `2px solid ${T.border}`,
      borderRadius: 16,
      padding: "26px 30px",
      width: 520,
      boxShadow: highlight ? `0 14px 40px ${T.coralSoft}` : "0 2px 8px rgba(32,28,18,0.09)",
      ...style,
    }}
  >
    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 12 }}>
      <span style={{ fontSize: 30, fontWeight: 700 }}>{name}</span>
      <Badge kind={badge.kind} label={badge.label} />
    </div>
    <div style={{ fontSize: 23, color: T.inkSecondary, marginBottom: 14, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
      {line}
    </div>
    <div style={{ fontFamily: FONT.mono, fontSize: 20, color: T.inkDim }}>{stats}</div>
  </div>
);

// ── Dark terminal / app window frame ───────────────────────────────────────
export const WindowFrame: React.FC<{
  title?: string;
  children: React.ReactNode;
  width?: number;
  style?: React.CSSProperties;
}> = ({ title, children, width = 980, style }) => (
  <div
    style={{
      width,
      background: T.night,
      borderRadius: 18,
      overflow: "hidden",
      border: `2px solid ${T.borderStrong}`,
      boxShadow: "0 24px 70px rgba(32,28,18,0.3)",
      ...style,
    }}
  >
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 9,
        padding: "14px 18px",
        background: T.nightDeep,
        borderBottom: "1px solid rgba(247,248,248,0.1)",
      }}
    >
      <i style={{ width: 14, height: 14, borderRadius: "50%", background: "rgba(247,248,248,0.14)" }} />
      <i style={{ width: 14, height: 14, borderRadius: "50%", background: "rgba(247,248,248,0.14)" }} />
      <i style={{ width: 14, height: 14, borderRadius: "50%", background: "rgba(247,248,248,0.14)" }} />
      {title ? (
        <span style={{ marginLeft: 12, fontFamily: FONT.mono, fontSize: 18, color: T.nightDim }}>{title}</span>
      ) : null}
    </div>
    {children}
  </div>
);

// ── Cursor that glides along keyframed positions, with click scale ────────
export type CursorKey = { at: number; x: number; y: number; click?: boolean };
export const Cursor: React.FC<{ keys: CursorKey[] }> = ({ keys }) => {
  const frame = useCurrentFrame();
  const xs = keys.map((k) => k.at);
  const x = interpolate(frame, xs, keys.map((k) => k.x), {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const y = interpolate(frame, xs, keys.map((k) => k.y), {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const clickKey = keys.find((k) => k.click && frame >= k.at && frame < k.at + 8);
  const scale = clickKey ? 0.8 : 1;
  const visible = frame >= keys[0].at - 4;
  if (!visible) return null;
  return (
    <svg
      width={44}
      height={44}
      viewBox="0 0 24 24"
      style={{
        position: "absolute",
        left: x,
        top: y,
        transform: `scale(${scale})`,
        filter: "drop-shadow(0 3px 6px rgba(32,28,18,0.4))",
        zIndex: 50,
      }}
    >
      <path d="M4 2 L20 12 L12.5 13.5 L16 21 L13 22 L9.5 14.5 L4 19 Z" fill={T.ink} stroke={T.card} strokeWidth={1.4} />
    </svg>
  );
};

// ── Coral click ripple ─────────────────────────────────────────────────────
export const ClickRipple: React.FC<{ x: number; y: number; start: number }> = ({ x, y, start }) => {
  const frame = useCurrentFrame();
  const t = clamp01((frame - start) / 16);
  if (t <= 0 || t >= 1) return null;
  const eased = easeOut(t);
  const size = lerp(20, 130, eased);
  return (
    <div
      style={{
        position: "absolute",
        left: x - size / 2,
        top: y - size / 2,
        width: size,
        height: size,
        borderRadius: "50%",
        border: `3px solid ${T.coral}`,
        opacity: (1 - eased) * 0.6,
        zIndex: 49,
      }}
    />
  );
};

// ── Picture-in-picture: real screen capture in a mini window ───────────────
export type PipProps = {
  src: string;
  startFrom?: number;
  pos: "tl" | "tr" | "bl" | "br";
  objectPosition?: string;
  enter?: number;
};
const PIP_POS: Record<PipProps["pos"], React.CSSProperties> = {
  tl: { top: 200, left: 90 },
  tr: { top: 200, right: 90 },
  bl: { bottom: 70, left: 90 },
  br: { bottom: 70, right: 90 },
};
export const Pip: React.FC<PipProps> = ({ src, startFrom = 210, pos, objectPosition = "top left", enter = 34 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({ frame: frame - enter, fps, config: { damping: 13, mass: 0.6 } });
  return (
    <div
      style={{
        position: "absolute",
        ...PIP_POS[pos],
        width: 480,
        borderRadius: 14,
        overflow: "hidden",
        border: `2px solid ${T.borderStrong}`,
        boxShadow: "0 18px 50px rgba(32,28,18,0.28)",
        background: T.night,
        transform: `scale(${s})`,
        opacity: s,
        transformOrigin: pos.includes("l") ? "bottom left" : "bottom right",
        zIndex: 40,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "8px 12px",
          background: T.nightDeep,
          borderBottom: "1px solid rgba(247,248,248,0.1)",
        }}
      >
        <i style={{ width: 9, height: 9, borderRadius: "50%", background: "rgba(247,248,248,0.16)" }} />
        <i style={{ width: 9, height: 9, borderRadius: "50%", background: "rgba(247,248,248,0.16)" }} />
        <i style={{ width: 9, height: 9, borderRadius: "50%", background: "rgba(247,248,248,0.16)" }} />
        <span style={{ marginLeft: 8, fontFamily: FONT.mono, fontSize: 13, color: T.nightDim }}>
          the real app · live capture
        </span>
      </div>
      <div style={{ height: 252, overflow: "hidden" }}>
        <OffthreadVideo
          muted
          src={staticFile(src)}
          startFrom={startFrom}
          style={{ width: "100%", height: "100%", objectFit: "cover", objectPosition }}
        />
      </div>
    </div>
  );
};

// ── Fade wrapper for scene transitions ────────────────────────────────────
export const FadeInOut: React.FC<{
  children: React.ReactNode;
  duration: number;
  fadeIn?: number;
  fadeOut?: number;
}> = ({ children, duration, fadeIn = 10, fadeOut = 10 }) => {
  const frame = useCurrentFrame();
  const opacity = Math.min(clamp01(frame / fadeIn), clamp01((duration - frame) / fadeOut));
  return <AbsoluteFill style={{ opacity }}>{children}</AbsoluteFill>;
};
