import React from "react";
import { Img, interpolate, spring, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, ease, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { FadeInOut } from "../ui";

// 210 frames. Full cockpit art settles in, brand lockup lands, long hold.
export const Outro: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const drift = interpolate(frame, [0, 210], [1.06, 1.0], { easing: ease });
  const lockup = spring({ frame: frame - 34, fps, config: { damping: 15, mass: 0.8 } });
  const url = clamp01((frame - 70) / 16);
  return (
    <FadeInOut duration={210} fadeIn={14} fadeOut={0}>
      <div style={{ position: "absolute", inset: 0, background: T.nightDeep, overflow: "hidden" }}>
        <Img
          src={staticFile("mascot-cockpit.png")}
          style={{
            position: "absolute",
            inset: 0,
            width: "100%",
            height: "100%",
            objectFit: "cover",
            transform: `scale(${drift})`,
            opacity: 0.9,
          }}
        />
        <div
          style={{
            position: "absolute",
            inset: 0,
            background: "linear-gradient(180deg, rgba(8,9,10,0.15) 0%, rgba(8,9,10,0.05) 40%, rgba(8,9,10,0.88) 82%)",
          }}
        />
        <div
          style={{
            position: "absolute",
            bottom: 90,
            left: 0,
            right: 0,
            textAlign: "center",
            transform: `translateY(${lerp(30, 0, lockup)}px)`,
            opacity: lockup,
          }}
        >
          <div
            style={{
              fontFamily: FONT.display,
              fontSize: 92,
              fontWeight: 600,
              color: T.nightText,
              letterSpacing: "-0.01em",
              marginBottom: 10,
            }}
          >
            Claw Fleet
          </div>
          <div style={{ fontFamily: FONT.body, fontSize: 34, color: "rgba(247,248,248,0.85)", marginBottom: 26 }}>
            Mission control for your AI coding agents
          </div>
          <div style={{ fontFamily: FONT.mono, fontSize: 26, color: T.coralNight, opacity: url }}>
            github.com/hoveychen/claw-fleet · free &amp; open-source
          </div>
        </div>
      </div>
    </FadeInOut>
  );
};
