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
 * 界面语言的初值。三级,和精简模式同构:这个客户端存过的**显式**选择最高,其次
 * 是这台主机上次给出的默认值(后端的 `FLEET_LOCALE`,经 host_features 送来),
 * 都没有才回落到浏览器自己的语言。
 *
 * 为什么要缓存主机的答案:`host_features` 是启动后一次异步请求,而语言必须在
 * i18next init 时同步定下来 —— 只等那次请求的话,每次打开都会先画一帧英文界面
 * 再跳成中文。缓存让第二次之后的加载直接就位;主机的答案每次加载都会刷新这份
 * 缓存,所以主机改口(或不再表态)时它跟着改口,不会变成一个撤不掉的粘滞开关。
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
