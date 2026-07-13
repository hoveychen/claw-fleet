import React from "react";
import { OffthreadVideo, staticFile, spring, useCurrentFrame, useVideoConfig } from "remotion";
import { clamp01, lerp } from "../helpers";
import { FONT, T } from "../tokens";
import { Bubble, FadeInOut, PaperStage, Porthole, WindowFrame } from "../ui";

// 180 frames. Real screen recordings of the actual app (mock-data mode):
// first the live gallery board, then a session detail. "Not a mockup" beat.
export const RealUI: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const titleIn = spring({ frame: frame - 6, fps, config: { damping: 14 } });
  const showDetail = frame >= 90;
  const crossfade = clamp01((frame - 84) / 12);
  return (
    <FadeInOut duration={180} fadeIn={8} fadeOut={10}>
      <PaperStage>
        <div
          style={{
            position: "absolute",
            top: 74,
            left: 90,
            fontFamily: FONT.display,
            fontSize: 58,
            fontWeight: 600,
            letterSpacing: "-0.01em",
            opacity: titleIn,
            transform: `translateX(${lerp(-60, 0, titleIn)}px)`,
          }}
        >
          And no — <span style={{ color: T.coral, fontStyle: "italic" }}>not a mockup</span>
        </div>
        <div style={{ position: "absolute", top: 190, left: 150 }}>
          <WindowFrame title="Claw Fleet — live session board" width={1380}>
            <div style={{ position: "relative", height: 700, overflow: "hidden" }}>
              <OffthreadVideo
                muted
                src={staticFile("footage/gallery.mp4")}
                startFrom={210}
                style={{
                  width: "100%",
                  height: "100%",
                  objectFit: "cover",
                  objectPosition: "top left",
                  opacity: 1 - crossfade,
                }}
              />
              {showDetail ? (
                <OffthreadVideo
                  muted
                  src={staticFile("footage/detail.mp4")}
                  startFrom={330}
                  style={{
                    position: "absolute",
                    inset: 0,
                    width: "100%",
                    height: "100%",
                    objectFit: "cover",
                    objectPosition: "top right",
                    opacity: crossfade,
                  }}
                />
              ) : null}
            </div>
          </WindowFrame>
        </div>
        <Porthole side="right" enter={12} size={260} bottom={46} />
        <Bubble
          text="Yes, it really looks this calm. That's the point."
          enter={104}
          side="right"
          bottom={150}
          width={520}
        />
      </PaperStage>
    </FadeInOut>
  );
};
