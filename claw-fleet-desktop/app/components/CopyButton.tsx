import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import styles from "./CopyButton.module.css";

type CopyState = "idle" | "done" | "failed";

/**
 * Copies `text`, then shows for a moment whether it worked.
 *
 * The write is awaited and its rejection surfaced: `writeText` goes through
 * Tauri's ACL, so a capability lacking `clipboard-manager:allow-write-text`
 * rejects it. A fire-and-forget call would flash "copied" while the clipboard
 * stayed empty.
 */
export function CopyButton({
  text,
  className,
}: {
  text: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<CopyState>("idle");
  const timerRef = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timerRef.current), []);

  const copy = useCallback(async () => {
    window.clearTimeout(timerRef.current);
    try {
      await writeText(text);
      setState("done");
    } catch {
      setState("failed");
    }
    timerRef.current = window.setTimeout(() => setState("idle"), 1400);
  }, [text]);

  const label =
    state === "done"
      ? t("detail.copied")
      : state === "failed"
        ? t("detail.copy_failed")
        : t("detail.copy");

  return (
    <button
      type="button"
      className={`${styles.copy_btn} ${state === "failed" ? styles.copy_btn_failed : ""} ${className ?? ""}`}
      onClick={copy}
      title={label}
      aria-label={label}
    >
      {state === "done" ? "✓" : state === "failed" ? "✕" : "⧉"}
    </button>
  );
}
