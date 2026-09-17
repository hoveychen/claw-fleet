import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useUIStore } from "../store";
import styles from "./Wizard.module.css";

interface Step {
  target: string; // data-wizard attribute value
  titleKey: string;
  descKey: string;
  placement: "right" | "top" | "bottom";
}

// Option B—single-step shortcut: onboarding end goes straight to the Tasks page
// and highlights the "New Session" button, landing guidance on an action that
// truly starts a session rather than idling in the sidebar describing features.
const STEPS: Step[] = [
  {
    target: "new-session-btn",
    titleKey: "wizard.new_session.title",
    descKey: "wizard.new_session.desc",
    placement: "bottom",
  },
];

function getRect(target: string): DOMRect | null {
  const el = document.querySelector(`[data-wizard="${target}"]`);
  return el ? el.getBoundingClientRect() : null;
}

export function Wizard({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation();
  const [step, setStep] = useState(0);
  const [rect, setRect] = useState<DOMRect | null>(null);

  const current = STEPS[step];

  // The "New Session" button only exists on the Tasks (history) page, so switch
  // there before we try to spotlight it.
  useEffect(() => {
    useUIStore.getState().setViewMode("history");
  }, []);

  // The anchor isn't in the DOM until HistoryView mounts after the view switch
  // above, so poll for its rect via rAF instead of measuring once. Give up
  // after a few seconds (a missing anchor just finishes the wizard silently)
  // so it can never spin forever.
  useEffect(() => {
    let raf = 0;
    let tries = 0;
    const MAX_TRIES = 180; // ~3s at 60fps
    const tick = () => {
      const r = getRect(current.target);
      if (r) {
        setRect(r);
        return;
      }
      if (tries++ > MAX_TRIES) {
        onDone();
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [current.target, onDone]);

  const updateRect = useCallback(() => {
    const r = getRect(current.target);
    if (r) setRect(r);
  }, [current.target]);

  useEffect(() => {
    window.addEventListener("resize", updateRect);
    return () => window.removeEventListener("resize", updateRect);
  }, [updateRect]);

  const next = () => {
    if (step < STEPS.length - 1) {
      setStep(step + 1);
    } else {
      onDone();
    }
  };

  const skip = () => onDone();

  if (!rect) return null;

  const pad = 6;
  const spotStyle = {
    top: rect.top - pad,
    left: rect.left - pad,
    width: rect.width + pad * 2,
    height: rect.height + pad * 2,
  };

  // Tooltip position
  let tooltipStyle: React.CSSProperties;
  if (current.placement === "right") {
    tooltipStyle = {
      top: rect.top,
      left: rect.right + 16,
    };
  } else if (current.placement === "bottom") {
    // Anchor sits at the top-right of the toolbar; drop the tooltip below it
    // and pin its right edge to the button so it stays inside the viewport.
    tooltipStyle = {
      top: rect.bottom + 12,
      right: Math.max(12, window.innerWidth - rect.right),
    };
  } else {
    // top
    tooltipStyle = {
      bottom: window.innerHeight - rect.top + 12,
      left: rect.left,
    };
  }

  return (
    <div className={styles.overlay} onClick={skip}>
      {/* Spotlight cutout */}
      <div className={styles.spotlight} style={spotStyle} />

      {/* Tooltip */}
      <div
        className={styles.tooltip}
        style={tooltipStyle}
        onClick={(e) => e.stopPropagation()}
      >
        <div className={styles.tooltip_title}>{t(current.titleKey)}</div>
        <div className={styles.tooltip_desc}>{t(current.descKey)}</div>
        <div className={styles.tooltip_footer}>
          {STEPS.length > 1 && (
            <span className={styles.step_indicator}>
              {step + 1} / {STEPS.length}
            </span>
          )}
          <div className={styles.tooltip_actions}>
            <button className={styles.btn_skip} onClick={skip}>
              {t("wizard.skip")}
            </button>
            <button className={styles.btn_next} onClick={next}>
              {step < STEPS.length - 1 ? t("wizard.next") : t("wizard.done")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
