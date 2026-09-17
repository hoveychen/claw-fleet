// Theme: follow system / light / dark. JS side resolves "system" (via matchMedia +
// change listener) then always writes result to html[data-theme]. CSS only needs
// one [data-theme="light"] variable set, no media query duplication.

import { useSyncExternalStore } from "react";

export type ThemeSetting = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

const THEME_KEY = "fleet-theme";

function initialSetting(): ThemeSetting {
  const saved = localStorage.getItem(THEME_KEY);
  if (saved === "system" || saved === "light" || saved === "dark") return saved;
  return "system";
}

let setting: ThemeSetting = initialSetting();
const listeners = new Set<() => void>();
const prefersLight = window.matchMedia("(prefers-color-scheme: light)");

function resolve(): ResolvedTheme {
  if (setting === "system") return prefersLight.matches ? "light" : "dark";
  return setting;
}

function apply(): void {
  document.documentElement.dataset.theme = resolve();
  for (const fn of listeners) fn();
}

prefersLight.addEventListener("change", () => {
  if (setting === "system") apply();
});

/** Call once at startup (main.tsx) so the attribute is set before first paint. */
export function initTheme(): void {
  document.documentElement.dataset.theme = resolve();
}

export function getThemeSetting(): ThemeSetting {
  return setting;
}

export function getResolvedTheme(): ResolvedTheme {
  return resolve();
}

export function setThemeSetting(next: ThemeSetting): void {
  if (next === setting) return;
  setting = next;
  localStorage.setItem(THEME_KEY, next);
  apply();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

// Snapshot covers both the setting and the resolved value, so a system-level
// light/dark flip (setting unchanged) still re-renders subscribers.
function snapshot(): string {
  return `${setting}:${resolve()}`;
}

export function useTheme(): {
  setting: ThemeSetting;
  resolved: ResolvedTheme;
  setTheme: (t: ThemeSetting) => void;
} {
  useSyncExternalStore(subscribe, snapshot);
  return { setting, resolved: resolve(), setTheme: setThemeSetting };
}

/** Re-render on theme flips; used where colors are baked into markup (iframe srcDoc). */
export function useResolvedTheme(): ResolvedTheme {
  useSyncExternalStore(subscribe, snapshot);
  return resolve();
}
