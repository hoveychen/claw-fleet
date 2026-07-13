import React from "react";
import { createRoot } from "react-dom/client";
import { Player } from "@remotion/player";
import { loadFont as loadFraunces } from "@remotion/google-fonts/Fraunces";
import { loadFont as loadJbMono } from "@remotion/google-fonts/JetBrainsMono";
import { ClawFleetPromo } from "./ClawFleetPromo";

loadFraunces();
loadJbMono();

// GitHub Pages serves this bundle under /<repo>/player/ — anchor staticFile()
// to the bundle's own directory; the default is site-root-absolute and 404s.
// Must be a pathname, not a full URL: staticFile percent-encodes each segment,
// which would mangle "http:" into "http%3A".
(window as unknown as { remotion_staticBase: string }).remotion_staticBase =
  new URL(".", document.baseURI).pathname.replace(/\/$/, "");

const App: React.FC = () => (
  <Player
    component={ClawFleetPromo}
    durationInFrames={1980}
    fps={30}
    compositionWidth={1920}
    compositionHeight={1080}
    controls
    loop
    style={{ width: "100%", height: "100%" }}
  />
);

createRoot(document.getElementById("root")!).render(<App />);
