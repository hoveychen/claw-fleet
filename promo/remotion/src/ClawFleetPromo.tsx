import React from "react";
import { Series } from "remotion";
import { Intro } from "./scenes/Intro";
import { Outro } from "./scenes/Outro";
import { Tip5Inbox } from "./scenes/Tip5Inbox";
import { Tip1, Tip2, Tip3, Tip4 } from "./scenes/Tips1to4";
import { Tip5, Tip6, Tip7, Tip8 } from "./scenes/Tips5to8";

// 1980 frames @ 30fps = 66s. Real screen captures ride picture-in-picture
// inside every tip (Pip in TipShell); Tips5to8 exports are numbered 6–9
// on screen since the decision-inbox beat took slot #5.
export const ClawFleetPromo: React.FC = () => (
  <Series>
    <Series.Sequence durationInFrames={150}><Intro /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip1 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip2 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip3 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip4 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip5Inbox /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip5 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip6 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip7 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip8 /></Series.Sequence>
    <Series.Sequence durationInFrames={210}><Outro /></Series.Sequence>
  </Series>
);
