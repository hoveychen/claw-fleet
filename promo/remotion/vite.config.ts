import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Builds the <Player> embed bundle (dist/). The Remotion CLI (studio/render)
// does not use this config.
export default defineConfig({
  plugins: [react()],
  base: "./",
});
