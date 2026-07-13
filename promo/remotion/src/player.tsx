import React from "react";
import { createRoot } from "react-dom/client";
import { Player } from "@remotion/player";
import { ClawFleetPromo } from "./ClawFleetPromo";

const App: React.FC = () => (
  <Player
    component={ClawFleetPromo}
    durationInFrames={1800}
    fps={30}
    compositionWidth={1920}
    compositionHeight={1080}
    controls
    loop
    style={{ width: "100%", height: "100%" }}
  />
);

createRoot(document.getElementById("root")!).render(<App />);
