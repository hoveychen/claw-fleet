import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { Maximize2, Minimize2, MoreHorizontal, X } from "lucide-react";

import type { AuxDocKind } from "../detailAux";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import styles from "./AuxDocBar.module.css";
import { EXIT_MS, Presence } from "./Presence";
import { SkeletonNumber, Spinner } from "./loading";

/**
 * The header every auxiliary-rail reader wears.
 *
 * The four readers (file, wiki doc, web page, deliverable) each used to write
 * their own bar, and the drift showed: the deliverable's said "image · 837 KB"
 * and offered one button while four backend commands to act on it sat unused;
 * the file's hard-coded `sizeBytes: 0` so the size never rendered at all; the
 * wiki's had a version picker but not the copy/export its own page has. One bar
 * with one grammar, so a card can only gain a fact or an action — never a
 * second layout.
 *
 * The grammar is three slots:
 *
 * 1. **The name**, one line, ellipsised — what you would call this thing.
 * 2. **The density line** — dim mono facts (`·`-joined) answering "which one is
 *    this, and how big / which version / from where". The one value worth
 *    scanning for is marked `strong`; the rest recede.
 * 3. **The actions** — at most three icon buttons, then `⋯`.
 *
 * The `⋯` menu and the card's right-click menu are the *same* `menuItems`
 * array, built once per card by `buildAuxDocMenu`. That is the point: a card
 * whose right-click was empty (which is what every rail card's was — the
 * app-wide Settings/About/Quit menu answered instead) and a toolbar missing its
 * own actions were one bug, not two.
 */

/**
 * The expanded card's frame: the reader's own box, plus the right-click that
 * raises the card's menu from anywhere inside it.
 *
 * The anchor is the whole pane, not just its header, because the header is the
 * one part of a card you are *not* looking at when you want to act on what you
 * are reading — right-clicking the poster you just previewed is the natural way
 * to ask to export it. `preventDefault` is what keeps the app-wide
 * Settings/About/Quit menu (`contextMenu.ts`) from answering instead, which is
 * precisely what every rail card used to do.
 */
export function AuxPane({
  menuItems,
  className,
  children,
}: {
  menuItems: ContextMenuItem[];
  className: string;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);
  // "closing" keeps the overlay mounted for its exit animation, with the card
  // still inside it, before the card moves back into the rail.
  const [maxState, setMaxState] = useState<"off" | "on" | "closing">("off");
  const maximized = maxState !== "off";

  // The card's DOM lives in one detached host node that is *moved* between the
  // rail slot and the overlay frame, rather than rendered in two places. Moving
  // the node keeps every reader's React state (a wiki's picked version, a file
  // already read, a scrolled body) — swapping the render site would remount
  // the reader and refetch. `display: contents` keeps the host out of the
  // pane's flex layout.
  const [host] = useState(() => {
    const el = document.createElement("div");
    el.style.display = "contents";
    return el;
  });
  const slotRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const target = maximized ? frameRef.current : slotRef.current;
    if (target && host.parentNode !== target) target.appendChild(host);
  }, [maximized, host]);

  useEffect(() => () => host.remove(), [host]);

  const close = useCallback(() => {
    setMaxState((s) => (s === "on" ? "closing" : s));
  }, []);

  useEffect(() => {
    if (maxState !== "closing") return;
    const timer = setTimeout(() => setMaxState("off"), EXIT_MS);
    return () => clearTimeout(timer);
  }, [maxState]);

  useEffect(() => {
    if (maxState !== "on") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
      }
    };
    // Capture phase, so Esc restores the card before anything behind the
    // overlay (the detail pane, the rail) treats it as its own dismiss.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [maxState, close]);

  const maximize = useMemo<AuxMaximize>(
    () => ({
      maximized,
      toggle: () => (maxState === "on" ? close() : setMaxState("on")),
    }),
    [maximized, maxState, close],
  );

  const onContextMenu = (e: ReactMouseEvent) => {
    // A text field inside a reader keeps its native Copy/Paste menu, the
    // same carve-out the app-wide handler makes.
    if ((e.target as Element | null)?.closest?.("input, textarea, [contenteditable]")) return;
    if (menuItems.length === 0) return;
    e.preventDefault();
    e.stopPropagation();
    setAnchor({ x: e.clientX, y: e.clientY });
  };

  return (
    <AuxMaximizeContext.Provider value={maximize}>
      <div ref={slotRef} className={className} onContextMenu={onContextMenu}>
        {/* React events bubble along the React tree, so right-clicks inside the
            portal still reach this div's handler wherever the host sits. */}
        {createPortal(children, host)}
        {maximized && (
          <div className={styles.max_placeholder}>
            {t("detail.aux_maximized_here", "正在放大窗口中查看")}
          </div>
        )}
        <Presence when={Boolean(anchor)}>{anchor && (
          <ContextMenu anchor={anchor} items={menuItems} onClose={() => setAnchor(null)} />
        )}</Presence>
      </div>
      {maximized &&
        // Portalled to body: the rail sits under transformed ancestors that
        // would otherwise trap a `position: fixed` overlay (see ImageLightbox).
        createPortal(
          <div
            className={styles.max_overlay}
            data-exiting={maxState === "closing" || undefined}
            onClick={close}
            role="dialog"
            aria-modal="true"
          >
            <div
              ref={frameRef}
              className={`${className} ${styles.max_frame}`}
              onClick={(e) => e.stopPropagation()}
            />
          </div>,
          document.body,
        )}
    </AuxMaximizeContext.Provider>
  );
}

/** The pane's maximize switch, read by the bar so every reader gets the button
 *  without each one wiring it. Absent outside an `AuxPane`. */
interface AuxMaximize {
  maximized: boolean;
  toggle: () => void;
}

const AuxMaximizeContext = createContext<AuxMaximize | null>(null);

/** One fact on the density line. `strong` lifts the value the reader scans for
 *  (a size, a version) out of the surrounding context. */
export interface AuxFact {
  text: string;
  strong?: boolean;
  /** Not known yet: renders a placeholder this many px wide instead of `text`. */
  pending?: number;
}

/** A toolbar button. Icon-only — the label is its tooltip and its aria-label,
 *  because three of these plus `⋯` have to fit beside a name in a rail card
 *  that can be 320px wide. */
export interface AuxAction {
  id: string;
  label: string;
  icon: ReactNode;
  /** Swaps the icon for a spinner and blocks the button while the action is outstanding. A native save
   *  panel plus a large copy is a real wait; without this a click that has not
   *  opened its panel yet is indistinguishable from a dead button. */
  busy?: boolean;
  onSelect: () => void;
}

/** How many actions render as buttons. Everything else lives in `⋯` — which
 *  holds the full list anyway, so the cut is about the bar's width, not about
 *  what is reachable. */
export const MAX_BAR_ACTIONS = 3;

export function AuxDocBar({
  kind,
  icon,
  title,
  titleHint,
  facts,
  actions = [],
  menuItems = [],
  onCollapse,
  onClose,
}: {
  /** Colours the glyph, so the four card families are told apart pre-attentively. */
  kind: AuxDocKind;
  icon: ReactNode;
  title: string;
  /** Tooltip on the name — normally the full ref the name was shortened from. */
  titleHint?: string;
  facts: AuxFact[];
  actions?: AuxAction[];
  /** The card's whole menu. Also what its right-click shows. */
  menuItems?: ContextMenuItem[];
  /** Makes the name area the collapse control. Passed when this bar *is* the
   *  expanded card's header — which is the only shape it wears in the rail, so
   *  the card no longer prints its name once in a head strip and again here. */
  onCollapse?: () => void;
  /** Dismiss the card. Rendered as a trailing ✕, held apart from the action
   *  buttons: closing the card is not one more thing to do to the document. */
  onClose?: () => void;
}) {
  const { t } = useTranslation();
  const maximize = useContext(AuxMaximizeContext);
  const moreRef = useRef<HTMLButtonElement>(null);
  const [menuAnchor, setMenuAnchor] = useState<ContextMenuAnchor | null>(null);

  // Pinned to the button's own corner rather than the cursor: this one is a
  // menu button, not a right-click, and a menu that appeared wherever the
  // pointer happened to be would read as having missed.
  const openMenu = () => {
    const r = moreRef.current?.getBoundingClientRect();
    setMenuAnchor(r ? { x: r.left, y: r.bottom + 4 } : { x: 0, y: 0 });
  };

  const shown = actions.slice(0, MAX_BAR_ACTIONS);
  const shownFacts = facts.filter((f) => f.text.length > 0 || f.pending);

  return (
    <div className={styles.bar}>
      {/* One element for the glyph, the name and the facts, so the whole left
          side is the collapse target rather than a name you have to hit. */}
      <button
        type="button"
        className={styles.subject}
        // In the overlay, the header press restores rather than collapsing the
        // card out from under the reader.
        onClick={maximize?.maximized ? maximize.toggle : onCollapse}
        // Without a collapse there is nothing to press; a button that looks
        // pressable and answers nothing is worse than plain text.
        disabled={!onCollapse && !maximize?.maximized}
        title={titleHint ?? title}
        aria-label={
          maximize?.maximized
            ? t("detail.aux_restore", "还原")
            : onCollapse
              ? t("detail.aux_collapse_card", "收起此卡")
              : undefined
        }
      >
        <span className={styles.icon} data-kind={kind} aria-hidden="true">
          {icon}
        </span>
        <span className={styles.text}>
          <span className={styles.name}>{title}</span>
          {shownFacts.length > 0 && (
            <span className={styles.dense}>
              {shownFacts.map((f, i) => (
                <span key={i}>
                  {i > 0 && <span className={styles.dense_sep}> · </span>}
                  {f.pending ? (
                    <SkeletonNumber width={f.pending} height="0.8em" />
                  ) : (
                    <span className={f.strong ? styles.dense_strong : undefined}>{f.text}</span>
                  )}
                </span>
              ))}
            </span>
          )}
        </span>
      </button>
      <div className={styles.actions}>
        {shown.map((a) => (
          <button
            key={a.id}
            type="button"
            className={styles.act}
            onClick={a.onSelect}
            disabled={a.busy}
            aria-busy={a.busy || undefined}
            title={a.label}
            aria-label={a.label}
          >
            {/* The spinner takes the icon's place so the button keeps its size. */}
            {a.busy ? <Spinner size={12} /> : a.icon}
          </button>
        ))}
        {/* Outside the MAX_BAR_ACTIONS cut: reading the thing comfortably is
            the one action every card has, whatever its kind. */}
        {maximize && (
          <button
            type="button"
            className={styles.act}
            onClick={maximize.toggle}
            title={
              maximize.maximized
                ? t("detail.aux_restore", "还原")
                : t("detail.aux_maximize", "放大查看")
            }
            aria-label={
              maximize.maximized
                ? t("detail.aux_restore", "还原")
                : t("detail.aux_maximize", "放大查看")
            }
            aria-pressed={maximize.maximized}
          >
            {maximize.maximized ? (
              <Minimize2 size={13} strokeWidth={1.8} />
            ) : (
              <Maximize2 size={13} strokeWidth={1.8} />
            )}
          </button>
        )}
        {menuItems.length > 0 && (
          <button
            ref={moreRef}
            type="button"
            className={styles.act}
            onClick={openMenu}
            title={t("detail.aux_more", "更多操作")}
            aria-label={t("detail.aux_more", "更多操作")}
            aria-haspopup="menu"
          >
            <MoreHorizontal size={13} strokeWidth={1.8} />
          </button>
        )}
        {onClose && (
          <button
            type="button"
            className={`${styles.act} ${styles.act_close}`}
            onClick={onClose}
            title={t("common.close", "关闭")}
            aria-label={t("common.close", "关闭")}
          >
            <X size={13} strokeWidth={1.8} />
          </button>
        )}
      </div>
      <Presence when={Boolean(menuAnchor)}>{menuAnchor && (
        <ContextMenu anchor={menuAnchor} items={menuItems} onClose={() => setMenuAnchor(null)} />
      )}</Presence>
    </div>
  );
}

/** Whether the reader this is rendered in is maximized. Readers use it to turn
 *  on controls that only make sense with the room — an image's zoom and pan
 *  would otherwise swallow the rail's wheel scrolling. False outside a pane. */
export function useAuxMaximized(): boolean {
  return useContext(AuxMaximizeContext)?.maximized ?? false;
}
