import { useTranslation } from "react-i18next";
import { type Theme, useUIStore } from "../store";
import styles from "./ThemeToggle.module.css";

const THEMES: { value: Theme; icon: string }[] = [
  { value: "light", icon: "☀" },
  { value: "dark", icon: "☽" },
  { value: "system", icon: "⊙" },
];

export function ThemeToggle() {
  const { t } = useTranslation();
  const { theme, setTheme } = useUIStore();

  return (
    <div className={styles.group}>
      {THEMES.map(({ value, icon }) => (
        <button
          key={value}
          className={`${styles.btn} ${theme === value ? styles.active : ""}`}
          onClick={() => setTheme(value)}
          title={t(`theme.${value}`)}
        >
          {icon}
        </button>
      ))}
    </div>
  );
}
