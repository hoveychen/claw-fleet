import styles from "./ResizeHandle.module.css";

type Props = {
  /** Which edge of the parent the handle sits on. The parent must be
   *  `position: relative`. */
  side?: "left" | "right";
  active?: boolean;
  onMouseDown: (e: React.MouseEvent) => void;
};

/** Column-resize grip for a pane driven by useResizableWidth. */
export function ResizeHandle({ side = "left", active = false, onMouseDown }: Props) {
  return (
    <div
      className={`${styles.handle} ${side === "right" ? styles.handle_left_edge : styles.handle_right_edge} ${active ? styles.handle_active : ""}`}
      onMouseDown={onMouseDown}
    />
  );
}
