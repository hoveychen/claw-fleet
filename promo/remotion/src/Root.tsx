import React from "react";
import { Composition } from "remotion";
import { loadFont as loadFraunces } from "@remotion/google-fonts/Fraunces";
import { loadFont as loadJbMono } from "@remotion/google-fonts/JetBrainsMono";
import { ClawFleetPromo } from "./ClawFleetPromo";

loadFraunces();
loadJbMono();

export const RemotionRoot: React.FC = () => (
  <Composition
    id="ClawFleetPromo"
    component={ClawFleetPromo}
    durationInFrames={1980}
    fps={30}
    width={1920}
    height={1080}
  />
);
