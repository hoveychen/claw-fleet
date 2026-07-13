import React from "react";
import { Series } from "remotion";
import { Intro } from "./scenes/Intro";
import { Outro } from "./scenes/Outro";
import { RealUI } from "./scenes/RealUI";
import { Tip1, Tip2, Tip3, Tip4 } from "./scenes/Tips1to4";
import { Tip5, Tip6, Tip7, Tip8 } from "./scenes/Tips5to8";

// 1980 frames @ 30fps = 66s
export const ClawFleetPromo: React.FC = () => (
  <Series>
    <Series.Sequence durationInFrames={150}><Intro /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip1 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip2 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip3 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip4 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip5 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip6 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip7 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><Tip8 /></Series.Sequence>
    <Series.Sequence durationInFrames={180}><RealUI /></Series.Sequence>
    <Series.Sequence durationInFrames={210}><Outro /></Series.Sequence>
  </Series>
);
