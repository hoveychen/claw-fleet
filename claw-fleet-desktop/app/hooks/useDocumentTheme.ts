import { useEffect, useState } from "react";

/**
 * The resolved palette, as a JS value.
 *
 * Almost everything should read theme through CSS custom properties instead.
 * This hook exists for the two places that cannot: a mermaid diagram whose
 * colours are baked into generated SVG, and a decision card's preview iframe,
 * which sits on an opaque origin so the app's custom properties never reach it.
 *
 * `store.applyWindowTheme` stamps the resolved value on `<html>` (absent means
 * dark; see App.css), and the observer re-renders on flips so a diagram or
 * preview doesn't stay dark-on-paper after the user switches themes.
 */
export type DocumentTheme = "dark" | "light";

export function readDocumentTheme(): DocumentTheme {
  return document.documentElement.getAttribute("data-theme") === "light"
    ? "light"
    : "dark";
}

export function useDocumentTheme(): DocumentTheme {
  const [theme, setTheme] = useState(readDocumentTheme);
  useEffect(() => {
    const obs = new MutationObserver(() => setTheme(readDocumentTheme()));
    obs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => obs.disconnect();
  }, []);
  return theme;
}
