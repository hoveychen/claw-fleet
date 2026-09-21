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

import { cacheHitRatio, costLabel, groupExplainThreads, quoteSnippet } from "../../../shared-ts/sessionExplain";
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
 * chain first, the one just asked open.
 *
 * Each card is one *chain* — a first question plus the follow-ups continuing
 * it. A follow-up is its own record on the host side (the fork is never
 * resumed; the prior Q/A is folded into a new one), so one card per record
 * showed a two-turn conversation as two unrelated cards. Collapsed the card is
 * the opening question with the passage's first line and the chain's latest
 * state on the right (spinning while a fork runs, its cost once it has, a
 * failure mark otherwise). Open it is the whole thing: the quoted passage (tap
 * to jump back to it in the transcript), every turn in asking order with its
 * own spend line — model, cache share, cost, time — and one box at the foot to
 * ask a follow-up, which forks the *session* again with the chain folded into
 * the prompt.
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
  /** Any record id in the open chain — the id a fresh ask reports is enough. */
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
  // Chains newest-first, turns inside one oldest-first. A chain counts as open
  // when *any* of its records is the open id: a fresh follow-up reports its own
  // new id, and the chain it continues has to stay open under it.
  const threads = groupExplainThreads(explains);
  return (
    <div className={tabStyles.stack}>
      {threads.map((thread) => {
        const open = thread.records.some((r) => r.id === openId);
        return (
          <ExplainCard
            key={thread.id}
            records={thread.records}
            open={open}
            busy={busy}
            onToggle={() => onToggle(open ? null : thread.id)}
            onLocate={() => onLocate(thread.records[0])}
            onFollowUp={(q) => onFollowUp(thread.records[thread.records.length - 1], q)}
          />
        );
      })}
    </div>
  );
}

function ExplainCard({
  records,
  open,
  busy,
  onToggle,
  onLocate,
  onFollowUp,
}: {
  /** One chain, oldest turn first. */
  records: ExplainRecord[];
  open: boolean;
  busy: boolean;
  onToggle: () => void;
  onLocate: () => void;
  onFollowUp: (question: string) => void;
}) {
  const [followUp, setFollowUp] = useState("");
  // The head summarises the chain: its opening question, but the *latest*
  // turn's state — a chain whose follow-up is still forking reads as running.
  const first = records[0];
  const last = records[records.length - 1];
  const Icon = PRESET_ICON[first.preset] ?? MessageCircleQuestion;
  const running = last.status === "running";
  const failed = last.status === "error";
  const cost = costLabel(last.costUsd);
  const question = first.question || t(PRESET_LABEL[first.preset] ?? "解释");

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
        {records.length > 1 && <span className={styles.turns}>{t("{0} 轮", records.length)}</span>}
        {status}
      </div>

      {open ? (
        <>
          {/* The passage, as a link back to where it came from. Once per chain:
              every turn quotes the same one. */}
          <button type="button" className={styles.quote} onClick={onLocate}>
            <span className={styles.quoteText}>{first.quote}</span>
            <span className={styles.locate}>
              <Crosshair size={12} strokeWidth={1.8} aria-hidden="true" />
              {t("定位原文")}
            </span>
          </button>
          {records.map((rec) => (
            <ExplainTurn key={rec.id} rec={rec} withQuestion={records.length > 1} />
          ))}
          {last.status === "done" && (
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
          <span className={styles.previewQuote}>{quoteSnippet(first.quote, 60)}</span>
          {/* The latest answer, not the first: it is what the chain now says. */}
          {last.text && <span className={styles.previewAnswer}>{last.text}</span>}
          {failed && <span className={styles.error}>{last.error || t("失败")}</span>}
        </div>
      )}
    </div>
  );
}

/** One turn of a chain: its question (when the chain has more than one, where
 *  the head's single question line is no longer enough), the answer as it
 *  arrives, and that turn's own spend line. */
function ExplainTurn({ rec, withQuestion }: { rec: ExplainRecord; withQuestion: boolean }) {
  const running = rec.status === "running";
  const hit = cacheHitRatio(rec);
  const cost = costLabel(rec.costUsd);
  const when = new Date(rec.createdMs).toLocaleString(dateLocale(), {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
  return (
    <div className={styles.turn}>
      {withQuestion && rec.question && <div className={styles.turnQuestion}>{rec.question}</div>}
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
      {rec.status === "error" && <div className={styles.error}>{rec.error || t("失败")}</div>}
      <div className={styles.meta}>
        <span>{when}</span>
        {!running && rec.model && <span>{rec.model}</span>}
        {!running && hit != null && <span>{t("缓存 {0}%", Math.round(hit * 100))}</span>}
        {!running && cost && <span>{cost}</span>}
        {!running && rec.durationMs > 0 && <span>{(rec.durationMs / 1000).toFixed(1)}s</span>}
      </div>
    </div>
  );
}
