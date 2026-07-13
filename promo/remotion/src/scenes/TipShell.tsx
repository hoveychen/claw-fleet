import React from "react";
import { FadeInOut, PaperStage, Porthole, Bubble, TipHeader } from "../ui";

// Shared layout for every tip beat: header top-left, stage center,
// captain porthole + speech bubble reaction in a corner.
export const TipShell: React.FC<{
  n: number;
  title: string;
  bubble: string;
  bubbleEnter?: number;
  side?: "left" | "right";
  duration?: number;
  children: React.ReactNode;
}> = ({ n, title, bubble, bubbleEnter = 96, side = "right", duration = 180, children }) => (
  <FadeInOut duration={duration} fadeIn={8} fadeOut={10}>
    <PaperStage>
      <TipHeader n={n} title={title} />
      {children}
      <Porthole side={side} enter={10} />
      <Bubble text={bubble} enter={bubbleEnter} side={side} />
    </PaperStage>
  </FadeInOut>
);
