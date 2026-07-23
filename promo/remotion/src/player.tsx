import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { Player } from "@remotion/player";
import { loadFont as loadFraunces } from "@remotion/google-fonts/Fraunces";
import { loadFont as loadJbMono } from "@remotion/google-fonts/JetBrainsMono";
import { ClawFleetPromo2026, ClawFleetVertical, VERTICAL_TOPICS } from "./Promo2026";

loadFraunces();
loadJbMono();

// GitHub Pages serves this bundle under /<repo>/player/ — anchor staticFile()
// to the bundle's own directory; the default is site-root-absolute and 404s.
// Must be a pathname, not a full URL: staticFile percent-encodes each segment,
// which would mangle "http:" into "http%3A".
(window as unknown as { remotion_staticBase: string }).remotion_staticBase =
  new URL(".", document.baseURI).pathname.replace(/\/$/, "");

const App: React.FC = () => {
  const [mode, setMode] = useState<"horizontal" | "vertical">("horizontal");
  const [topicId, setTopicId] = useState("mobile");
  const topic = VERTICAL_TOPICS.find((item) => item.id === topicId) ?? VERTICAL_TOPICS[0];

  return (
    <main className={`shell shell--${mode}`}>
      <header className="masthead">
        <div className="brand-lockup">
          <img src="./icon.png" alt="" />
          <div><strong>Claw Fleet</strong><span>promo player · 2026 cut</span></div>
        </div>
        <div className="format-switch" role="group" aria-label="Video format">
          <button className={mode === "horizontal" ? "active" : ""} onClick={() => setMode("horizontal")}>45s film</button>
          <button className={mode === "vertical" ? "active" : ""} onClick={() => setMode("vertical")}>9:16 series</button>
        </div>
      </header>

      <section className="stage">
        {mode === "horizontal" ? (
          <Player
            key="horizontal"
            component={ClawFleetPromo2026}
            durationInFrames={1350}
            fps={30}
            compositionWidth={1920}
            compositionHeight={1080}
            controls
            loop
            numberOfSharedAudioTags={8}
            style={{ width: "100%", height: "100%" }}
          />
        ) : (
          <Player
            key={topic.id}
            component={ClawFleetVertical}
            inputProps={{ topic }}
            durationInFrames={600}
            fps={30}
            compositionWidth={1080}
            compositionHeight={1920}
            controls
            loop
            numberOfSharedAudioTags={8}
            style={{ width: "100%", height: "100%" }}
          />
        )}
      </section>

      {mode === "vertical" && (
        <nav className="episode-rail" aria-label="Vertical promo episodes">
          {VERTICAL_TOPICS.map((item, index) => (
            <button key={item.id} className={item.id === topic.id ? "active" : ""} onClick={() => setTopicId(item.id)}>
              <span>{String(index + 1).padStart(2, "0")}</span>{item.eyebrow.replace(/^\d+ · /, "")}
            </button>
          ))}
        </nav>
      )}
    </main>
  );
};

createRoot(document.getElementById("root")!).render(<App />);
