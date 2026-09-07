import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, FileText, Globe, NotebookText } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AuxDoc, AuxDocKind } from "../detailAux";
import type { SessionInfo } from "../types";
import { getItem, setItem } from "../storage";
import { SessionAuxDoc } from "./SessionAuxDoc";
import { SubagentLiveCards } from "./SubagentLiveCards";
import styles from "./SessionDetail.module.css";

const DOC_ICON: Record<AuxDocKind, typeof FileText> = {
  file: FileText,
  wiki: NotebookText,
  web: Globe,
};

/** The expanded card's width, as a fraction of the pane it floats over.
 *
 *  A *ratio*, not a pixel count, because the same app runs on a 13" laptop and
 *  on a 3440px ultrawide: a width that reads as "half the conversation" on one
 *  is a hairline column or a full-screen takeover on the other. The reader's
 *  drag is therefore stored as a proportion and re-resolved against whatever
 *  pane it lands in. */
const RATIO_KEY = "detail-doc-card-ratio";
const RATIO_DEFAULT = 0.46;
const RATIO_MIN = 0.25;
const RATIO_MAX = 0.85;
/** Floor in px, so the ratio cannot squeeze the reader below something a file
 *  path or a web page can actually render in. On a pane narrower than this the
 *  card simply takes what there is (minus the rail's own right offset). */
const CARD_MIN_PX = 300;

function clampRatio(r: number): number {
  return Math.min(RATIO_MAX, Math.max(RATIO_MIN, r));
}

function readRatio(): number {
  const saved = getItem(RATIO_KEY);
  if (saved) {
    const n = Number.parseFloat(saved);
    if (Number.isFinite(n)) return clampRatio(n);
  }
  return RATIO_DEFAULT;
}

/**
 * The auxiliary rail — the *ambient* half of the auxiliary surfaces.
 *
 * One rounded, raised card per thing currently in play: each subagent running
 * right now, and each file / wiki doc / page the agent named that the reader
 * opened. No tabs, no headings, no dividers — a card's own edge and shadow is
 * the only separation it needs, and "how many are there" is answered by
 * counting shapes rather than reading a strip.
 *
 * The cards *float over* the transcript's right side (absolutely positioned
 * inside the messages pane) rather than filling a column beside it. They used
 * to be a second slab in the row, which read as a separate window and — the
 * tell — left the transcript's scrollbar stranded in the middle of the pane
 * with another surface to the right of it. The conversation now keeps the whole
 * pane and its scrollbar keeps the right edge; the reading column just holds
 * clear of the band the *collapsed* cards occupy.
 *
 * **Reading happens here too.** Clicking a doc card expands it in place into a
 * wide floating card that carries the same three readers the 仓库 / 知识库
 * pages use. It used to open the drawer instead, which put the doc's name on
 * screen twice (the drawer's title and the card that had just registered it)
 * and threw a fixed 560px panel over a conversation that, in a narrow pane, was
 * left with a sliver. The drawer is now facet-only.
 *
 * The expanded card is the only thing that grows: the rail's box widens to hold
 * it, the collapsed cards stay `--rail-w` and keep the right edge, and the
 * transcript's reserved band (`--rail-space`) does *not* move — which is what
 * makes this a card floating over the conversation rather than a column
 * squeezing it. Its width is a persisted ratio of the pane, dragged from the
 * handle on its left edge.
 *
 * Clicking a subagent card navigates to that subagent's transcript, the same as
 * it always did.
 */
export function SessionAuxRail({
  open,
  agents,
  docs,
  expandedId,
  onOpenAgent,
  onToggleDoc,
  onCloseDoc,
  onOpenWiki,
}: {
  /** Whether the column is on screen at all. The header's switch owns this;
   *  its default follows the content (see SessionDetail's `railOpen`). */
  open: boolean;
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  docs: AuxDoc[];
  /** The doc card expanded into a reader, if any. */
  expandedId: string | null;
  onOpenAgent: (session: SessionInfo) => void;
  /** Expand a card, or collapse the one already expanded. */
  onToggleDoc: (id: string) => void;
  onCloseDoc: (id: string) => void;
  /** A `[[slug]]` followed from inside a wiki doc opens the next one. */
  onOpenWiki: (slug: string) => void;
}) {
  const { t } = useTranslation();
  const railRef = useRef<HTMLElement | null>(null);
  const [ratio, setRatio] = useState(readRatio);
  // The pane the rail floats in — its offsetParent, i.e. the messages column.
  // Measured rather than assumed because it is what the ratio resolves against,
  // and it changes with the window, the sidebar and the 任务 page's split.
  const [paneW, setPaneW] = useState(0);
  const expandedDoc = docs.find((d) => d.id === expandedId) ?? null;

  useEffect(() => {
    const el = railRef.current;
    const pane = el?.offsetParent;
    if (!(pane instanceof HTMLElement)) return;
    const ro = new ResizeObserver(() => setPaneW(pane.clientWidth));
    ro.observe(pane);
    setPaneW(pane.clientWidth);
    return () => ro.disconnect();
  }, [open, expandedId != null]);

  const onHandleDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    const el = railRef.current;
    const pane = el?.offsetParent;
    if (!(pane instanceof HTMLElement)) return;
    e.preventDefault();
    const rect = pane.getBoundingClientRect();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      // The card hugs the pane's right edge, so its width is "how far the
      // cursor is from that edge" — no start offset to remember.
      setRatio(clampRatio((rect.right - ev.clientX) / Math.max(1, rect.width)));
    };
    const up = () => {
      handle.releasePointerCapture(e.pointerId);
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      setRatio((r) => {
        setItem(RATIO_KEY, r.toFixed(3));
        return r;
      });
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  }, []);

  if (!open) return null;
  const empty = agents.length === 0 && docs.length === 0;
  // Resolved here rather than in CSS: the floor has to win on a narrow pane and
  // the pane's own width is the ceiling, which `clamp()` alone cannot express
  // without also knowing the rail's right offset.
  const avail = Math.max(0, paneW - 20);
  const cardW = expandedDoc
    ? Math.round(Math.min(avail, Math.max(Math.min(CARD_MIN_PX, avail), ratio * paneW)))
    : 0;
  // No drag region on the <aside>: the column is pointer-events:none between
  // the cards so the transcript underneath keeps the wheel and the clicks.
  return (
    <aside
      ref={railRef}
      className={`${styles.rail} ${expandedDoc ? styles.rail_wide : ""}`}
      style={expandedDoc && cardW > 0 ? { width: cardW } : undefined}
    >
      {/* Held open by the switch with nothing in it. Saying so beats an empty
          column, which reads as the rail having failed to load rather than as
          "there is genuinely nothing running and nothing opened yet". */}
      {empty && <p className={styles.rail_empty}>{t("detail.rail_empty", "暂无运行中的 Agent 或已打开的文档")}</p>}
      <SubagentLiveCards agents={agents} onOpen={onOpenAgent} />
      {/* Newest first: the file the agent just named is the one you are most
          likely to be reaching for, and it lands nearest the live agents. */}
      {[...docs].reverse().map((d) => {
        const Icon = DOC_ICON[d.kind];
        const isOpen = d.id === expandedId;
        const head = (
          <>
            <button
              type="button"
              className={styles.doc_card_main}
              onClick={() => onToggleDoc(d.id)}
              title={d.ref}
              aria-expanded={isOpen}
            >
              {isOpen ? (
                <ChevronDown className={styles.doc_card_icon} size={13} strokeWidth={1.8} aria-hidden="true" />
              ) : (
                <Icon className={styles.doc_card_icon} size={13} strokeWidth={1.8} aria-hidden="true" />
              )}
              <span className={styles.doc_card_label}>{d.label}</span>
            </button>
            <button
              type="button"
              className={styles.doc_card_close}
              onClick={() => onCloseDoc(d.id)}
              title={t("common.close", "关闭")}
              aria-label={t("common.close", "关闭")}
            >
              ✕
            </button>
          </>
        );
        if (!isOpen) {
          return (
            <div key={d.id} className={`${styles.rail_card} ${styles.doc_card}`}>
              {head}
            </div>
          );
        }
        return (
          <div key={d.id} className={`${styles.rail_card} ${styles.doc_card_expanded}`}>
            {/* Grabbed from the card's left edge. `separator` with an
                orientation is what a resize grip is; it carries the ratio as
                its value so the drag is legible to assistive tech too. */}
            <div
              className={styles.doc_card_grip}
              onPointerDown={onHandleDown}
              role="separator"
              aria-orientation="vertical"
              aria-label={t("detail.doc_card_resize", "调整卡片宽度")}
            />
            <div className={`${styles.doc_card} ${styles.doc_card_head}`}>{head}</div>
            <SessionAuxDoc doc={d} onOpenWiki={onOpenWiki} onClose={() => onCloseDoc(d.id)} />
          </div>
        );
      })}
    </aside>
  );
}
