// 设置 sheet：语言（中/EN）与主题（跟随系统/亮/暗），即时生效并持久到
// localStorage。语言选项用各自母语展示，不随 UI 语言翻译。

import { useI18n, type Lang } from "../i18n";
import { useTheme, type ThemeSetting } from "../theme";
import styles from "./SettingsSheet.module.css";

const LANG_CHOICES: Array<[Lang, string]> = [
  ["zh", "中文"],
  ["en", "English"],
];

export function SettingsSheet({ onClose }: { onClose: () => void }) {
  const { lang, setLang, t } = useI18n();
  const { setting, setTheme } = useTheme();

  const themeChoices: Array<[ThemeSetting, string]> = [
    ["system", t("跟随系统")],
    ["light", t("亮色")],
    ["dark", t("暗色")],
  ];

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()}>
        <div className={styles.head}>
          <span className={styles.title}>{t("设置")}</span>
          <button className={styles.close} onClick={onClose}>
            ×
          </button>
        </div>
        <div className={styles.row}>
          <span className={styles.label}>{t("语言")}</span>
          <div className={styles.segment}>
            {LANG_CHOICES.map(([value, label]) => (
              <button
                key={value}
                className={styles.segmentButton}
                data-active={lang === value}
                onClick={() => setLang(value)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
        <div className={styles.row}>
          <span className={styles.label}>{t("主题")}</span>
          <div className={styles.segment}>
            {themeChoices.map(([value, label]) => (
              <button
                key={value}
                className={styles.segmentButton}
                data-active={setting === value}
                onClick={() => setTheme(value)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
