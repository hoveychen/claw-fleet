import { useTranslation } from "react-i18next";
import styles from "./LanguageSwitcher.module.css";

const LANGS = [
  { code: "en", label: "EN" },
  { code: "zh", label: "中" },
];

export function LanguageSwitcher() {
  const { i18n } = useTranslation();

  const change = (code: string) => {
    i18n.changeLanguage(code);
    localStorage.setItem("lang", code);
  };

  return (
    <div className={styles.group}>
      {LANGS.map(({ code, label }) => (
        <button
          key={code}
          className={`${styles.btn} ${i18n.language === code ? styles.active : ""}`}
          onClick={() => change(code)}
        >
          {label}
        </button>
      ))}
    </div>
  );
}
