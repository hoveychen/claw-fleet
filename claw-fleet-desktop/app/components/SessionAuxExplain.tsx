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
 * "Copy answer" on a chain card: the whole conversation, not just the turn the
 * reader happens to see last — Q/A pairs in asking order, which is what
 * pasting it into a commit message or a chat has to carry.
 */
export function chainAnswerText(records: readonly ExplainRecord[]): string {
  return records
    .filter((r) => r.text)
    .map((r) => (r.question ? `${r.question}\n\n${r.text}` : r.text))
    .join("\n\n---\n\n");
}

/**
 * A side-question ("追问") chain in the auxiliary rail — one card per
 * conversation the reader had about a passage of agent prose.
 *
 * One card per *chain*, not per record: a follow-up is its own record on the
 * host side (the fork is never resumed; the prior Q/A is folded into a new
 * one), so a card per record spent one of the rail's ten slots per turn and
 * stacked the turns newest-first, above the questions they answered.
 *
 * Collapsed it is a chip like a doc's: the passage's first line, and the one
 * value that says how the chain is doing (spinning while a fork runs, its cost
 * once it has answered, a failure mark otherwise) — read off the *latest* turn.
 * Expanded it is the whole exchange: the quoted passage once (click it to
 * scroll back to and re-select the original), then every turn in asking order
 * with its own spend line — model, cache share, cost, time — and one box at
 * the foot to ask a follow-up, which forks the *session* again with the chain
 * folded into the prompt.
 *
 * The answers are rendered with the transcript's own `TextBlock`, so a table or
 * a code span in the explanation reads exactly as it would in the agent's
 * reply. `isPartial` while running keeps the progressive renderer in streaming
 * mode, the same way a live assistant turn does.
 */
export function SessionAuxExplain({
  records,
  isOpen,
  onToggle,
  onClose,
  onLocate,
  onFollowUp,
  onGripDown,
  onHideRail,
}: {
  /** One chain, oldest turn first. */
  records: ExplainRecord[];
  isOpen: boolean;
  onToggle: () => void;
  /** Hide this card for the current view (the records themselves stay). */
  onClose: () => void;
  /** Scroll the transcript back to the quoted passage and re-select it. */
  onLocate: () => void;
  /** Ask a follow-up about the same passage, continuing this chain. */
  onFollowUp: (question: string) => void;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
  onHideRail: () => void;
}) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<ContextMenuAnchor | null>(null);
  const [followUp, setFollowUp] = useState("");
  // The chip and the head summarise the chain: its opening question and quote,
  // but the latest turn's state — a chain whose follow-up is still forking
  // reads as running, not as the first answer's cost.
  const first = records[0];
  const last = records[records.length - 1];
  const Icon = PRESET_ICON[first.preset] ?? MessageCircleQuestion;
  const running = last.status === "running";
  const failed = last.status === "error";
  const cost = costLabel(last.costUsd);

  const copyAnswer = () => {
    writeText(chainAnswerText(records)).catch((e) => console.error("clipboard write failed:", e));
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
    ...(records.some((r) => r.text)
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

  /** The chip's one value: what the chain's latest turn is doing right now. */
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
          title={first.question}
          aria-expanded={false}
        >
          <Icon className={styles.doc_card_icon} data-kind="explain" size={13} strokeWidth={1.8} aria-hidden="true" />
          <span className={styles.doc_card_label}>{quoteSnippet(first.quote)}</span>
          {records.length > 1 && (
            <span className={styles.doc_card_meta}>{t("detail.explain_turns", "{{count}} 轮", { count: records.length })}</span>
          )}
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
          <span className={styles.doc_card_label}>{first.question}</span>
          {records.length > 1 && (
            <span className={styles.doc_card_meta}>{t("detail.explain_turns", "{{count}} 轮", { count: records.length })}</span>
          )}
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
        {/* The passage, as a link back to where it came from. Once per chain:
            every turn quotes the same one. */}
        <button
          type="button"
          className={styles.explain_quote}
          onClick={onLocate}
          title={t("detail.explain_locate", "定位原文")}
        >
          {first.quote}
        </button>
        {records.map((rec) => (
          <ExplainTurn key={rec.id} rec={rec} withQuestion={records.length > 1} />
        ))}
        {last.status === "done" && (
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

/** One turn of a chain: its question (only once the chain has more than one,
 *  where the head's single question line no longer covers it), the answer as
 *  the record file is re-read, and that turn's own spend line. */
function ExplainTurn({ rec, withQuestion }: { rec: ExplainRecord; withQuestion: boolean }) {
  const { t } = useTranslation();
  const running = rec.status === "running";
  const hit = cacheHitRatio(rec);
  const cost = costLabel(rec.costUsd);
  return (
    <div className={styles.explain_turn}>
      {withQuestion && rec.question && (
        <div className={styles.explain_turn_question}>{rec.question}</div>
      )}
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
      {rec.status === "error" && (
        <div className={styles.explain_error}>{rec.error || t("detail.explain_failed", "失败")}</div>
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
    </div>
  );
}
