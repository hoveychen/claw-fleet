import {
  ChevronRight,
  Crosshair,
  Languages,
  LoaderCircle,
  MessageCircleQuestion,
  PencilLine,
  Scale,
} from "lucide-react";
import { useState } from "react";

import { cacheHitRatio, costLabel, quoteSnippet } from "../../../shared-ts/sessionExplain";
import { dateLocale, t } from "../i18n";
import type { ExplainPreset, ExplainRecord } from "../sessionExplain";
import { Md } from "./DecisionQa";
import { EmptyState } from "./EmptyState";
import tabStyles from "./SessionDetailTabs.module.css";
import styles from "./SessionExplainsTab.module.css";

const PRESET_ICON: Record<ExplainPreset, typeof MessageCircleQuestion> = {
  explain: MessageCircleQuestion,
  translate: Languages,
  rationale: Scale,
  custom: PencilLine,
};

const PRESET_LABEL: Record<ExplainPreset, string> = {
  explain: "解释",
  translate: "翻译",
  rationale: "为什么",
  custom: "自定义提问",
};

/**
 * The 「追问」 pane: every side question asked about this session, newest
 * first, the one just asked open.
 *
 * Each card is one exchange. Collapsed it is the question line with the
 * passage's first line under it and the state on the right (spinning while
 * the fork runs, its cost once it has, a failure mark otherwise). Open it is
 * the whole thing: the quoted passage (tap to jump back to it in the
 * transcript), the answer growing as the record is re-read, the spend line —
 * model, cache share, cost, time — and a box to ask a follow-up, which forks
 * the *session* again with this Q/A folded into the prompt.
 *
 * The answer renders through the same markdown chain as decision bodies, so a
 * table or code span in an explanation reads as it does elsewhere on the phone.
 */
export function SessionExplainsTab({
  explains,
  loaded,
  openId,
  busy,
  onToggle,
  onLocate,
  onFollowUp,
}: {
  explains: ExplainRecord[];
  /** The list has been read at least once; before that an empty list is silence, not 无. */
  loaded: boolean;
  openId: string | null;
  /** A question is being submitted; follow-up boxes wait. */
  busy: boolean;
  onToggle: (id: string | null) => void;
  /** Leave the pane and scroll the transcript to the quoted passage. */
  onLocate: (rec: ExplainRecord) => void;
  onFollowUp: (prev: ExplainRecord, question: string) => void;
}) {
  if (!loaded && explains.length === 0) return <div className={tabStyles.hint}>{t("加载追问…")}</div>;
  if (explains.length === 0) {
    return (
      <EmptyState
        compact
        icon={MessageCircleQuestion}
        title={t("该会话还没有追问")}
        description={t("长按选中回复里的一段文字，就能对它追问")}
      />
    );
  }
  const ordered = [...explains].sort((a, b) => b.createdMs - a.createdMs || b.id.localeCompare(a.id));
  return (
    <div className={tabStyles.stack}>
      {ordered.map((rec) => (
        <ExplainCard
          key={rec.id}
          rec={rec}
          open={openId === rec.id}
          busy={busy}
          onToggle={() => onToggle(openId === rec.id ? null : rec.id)}
          onLocate={() => onLocate(rec)}
          onFollowUp={(q) => onFollowUp(rec, q)}
        />
      ))}
    </div>
  );
}

function ExplainCard({
  rec,
  open,
  busy,
  onToggle,
  onLocate,
  onFollowUp,
}: {
  rec: ExplainRecord;
  open: boolean;
  busy: boolean;
  onToggle: () => void;
  onLocate: () => void;
  onFollowUp: (question: string) => void;
}) {
  const [followUp, setFollowUp] = useState("");
  const Icon = PRESET_ICON[rec.preset] ?? MessageCircleQuestion;
  const running = rec.status === "running";
  const failed = rec.status === "error";
  const hit = cacheHitRatio(rec);
  const cost = costLabel(rec.costUsd);
  const question = rec.question || t(PRESET_LABEL[rec.preset] ?? "解释");
  const when = new Date(rec.createdMs).toLocaleString(dateLocale(), {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });

  const status = running ? (
    <span className={styles.state} data-tone="live">
      <LoaderCircle size={11} className={styles.spin} aria-hidden="true" />
      {t("追问中")}
    </span>
  ) : failed ? (
    <span className={styles.state} data-tone="bad">
      {t("失败")}
    </span>
  ) : cost ? (
    <span className={styles.state}>{cost}</span>
  ) : null;

  return (
    <div className={tabStyles.planCard} data-testid="explain-card" data-open={open || undefined}>
      <div
        className={styles.head}
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <ChevronRight
          size={14}
          className={tabStyles.hopChevron}
          style={{ transform: open ? "rotate(90deg)" : "none" }}
        />
        <Icon size={14} strokeWidth={1.8} className={styles.presetIcon} aria-hidden="true" />
        <span className={styles.question}>{question}</span>
        {status}
      </div>

      {open ? (
        <>
          {/* The passage, as a link back to where it came from. */}
          <button type="button" className={styles.quote} onClick={onLocate}>
            <span className={styles.quoteText}>{rec.quote}</span>
            <span className={styles.locate}>
              <Crosshair size={12} strokeWidth={1.8} aria-hidden="true" />
              {t("定位原文")}
            </span>
          </button>
          {rec.text ? (
            <div className={`${tabStyles.markdown} ${styles.answer}`} data-partial={running || undefined}>
              <Md text={rec.text} />
            </div>
          ) : running ? (
            <div className={styles.waiting}>
              <LoaderCircle size={13} className={styles.spin} aria-hidden="true" />
              {t("正在 fork 会话作答…")}
            </div>
          ) : null}
          {failed && <div className={styles.error}>{rec.error || t("失败")}</div>}
          <div className={styles.meta}>
            <span>{when}</span>
            {!running && rec.model && <span>{rec.model}</span>}
            {!running && hit != null && <span>{t("缓存 {0}%", Math.round(hit * 100))}</span>}
            {!running && cost && <span>{cost}</span>}
            {!running && rec.durationMs > 0 && <span>{(rec.durationMs / 1000).toFixed(1)}s</span>}
          </div>
          {rec.status === "done" && (
            <form
              className={styles.follow}
              onSubmit={(e) => {
                e.preventDefault();
                const q = followUp.trim();
                if (!q || busy) return;
                onFollowUp(q);
                setFollowUp("");
              }}
            >
              <input
                className={styles.followInput}
                value={followUp}
                onChange={(e) => setFollowUp(e.target.value)}
                placeholder={t("继续追问这段话…")}
                aria-label={t("继续追问")}
                enterKeyHint="send"
              />
              <button type="submit" className={styles.followSend} disabled={busy || !followUp.trim()}>
                {t("发送")}
              </button>
            </form>
          )}
        </>
      ) : (
        <div className={styles.preview}>
          <span className={styles.previewQuote}>{quoteSnippet(rec.quote, 60)}</span>
          {rec.text && <span className={styles.previewAnswer}>{rec.text}</span>}
          {failed && <span className={styles.error}>{rec.error || t("失败")}</span>}
        </div>
      )}
    </div>
  );
}
