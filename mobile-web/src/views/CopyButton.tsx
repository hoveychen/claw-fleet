// Copy-to-clipboard button for the mobile web app. Mirrors the desktop
// CopyButton state machine (idle → done/failed, auto-reset after a beat) but
// uses the browser `navigator.clipboard` API instead of Tauri's plugin, and
// lucide icons instead of the desktop's glyphs.

import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Copy, X } from "lucide-react";
import { t } from "../i18n";
import styles from "./CopyButton.module.css";

type CopyState = "idle" | "done" | "failed";

/** The write is awaited and its rejection surfaced: a fire-and-forget call
 *  would flash "copied" even when the clipboard write was blocked (insecure
 *  context, permission denied). */
export function CopyButton({ text, className }: { text: string; className?: string }) {
  const [state, setState] = useState<CopyState>("idle");
  const timerRef = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timerRef.current), []);

  const copy = useCallback(
    async (e: React.MouseEvent) => {
      e.stopPropagation();
      window.clearTimeout(timerRef.current);
      try {
        await navigator.clipboard.writeText(text);
        setState("done");
      } catch {
        setState("failed");
      }
      timerRef.current = window.setTimeout(() => setState("idle"), 1400);
    },
    [text],
  );

  const title =
    state === "done" ? t("已复制") : state === "failed" ? t("复制失败") : t("复制");

  return (
    <button
      type="button"
      className={`${styles.copyBtn} ${state === "failed" ? styles.failed : ""} ${className ?? ""}`}
      onClick={(e) => void copy(e)}
      title={title}
      aria-label={title}
    >
      {state === "done" ? (
        <Check size={13} />
      ) : state === "failed" ? (
        <X size={13} />
      ) : (
        <Copy size={13} />
      )}
    </button>
  );
}
