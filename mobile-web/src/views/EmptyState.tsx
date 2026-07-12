import type { LucideIcon } from "lucide-react";
import styles from "./EmptyState.module.css";

type Props = {
  /** lucide-react icon component, e.g. `Inbox`. */
  icon: LucideIcon;
  /** Bold headline — the one-line "nothing here" statement. */
  title: string;
  /** Optional muted sub-line explaining what would fill this space. */
  description?: string;
  /** Optional call-to-action rendered below the description. */
  action?: React.ReactNode;
  /** Tighter layout for inline / filtered-result empties (search, tabs). */
  compact?: boolean;
};

/**
 * GitHub-blankslate–style empty state: a soft icon chip, a bold title, and a
 * muted description, centered. One component for every "nothing here yet"
 * surface so they read as one system across the app.
 */
export function EmptyState({ icon: Icon, title, description, action, compact }: Props) {
  return (
    <div className={styles.root} data-compact={compact || undefined}>
      <div className={styles.iconChip} aria-hidden="true">
        <Icon size={compact ? 20 : 26} strokeWidth={1.75} />
      </div>
      <div className={styles.title}>{title}</div>
      {description && <div className={styles.description}>{description}</div>}
      {action && <div className={styles.action}>{action}</div>}
    </div>
  );
}
