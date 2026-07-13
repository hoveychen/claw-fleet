import React from "react";
import { Series } from "remotion";
import { Intro } from "./scenes/Intro";
import { Outro } from "./scenes/Outro";
import { Tip4 } from "./scenes/Tip4Decide";
import { Tip1, Tip2, Tip3 } from "./scenes/Tips1to4";
import { Tip5, Tip6, Tip7, Tip8 } from "./scenes/Tips5to8";

// 1800 frames @ 30fps = 60s.
// Every tip is two beats: THE PAIN (dark terminal chaos) → the elegant solve,
// with a real screen-capture operation riding picture-in-picture.
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
    <Series.Sequence durationInFrames={210}><Outro /></Series.Sequence>
  </Series>
);
