import React from "react";
import { Img, spring, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { Bubble, FadeInOut, PaperStage } from "../ui";

// 150 frames. Headline lands, last word flips coral, captain pops in with the hook.
export const Intro: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const titleIn = spring({ frame, fps, config: { damping: 16, mass: 0.8 } });
  const coralMix = clamp01((frame - 34) / 12);
  const underline = clamp01((frame - 44) / 12);
  const capPop = spring({ frame: frame - 52, fps, config: { damping: 9, mass: 0.8 } });
  const bob = Math.sin(frame / 20) * 6;

  return (
    <FadeInOut duration={150} fadeIn={6} fadeOut={12}>
      <PaperStage>
        <div style={{ position: "absolute", top: 150, left: 120, maxWidth: 1100 }}>
          <div
            style={{
              fontFamily: FONT.mono,
              fontSize: 26,
              color: T.inkDim,
              marginBottom: 26,
              opacity: titleIn,
            }}
          >
            Captain Claw's Field Guide
          </div>
          <h1
            style={{
              fontFamily: FONT.display,
              fontWeight: 600,
              fontSize: 108,
              lineHeight: 1.06,
              letterSpacing: "-0.015em",
              transform: `translateY(${lerp(30, 0, titleIn)}px)`,
              opacity: titleIn,
            }}
          >
            Running AI agents
            <br />
            without losing your{" "}
            <span style={{ position: "relative", color: coralMix > 0.5 ? T.coral : T.ink, fontStyle: "italic" }}>
              mind
              <span
                style={{
                  position: "absolute",
                  left: 0,
                  bottom: -6,
                  height: 8,
                  width: `${underline * 100}%`,
                  background: T.coral,
                  borderRadius: 4,
                  opacity: clamp01(1 - (frame - 120) / 12),
                }}
              />
            </span>
          </h1>
        </div>
        {/* Captain rises from bottom-right, bigger than the tip beats */}
        <div
          style={{
            position: "absolute",
            bottom: -40,
            right: 100,
            width: 400,
            height: 400,
            borderRadius: "50%",
            overflow: "hidden",
            border: `12px solid ${T.night}`,
            boxShadow: "0 22px 60px rgba(32,28,18,0.32)",
            transform: `scale(${capPop}) translateY(${bob}px)`,
            background: T.night,
          }}
        >
          <Img src={staticFile("mascot-captain.png")} style={{ width: "100%", height: "100%" }} />
        </div>
        <Bubble
          text="Nine tips from a crab who's seen agents go very, very wrong."
          enter={62}
          side="right"
          bottom={410}
          width={620}
        />
      </PaperStage>
    </FadeInOut>
  );
};
