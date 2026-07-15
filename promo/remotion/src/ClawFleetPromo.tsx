import React from "react";
import { Series, Audio, staticFile } from "remotion";
import { Intro } from "./scenes/Intro";
import { Outro } from "./scenes/Outro";
import { Tip4 } from "./scenes/Tip4Decide";
import { Tip1, Tip2, Tip3 } from "./scenes/Tips1to4";
import { Tip5, Tip6, Tip7, Tip8 } from "./scenes/Tips5to8";
import { Tip9 } from "./scenes/Tip9Wiki";

// 1980 frames @ 30fps = 66s.
// Every tip is two beats: THE PAIN (dark terminal chaos) → the elegant solve,
// with a real screen-capture operation riding picture-in-picture.
//
// Audio:
//   • Per-scene voiceover (edge-tts en-US-ChristopherNeural) lives inside each
//     Series.Sequence, so it plays from that scene's first frame automatically —
//     no manual timecodes, and it follows the scene if durations ever change.
//   • One original sea-shanty BGM bed (audio-build/compose_bgm.py) rides the whole
//     composition. It is already side-chain ducked in audio-build/render_bgm.sh —
//     the music dips ~10 dB while the captain speaks and lifts back up in the gaps —
//     so here it only needs a static level to sit under the voiceover.

// Level of the pre-ducked music bed. VO clips average ~-20 dB. 0.55 lands the
// music at ~-20 dB in the gaps (full and present) and ~-32 dB under speech
// (well beneath the voice), because the duck is already baked into the file.
const BGM_VOLUME = 0.55;

// Each scene paired with its narration clip in public/audio/.
const VO: Record<string, string> = {
  intro: "audio/vo-intro.mp3",
  tip1: "audio/vo-tip1.mp3",
  tip2: "audio/vo-tip2.mp3",
  tip3: "audio/vo-tip3.mp3",
  tip4: "audio/vo-tip4.mp3",
  tip5: "audio/vo-tip5.mp3",
  tip6: "audio/vo-tip6.mp3",
  tip7: "audio/vo-tip7.mp3",
  tip8: "audio/vo-tip8.mp3",
  tip9: "audio/vo-tip9.mp3",
  outro: "audio/vo-outro.mp3",
};

const Narrated: React.FC<{ id: keyof typeof VO; children: React.ReactNode }> = ({
  id,
  children,
}) => (
  <>
    {children}
    <Audio src={staticFile(VO[id])} />
  </>
);

export const ClawFleetPromo: React.FC = () => (
  <>
    {/* Music bed: full-composition, ducked under the narration. */}
    <Audio src={staticFile("audio/bgm.mp3")} volume={BGM_VOLUME} />
    <Series>
      <Series.Sequence durationInFrames={150}>
        <Narrated id="intro"><Intro /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip1"><Tip1 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip2"><Tip2 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip3"><Tip3 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip4"><Tip4 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip5"><Tip5 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip6"><Tip6 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip7"><Tip7 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip8"><Tip8 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={180}>
        <Narrated id="tip9"><Tip9 /></Narrated>
      </Series.Sequence>
      <Series.Sequence durationInFrames={210}>
        <Narrated id="outro"><Outro /></Narrated>
      </Series.Sequence>
    </Series>
  </>
);
