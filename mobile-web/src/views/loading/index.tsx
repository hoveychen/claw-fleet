// Loading primitives (mirror of claw-fleet-desktop/app/components/loading). Rules every consumer follows:
// 1. Loading, empty and error are three distinct states. Never render a
//    fallback value (`0`, `$0.00`, `[]` → "nothing here") before the first
//    response — render a skeleton in its place.
// 2. First load with a known layout → Skeleton*; refetch with content already
//    on screen → keep it and show <TopProgress>; never swap it for "Loading…".
// 3. A button that starts async work shows <Spinner> and refuses re-clicks
//    (see `usePending`).
// 4. Gate with `useDelayedFlag` so sub-200ms waits never flash a loader.
import type { CSSProperties, ReactNode } from "react";
import { t } from "../../i18n";
import styles from "./loading.module.css";

type Len = number | string;

const px = (v: Len | undefined) => (typeof v === "number" ? `${v}px` : v);

interface SkeletonProps {
  width?: Len;
  height?: Len;
  /** Fully round (avatar / status dot). */
  circle?: boolean;
  /** Sit inline with text (numbers, chips) instead of taking a block. */
  inline?: boolean;
  radius?: Len;
  className?: string;
  style?: CSSProperties;
}

/** One shimmering placeholder shape. Purely decorative: wrap groups in an
 *  element carrying `role="status"` (the composite skeletons below do). */
export function Skeleton({
  width,
  height = 10,
  circle,
  inline,
  radius,
  className,
  style,
}: SkeletonProps) {
  const cls = [
    styles.skeleton,
    circle ? styles.circle : "",
    inline ? styles.inline : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <span
      aria-hidden
      className={cls}
      style={{
        width: px(width ?? (circle ? height : "100%")),
        height: px(height),
        borderRadius: px(radius),
        ...style,
      }}
    />
  );
}

/** Accessible wrapper: announces "Loading…" once for the whole group. */
function Busy({
  children,
  className,
  style,
}: {
  children: ReactNode;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label={t("加载中…")}
      className={className}
      style={style}
    >
      {children}
    </div>
  );
}

// Deterministic widths so a skeleton does not reshuffle on every re-render.
const LINE_WIDTHS = ["92%", "84%", "96%", "70%", "88%", "78%"];
const ROW_WIDTHS = ["62%", "78%", "54%", "70%", "66%", "58%", "74%", "50%"];

/** Paragraph placeholder for prose, markdown and file previews. */
export function SkeletonText({
  lines = 3,
  className,
}: {
  lines?: number;
  className?: string;
}) {
  return (
    <Busy className={[styles.text, className ?? ""].join(" ")}>
      {Array.from({ length: lines }, (_, i) => (
        <Skeleton
          key={i}
          height={9}
          width={i === lines - 1 && lines > 1 ? "55%" : LINE_WIDTHS[i % LINE_WIDTHS.length]}
        />
      ))}
    </Busy>
  );
}

/** List / table placeholder. Row height should match the real rows. */
export function SkeletonList({
  rows = 5,
  avatar = false,
  meta = true,
  rowHeight,
  className,
}: {
  rows?: number;
  /** Leading round glyph (status dot, icon). */
  avatar?: boolean;
  /** Second, shorter line under the title. */
  meta?: boolean;
  rowHeight?: number;
  className?: string;
}) {
  return (
    <Busy className={[styles.list, className ?? ""].join(" ")}>
      {Array.from({ length: rows }, (_, i) => (
        <div
          key={i}
          className={styles.list_row}
          style={rowHeight ? { minHeight: rowHeight } : undefined}
        >
          {avatar && <Skeleton circle height={14} />}
          <div className={styles.list_row_body}>
            <Skeleton height={10} width={ROW_WIDTHS[i % ROW_WIDTHS.length]} />
            {meta && <Skeleton height={8} width="32%" />}
          </div>
        </div>
      ))}
    </Busy>
  );
}

/** Block placeholder for cards, charts, image wells and iframes. */
export function SkeletonCard({
  height = 120,
  width,
  className,
}: {
  height?: Len;
  width?: Len;
  className?: string;
}) {
  return (
    <Busy className={className} style={{ width: px(width) }}>
      <Skeleton className={styles.card} height={height} />
    </Busy>
  );
}

/** Inline placeholder for a cost, count or KPI that has not arrived yet.
 *  Size it to the real number so the row does not jump when it lands. */
export function SkeletonNumber({
  width = 40,
  height = "0.9em",
  className,
}: {
  width?: Len;
  height?: Len;
  className?: string;
}) {
  return (
    <span role="status" aria-label={t("加载中…")} className={className}>
      <Skeleton inline width={width} height={height} />
    </span>
  );
}

/** The single rotating ring. */
export function Spinner({
  size = 12,
  className,
  label,
}: {
  size?: number;
  className?: string;
  /** Accessible label; omit when the surrounding button already says it. */
  label?: string;
}) {
  return (
    <span
      role={label ? "status" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={[styles.spinner, className ?? ""].join(" ")}
      style={{ width: size, height: size, borderWidth: size <= 12 ? 1.5 : 2 }}
    />
  );
}

/** 2px indeterminate bar pinned to the top of the nearest positioned ancestor.
 *  Use while stale content stays visible during a refetch / switch. */
export function TopProgress({ active }: { active: boolean }) {
  if (!active) return null;
  return <div className={styles.top_progress} role="progressbar" aria-busy="true" />;
}

export const loadingStyles = styles;
