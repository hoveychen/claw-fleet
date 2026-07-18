import React from "react";
import { Composition } from "remotion";
import { loadFont as loadFraunces } from "@remotion/google-fonts/Fraunces";
import { loadFont as loadJbMono } from "@remotion/google-fonts/JetBrainsMono";
import { ClawFleetPromo } from "./ClawFleetPromo";
import { ClawFleetPromo2026, ClawFleetVertical, VERTICAL_TOPICS } from "./Promo2026";

loadFraunces();
loadJbMono();

export const RemotionRoot: React.FC = () => (
  <>
    <Composition id="ClawFleetPromo2026" component={ClawFleetPromo2026} durationInFrames={1350} fps={30} width={1920} height={1080} />
    {VERTICAL_TOPICS.map((topic) => (
      <Composition
        key={topic.id}
        id={`ClawFleetVertical-${topic.id}`}
        component={ClawFleetVertical}
        defaultProps={{ topic }}
        durationInFrames={600}
        fps={30}
        width={1080}
        height={1920}
      />
    ))}
    <Composition id="ClawFleetPromoLegacy" component={ClawFleetPromo} durationInFrames={1980} fps={30} width={1920} height={1080} />
  </>
);
