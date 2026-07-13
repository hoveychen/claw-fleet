import React from "react";
import { Sequence } from "remotion";
import { FadeInOut, PaperStage, PainWindow, Pip, PipProps, Porthole, Bubble, TipHeader } from "../ui";

export type Pain = { title: string; lines: string[] };

// Two-beat tip: THE PAIN (dark terminal chaos, frames 0–~60) is swept away,
// then the elegant solve plays. Real screen capture rides picture-in-picture
// during the solve. Captain watches throughout and quips at the end.
export const TipShell: React.FC<{
  n: number;
  title: string;
  pain: Pain;
  bubble: string;
  bubbleEnter?: number;
  side?: "left" | "right";
  duration?: number;
  pip?: PipProps;
  children: React.ReactNode;
}> = ({ n, title, pain, bubble, bubbleEnter = 130, side = "right", duration = 180, pip, children }) => (
  <FadeInOut duration={duration} fadeIn={8} fadeOut={10}>
    <PaperStage>
      <TipHeader n={n} title={title} />
      <PainWindow title={pain.title} lines={pain.lines} exitAt={50} />
      <Sequence from={56}>{children}</Sequence>
      {pip ? (
        <Sequence from={56}>
          <Pip {...pip} enter={16} />
        </Sequence>
      ) : null}
      <Porthole side={side} enter={10} />
      <Bubble text={bubble} enter={bubbleEnter} side={side} />
    </PaperStage>
  </FadeInOut>
);
