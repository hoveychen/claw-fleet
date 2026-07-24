import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { FindController } from "../find/useFindController";
import styles from "./FindBar.module.css";

/**
 * The floating Cmd/Ctrl+F find bar. Purely presentational — all search state and
 * highlighting live in {@link FindController}. Rendered at the app root so a
 * single bar serves whatever view is on screen.
 */
export function FindBar({ controller }: { controller: FindController }) {
  const { t } = useTranslation();
  const { open, query, total, activeIndex, supported, setQuery, next, prev, close } = controller;
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [open]);

  if (!open) return null;

  const noMatches = query.length > 0 && total === 0;

  return (
    <div className={styles.bar} data-find-bar role="search">
      <input
        ref={inputRef}
        className={`${styles.input} ${noMatches ? styles.empty : ""}`}
        type="text"
        value={query}
        placeholder={t("find.placeholder", "在页面中查找")}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (e.shiftKey) prev();
            else next();
          } else if (e.key === "Escape") {
            e.preventDefault();
            close();
          }
        }}
      />
      {supported ? (
        <span className={styles.count}>
          {total > 0 ? `${activeIndex + 1}/${total}` : "0/0"}
        </span>
      ) : (
        <span className={styles.unsupported}>{t("find.unsupported", "此环境不支持高亮")}</span>
      )}
      <button
        className={styles.btn}
        onClick={prev}
        disabled={total === 0}
        title={t("find.prev", "上一个")}
        aria-label={t("find.prev", "上一个")}
      >
        ↑
      </button>
      <button
        className={styles.btn}
        onClick={next}
        disabled={total === 0}
        title={t("find.next", "下一个")}
        aria-label={t("find.next", "下一个")}
      >
        ↓
      </button>
      <button
        className={styles.btn}
        onClick={close}
        title={t("find.close", "关闭")}
        aria-label={t("find.close", "关闭")}
      >
        ✕
      </button>
    </div>
  );
}
