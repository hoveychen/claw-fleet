import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import zh from "./locales/zh.json";
import { getItem } from "./storage";

/** The languages this bundle actually ships a translation for.
 *
 *  Which languages exist is the frontend's fact, not the backend's — core
 *  reports whatever `FLEET_LOCALE` said (normalised to a language subtag) and
 *  leaves the "do you have a bundle for it" question here, next to the imports
 *  above that answer it. */
export const SUPPORTED_LANGUAGES = ["en", "zh"] as const;

export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

export function isSupportedLanguage(code: string | null | undefined): code is SupportedLanguage {
  return !!code && (SUPPORTED_LANGUAGES as readonly string[]).includes(code);
}

/**
 * Detect the initial interface language. Three-tier precedence (same as simplified mode):
 * explicit choice stored on this client is highest; second is the host default from last boot
 * (backend's `FLEET_LOCALE`, delivered via `host_features`); if neither, fall back to the
 * browser's own language.
 *
 * Why cache the host answer: `host_features` is an async request after boot, but language
 * must resolve synchronously at i18next init time. Waiting for that request means the first
 * page load draws the English UI, then switches to Chinese. Caching lets subsequent loads
 * start in the right language. The cache refreshes at each boot, so when the host changes
 * its mind (or stops expressing one), the cache follows — it never becomes a sticky toggle
 * the user can't undo.
 */
function detectLanguage(): string {
  const saved = getItem("lang");
  if (saved) return saved;
  const hostDefault = getItem("lang-host-default");
  if (isSupportedLanguage(hostDefault)) return hostDefault;
  const locale = navigator.language || "";
  return locale.startsWith("zh") ? "zh" : "en";
}

const savedLang = detectLanguage();

i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    zh: { translation: zh },
  },
  lng: savedLang,
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

export default i18n;
