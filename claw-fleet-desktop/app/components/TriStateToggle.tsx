import { useTranslation } from "react-i18next";
import type { FeatureState } from "../storage";
import styles from "./TriStateToggle.module.css";

interface Props {
  value: FeatureState;
  onChange: (state: FeatureState) => void;
  /** The current recommended default (from FEATURE_DEFAULTS), used to label the
   *  "default" segment — e.g. 默认（开） / Default (On) — so the user can see
   *  what "default" resolves to right now without leaving Settings. */
  defaultOn: boolean;
  disabled?: boolean;
}

/**
 * Segmented on / off / default control for a tristate feature toggle.
 *
 * The "default" segment stores NO explicit value (see storage.setFeatureState),
 * so the feature follows FEATURE_DEFAULTS and any future change to it — that's
 * what lets us re-pick a feature's default later without a per-user migration.
 */
export function TriStateToggle({ value, onChange, defaultOn, disabled }: Props) {
  const { t } = useTranslation();
  const options: { state: FeatureState; label: string }[] = [
    { state: "on", label: t("settings.tri_on") },
    { state: "off", label: t("settings.tri_off") },
    {
      state: "default",
      label: t(defaultOn ? "settings.tri_default_on" : "settings.tri_default_off"),
    },
  ];
  return (
    <div
      className={styles.group}
      role="radiogroup"
      aria-disabled={disabled || undefined}
    >
      {options.map((o) => (
        <button
          key={o.state}
          type="button"
          role="radio"
          aria-checked={value === o.state}
          className={`${styles.segment} ${value === o.state ? styles.active : ""}`}
          disabled={disabled}
          onClick={() => onChange(o.state)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
