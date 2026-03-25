import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import zh from "./locales/zh.json";
import { getItem } from "./storage";

function detectLanguage(): string {
  const saved = getItem("lang");
  if (saved) return saved;
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
