// A 资料 error line: the localized message, and — when that message is the
// generic one — the host's original text underneath, small, for debugging.
// The box itself (`className`) stays the caller's, so each screen keeps its
// own error styling.

import type { LibraryErrorView } from "../libraryErrors";
import styles from "./LibraryErrorNote.module.css";

export function LibraryErrorNote({
  error,
  className,
  onDismiss,
}: {
  error: LibraryErrorView | null;
  className?: string;
  onDismiss?: () => void;
}) {
  if (!error) return null;
  return (
    <div className={className} role="alert" onClick={onDismiss} data-library-error>
      <span>{error.message}</span>
      {error.detail && <span className={styles.detail}>{error.detail}</span>}
    </div>
  );
}
