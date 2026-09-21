import {
  ChevronDown,
  Copy,
  Crosshair,
  Languages,
  LoaderCircle,
  MessageCircleQuestion,
  PanelRightClose,
  PencilLine,
  Scale,
} from "lucide-react";
import { useState, type PointerEvent as ReactPointerEvent } from "react";
import { useTranslation } from "react-i18next";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import type { ExplainPreset, ExplainRecord } from "../explainApi";
import { cacheHitRatio, costLabel, quoteSnippet } from "../selectionExplain";
import { TextBlock } from "./blocks/TextBlock";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import styles from "./SessionDetail.module.css";

const PRESET_ICON: Record<ExplainPreset, typeof MessageCircleQuestion> = {
  explain: MessageCircleQuestion,
  translate: Languages,
  rationale: Scale,
  custom: PencilLine,
};

/**
 * A side question ("追问") in the auxiliary rail — one card per question the
 * reader asked about a passage of agent prose.
 *
 * Collapsed it is a chip like a doc's: the passage's first line, and the one
 * value that says how the question is doing (spinning while the fork runs, its
 * cost once it has answered, a failure mark otherwise). Expanded it is the
 * whole exchange: the quoted passage (click it to scroll back to and re-select
 * the original), the answer growing as the record file is re-read, the
 * spend line — model, cache share, cost, time — and a box to ask a follow-up,
 * which forks the *session* again with this Q/A folded into the prompt.
 *
 * The answer is rendered with the transcript's own `TextBlock`, so a table or
 * a code span in the explanation reads exactly as it would in the agent's
 * reply. `isPartial` while running keeps the progressive renderer in streaming
 * mode, the same way a live assistant turn does.
 */
export function SessionAuxExplain({
  rec,
  isOpen,
  onToggle,
  onClose,
  onLocate,
  onFollowUp,
  onGripDown,
  onHideRail,
}: {
  rec: ExplainRecord;
  isOpen: boolean;
  onToggle: () => void;
  /** Hide this card for the current view (the record itself stays). */
  onClose: () => void;
  /** Scroll the transcript back to the quoted passage and re-select it. */
  onLocate: () => void;
  /** Ask a follow-up about the same passage, continuing this thread. */
  onFollowUp: (question: string) => void;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
  onHideRail: () => void;
}) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<ContextMenuAnchor | null>(null);
  const [followUp, setFollowUp] = useState("");
  const Icon = PRESET_ICON[rec.preset] ?? MessageCircleQuestion;
  const running = rec.status === "running";
  const failed = rec.status === "error";
  const hit = cacheHitRatio(rec);
  const cost = costLabel(rec.costUsd);

  const copyAnswer = () => {
    writeText(rec.text).catch((e) => console.error("clipboard write failed:", e));
  };

  const menuItems: ContextMenuItem[] = [
    {
      id: "toggle",
      label: isOpen ? t("detail.aux_collapse_card", "收起此卡") : t("detail.aux_expand_card", "展开此卡"),
      icon: <ChevronDown size={13} strokeWidth={1.7} />,
      onSelect: onToggle,
    },
    {
      id: "locate",
      label: t("detail.explain_locate", "定位原文"),
      icon: <Crosshair size={13} strokeWidth={1.7} />,
      onSelect: onLocate,
    },
    ...(rec.text
      ? [
          {
            id: "copy",
            label: t("detail.explain_copy_answer", "复制回答"),
            icon: <Copy size={13} strokeWidth={1.7} />,
            onSelect: copyAnswer,
          } satisfies ContextMenuItem,
        ]
      : []),
    {
      id: "close",
      label: t("common.close", "关闭"),
      dividerBefore: true,
      onSelect: onClose,
    },
    {
      id: "hide-rail",
      label: t("detail.rail_hide", "收起辅助栏"),
      icon: <PanelRightClose size={13} strokeWidth={1.7} />,
      onSelect: onHideRail,
    },
  ];

  const onCardContextMenu = (e: React.MouseEvent) => {
    if ((e.target as Element | null)?.closest?.("input, textarea, [contenteditable]")) return;
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY });
  };

  /** The chip's one value: what the question is doing right now. */
  const status = running ? (
    <span className={`${styles.doc_card_meta} ${styles.explain_running}`}>
      <LoaderCircle size={10} aria-hidden="true" />
      {t("detail.explain_running", "追问中")}
    </span>
  ) : failed ? (
    <span className={`${styles.doc_card_meta} ${styles.explain_failed}`}>
      {t("detail.explain_failed", "失败")}
    </span>
  ) : cost ? (
    <span className={styles.doc_card_meta}>{cost}</span>
  ) : null;

  if (!isOpen) {
    return (
      <div
        className={`${styles.rail_card} ${styles.doc_card}`}
        onContextMenu={onCardContextMenu}
        data-testid="explain-chip"
      >
        <button
          type="button"
          className={styles.doc_card_main}
          onClick={onToggle}
          title={rec.question}
          aria-expanded={false}
        >
          <Icon className={styles.doc_card_icon} data-kind="explain" size={13} strokeWidth={1.8} aria-hidden="true" />
          <span className={styles.doc_card_label}>{quoteSnippet(rec.quote)}</span>
          {status}
        </button>
        <button
          type="button"
          className={styles.doc_card_close}
          onClick={onClose}
          title={t("common.close", "关闭")}
          aria-label={t("common.close", "关闭")}
        >
          ✕
        </button>
        {menu && <ContextMenu anchor={menu} items={menuItems} onClose={() => setMenu(null)} />}
      </div>
    );
  }

  return (
    <div
      className={`${styles.rail_card} ${styles.doc_card_expanded}`}
      onContextMenu={onCardContextMenu}
      data-testid="explain-card"
    >
      <div
        className={styles.doc_card_grip}
        onPointerDown={onGripDown}
        role="separator"
        aria-orientation="vertical"
        aria-label={t("detail.doc_card_resize", "调整卡片宽度")}
      />
      <div className={`${styles.doc_card} ${styles.doc_card_head}`}>
        <button
          type="button"
          className={styles.doc_card_main}
          onClick={onToggle}
          aria-expanded
          title={t("detail.aux_collapse_card", "收起此卡")}
        >
          <ChevronDown className={styles.doc_card_icon} size={13} strokeWidth={1.8} aria-hidden="true" />
          <Icon className={styles.doc_card_icon} data-kind="explain" size={13} strokeWidth={1.8} aria-hidden="true" />
          <span className={styles.doc_card_label}>{rec.question}</span>
          {status}
        </button>
        <button
          type="button"
          className={styles.doc_card_close}
          onClick={onLocate}
          title={t("detail.explain_locate", "定位原文")}
          aria-label={t("detail.explain_locate", "定位原文")}
        >
          <Crosshair size={11} strokeWidth={1.8} aria-hidden="true" />
        </button>
        <button
          type="button"
          className={styles.doc_card_close}
          onClick={onClose}
          title={t("common.close", "关闭")}
          aria-label={t("common.close", "关闭")}
        >
          ✕
        </button>
      </div>
      <div className={styles.explain_pane}>
        {/* The passage, as a link back to where it came from. */}
        <button
          type="button"
          className={styles.explain_quote}
          onClick={onLocate}
          title={t("detail.explain_locate", "定位原文")}
        >
          {rec.quote}
        </button>
        {rec.text ? (
          <div className={styles.explain_answer}>
            <TextBlock text={rec.text} isPartial={running} />
          </div>
        ) : running ? (
          <div className={styles.explain_waiting}>
            <LoaderCircle size={12} aria-hidden="true" />
            {t("detail.explain_waiting", "正在 fork 会话作答…")}
          </div>
        ) : null}
        {failed && (
          <div className={styles.explain_error}>
            {rec.error || t("detail.explain_failed", "失败")}
          </div>
        )}
        {!running && (rec.model || hit != null || cost || rec.durationMs > 0) && (
          <div className={styles.explain_meta}>
            {rec.model && <span>{rec.model}</span>}
            {hit != null && (
              <span title={t("detail.explain_cache_hit", "提示词缓存命中率")}>
                {t("detail.explain_cache_hit_short", "缓存")} {Math.round(hit * 100)}%
              </span>
            )}
            {cost && <span>{cost}</span>}
            {rec.durationMs > 0 && <span>{(rec.durationMs / 1000).toFixed(1)}s</span>}
          </div>
        )}
        {rec.status === "done" && (
          <form
            className={styles.explain_follow}
            onSubmit={(e) => {
              e.preventDefault();
              const q = followUp.trim();
              if (!q) return;
              onFollowUp(q);
              setFollowUp("");
            }}
          >
            <input
              className={styles.explain_follow_input}
              value={followUp}
              onChange={(e) => setFollowUp(e.target.value)}
              placeholder={t("detail.explain_follow_up_placeholder", "继续追问这段话…")}
              aria-label={t("detail.explain_follow_up", "继续追问")}
            />
            <button type="submit" className={styles.explain_follow_send} disabled={!followUp.trim()}>
              {t("detail.explain_send", "发送")}
            </button>
          </form>
        )}
      </div>
      {menu && <ContextMenu anchor={menu} items={menuItems} onClose={() => setMenu(null)} />}
    </div>
  );
}
