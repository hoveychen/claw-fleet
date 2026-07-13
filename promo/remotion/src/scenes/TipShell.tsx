import React from "react";
import { FadeInOut, PaperStage, Pip, PipProps, Porthole, Bubble, TipHeader } from "../ui";

// Shared layout for every tip beat: header top-left, stage center,
// captain porthole + speech bubble reaction in a corner, and an optional
// real-screen-capture picture-in-picture window.
export const TipShell: React.FC<{
  n: number;
  title: string;
  bubble: string;
  bubbleEnter?: number;
  side?: "left" | "right";
  duration?: number;
  pip?: PipProps;
  children: React.ReactNode;
}> = ({ n, title, bubble, bubbleEnter = 96, side = "right", duration = 180, pip, children }) => (
  <FadeInOut duration={duration} fadeIn={8} fadeOut={10}>
    <PaperStage>
      <TipHeader n={n} title={title} />
      {children}
      {pip ? <Pip {...pip} /> : null}
      <Porthole side={side} enter={10} />
      <Bubble text={bubble} enter={bubbleEnter} side={side} />
    </PaperStage>
  </FadeInOut>
);
