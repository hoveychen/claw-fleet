import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./Wizard.module.css";

interface Step {
  target: string; // data-wizard attribute value
  titleKey: string;
  descKey: string;
  placement: "right" | "top";
}

const STEPS: Step[] = [
  {
    target: "view-toggle",
    titleKey: "wizard.view_toggle.title",
    descKey: "wizard.view_toggle.desc",
    placement: "right",
  },
  {
    target: "token-speed",
    titleKey: "wizard.token_speed.title",
    descKey: "wizard.token_speed.desc",
    placement: "right",
  },
  {
    target: "settings-footer",
    titleKey: "wizard.account_info.title",
    descKey: "wizard.account_info.desc",
    placement: "top",
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

  const updateRect = useCallback(() => {
    setRect(getRect(current.target));
  }, [current.target]);

  useEffect(() => {
    updateRect();
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
          <span className={styles.step_indicator}>
            {step + 1} / {STEPS.length}
          </span>
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
