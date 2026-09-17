// The "activity rail" below the session-detail page header.
//
// It replaces a six-tab bar (Messages / Decisions / Plans / Tokens / Workflow / Relay).
// That tab bar on a 390px-wide screen leaves each label with ~46px, and none of the
// six carry numbers—you have to click into each one to know which has content. This
// rail reverses it: it doesn't give six equal-weight entry points, it just says what
// this session is doing *right now*, and each line happens to be an entry to that.
//
// When empty, it doesn't render at all (not as a blank strip)—a quiet session shouldn't
// pay 34px of visual noise. Content rules are in sessionStatusPills.ts.

import { t } from "../i18n";
import type { PillTarget, StatusPill } from "./sessionStatusPills";
import styles from "./StatusRail.module.css";

export function StatusRail({
  pills,
  onOpen,
}: {
  pills: StatusPill[];
  onOpen: (target: PillTarget) => void;
}) {
  if (pills.length === 0) return null;
  return (
    // role=list not nav: these are first and foremost readouts, some of which
    // happen to be clickable. Using nav would make screen readers treat
    // "Context 40%" as a navigation landmark.
    <div className={styles.rail} role="list" aria-label={t("会话状态")}>
      {pills.map((p) =>
        p.target ? (
          <button
            key={p.key}
            type="button"
            role="listitem"
            className={styles.pill}
            data-tone={p.tone}
            onClick={() => onOpen(p.target as PillTarget)}
          >
            {p.dot && <i className={styles.dot} aria-hidden="true" />}
            {p.label}
          </button>
        ) : (
          <span key={p.key} role="listitem" className={styles.pill} data-tone={p.tone}>
            {p.dot && <i className={styles.dot} aria-hidden="true" />}
            {p.label}
          </span>
        ),
      )}
    </div>
  );
}
