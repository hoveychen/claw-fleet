// Side questions asked from inside a decision card's question, phone side — the
// counterpart of claw-fleet-desktop/app/components/DecisionExplainMarks.tsx.
//
// The card's question body is stamped `data-role="assistant"` / `data-msg-idx`
// (the question index) so the same `SelectionAskBar` that floats over
// transcript prose floats over it: a long-press, or a tap on one of the agent's
// `[?text]` marks, selects a passage and the bar offers the presets. The ask
// goes to the card's session with an empty anchor (a card is not a transcript
// row) and the answer lands under the question, inside the card, where the
// person deciding can read it without leaving. The record is persisted like
// every side question, so the session's 追问 pane shows it later too.
import { LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  cacheHitRatio,
  costLabel,
  groupExplainThreads,
  pollExplanation,
  type AssistantSelection,
} from "../../../shared-ts/sessionExplain";
import { t } from "../i18n";
import {
  askExplanation,
  getExplanation,
  refusedExplanation,
  type ExplainPreset,
  type ExplainRecord,
  type ExplainRequest,
} from "../sessionExplain";
import type { FleetTransport } from "../transport";
import type { SessionInfo } from "../types";
import { Md } from "./DecisionQa";
import styles from "./DecisionExplainMarks.module.css";

export function useDecisionExplainMarks(
  client: FleetTransport | null,
  session: SessionInfo | undefined,
): {
  enabled: boolean;
  busy: boolean;
  ask: (sel: AssistantSelection, preset: ExplainPreset, question?: string) => void;
  /**
   * Continue a settled answer: same passage, the prior Q/A folded into a fresh
   * fork of the *session* (the fork itself is never resumed). Threading it off
   * `prev` is what keeps the follow-up aware of the answer it continues.
   */
  followUp: (prev: ExplainRecord, question: string) => void;
  answers: ExplainRecord[];
  dismiss: (id: string) => void;
} {
  const sessionId = session?.id;
  const sessionPath = session?.jsonlPath;
  const workspacePath = session?.workspacePath;
  const [answers, setAnswers] = useState<ExplainRecord[]>([]);
  const [busy, setBusy] = useState(false);
  const pollers = useRef(new Map<string, AbortController>());

  const upsert = useCallback((rec: ExplainRecord) => {
    setAnswers((prev) => {
      const i = prev.findIndex((r) => r.id === rec.id);
      if (i < 0) return [...prev, rec];
      const next = prev.slice();
      next[i] = rec;
      return next;
    });
  }, []);

  useEffect(() => {
    return () => {
      for (const ctl of pollers.current.values()) ctl.abort();
      pollers.current.clear();
      setAnswers([]);
    };
  }, [sessionId]);

  /** Post a request, show the record (or a refusal stand-in), and follow it
   *  until it settles. Shared by the first question and every follow-up. */
  const submit = useCallback(
    async (req: ExplainRequest) => {
      if (!client) return;
      setBusy(true);
      let rec: ExplainRecord;
      try {
        rec = await askExplanation(client, req);
      } catch (e) {
        rec = refusedExplanation(req, e);
      } finally {
        setBusy(false);
      }
      upsert(rec);
      if (rec.status === "running" && !pollers.current.has(rec.id)) {
        const ctl = new AbortController();
        pollers.current.set(rec.id, ctl);
        pollExplanation(() => getExplanation(client, req.sessionId, rec.id), upsert, { signal: ctl.signal })
          .catch((e) => console.error("explain poll failed:", e))
          .finally(() => {
            if (pollers.current.get(rec.id) === ctl) pollers.current.delete(rec.id);
          });
      }
    },
    [client, upsert],
  );

  const ask = useCallback(
    async (sel: AssistantSelection, preset: ExplainPreset, question?: string) => {
      if (!client || !sessionId || !sessionPath) return;
      await submit({
        sessionId,
        sessionPath,
        workspacePath: workspacePath || undefined,
        quote: sel.quote,
        preset,
        question: preset === "custom" ? question : undefined,
        anchor: undefined,
        thread: [],
      });
      window.getSelection()?.removeAllRanges();
    },
    [client, sessionId, sessionPath, workspacePath, submit],
  );

  const followUp = useCallback(
    async (prev: ExplainRecord, question: string) => {
      if (!client || !sessionId || !sessionPath) return;
      await submit({
        sessionId,
        sessionPath,
        workspacePath: workspacePath || undefined,
        quote: prev.quote,
        preset: "custom",
        question,
        anchor: prev.anchor,
        thread: [...(prev.thread ?? []), prev.id],
      });
    },
    [client, sessionId, sessionPath, workspacePath, submit],
  );

  const dismiss = useCallback((id: string) => {
    pollers.current.get(id)?.abort();
    pollers.current.delete(id);
    setAnswers((prev) => prev.filter((r) => r.id !== id));
  }, []);

  return { enabled: Boolean(client && sessionId && sessionPath), busy, ask, followUp, answers, dismiss };
}

/**
 * The answers asked from inside a card's question, under it — one card per
 * chain, turns oldest-first, with a follow-up box at the foot of each.
 *
 * Grouping is not cosmetic: a follow-up is its own record (the fork is never
 * resumed, the prior Q/A is folded into a new one), so a flat list read a
 * two-turn conversation as two unrelated answers.
 */
export function DecisionExplainAnswers({
  answers,
  busy,
  onDismiss,
  onFollowUp,
}: {
  answers: ExplainRecord[];
  busy: boolean;
  onDismiss: (id: string) => void;
  onFollowUp: (prev: ExplainRecord, question: string) => void;
}) {
  if (answers.length === 0) return null;
  return (
    <div className={styles.list} data-testid="decision-explain-answers">
      {groupExplainThreads(answers).map((thread) => {
        const last = thread.records[thread.records.length - 1];
        const running = last.status === "running";
        const hit = cacheHitRatio(last);
        const cost = costLabel(last.costUsd);
        return (
          <div key={thread.id} className={styles.card}>
            <div className={styles.head}>
              <span className={styles.quote}>{thread.records[0].quote}</span>
              {running && (
                <span className={styles.state} data-tone="live">
                  <LoaderCircle size={11} aria-hidden="true" className={styles.spin} />
                  {t("追问中")}
                </span>
              )}
              {!running && cost && <span className={styles.state}>{cost}</span>}
              {!running && hit != null && <span className={styles.state}>{t("缓存 {0}%", Math.round(hit * 100))}</span>}
              <button
                type="button"
                className={styles.close}
                onClick={() => {
                  for (const rec of thread.records) onDismiss(rec.id);
                }}
                aria-label={t("关闭")}
              >
                ✕
              </button>
            </div>
            {thread.records.map((rec) => (
              <ExplainTurn key={rec.id} rec={rec} />
            ))}
            {last.status === "done" && (
              <ExplainFollowUp busy={busy} onSubmit={(question) => onFollowUp(last, question)} />
            )}
          </div>
        );
      })}
    </div>
  );
}

/** One turn of a chain: the question asked, then the answer as it arrives. */
function ExplainTurn({ rec }: { rec: ExplainRecord }) {
  const running = rec.status === "running";
  return (
    <div className={styles.turn}>
      {rec.question && <div className={styles.question}>{rec.question}</div>}
      {rec.text ? (
        <div className={styles.answer} data-partial={running || undefined}>
          <Md text={rec.text} />
        </div>
      ) : running ? (
        <div className={styles.waiting}>{t("正在 fork 会话作答…")}</div>
      ) : null}
      {rec.status === "error" && <div className={styles.error}>{rec.error || t("失败")}</div>}
    </div>
  );
}

/**
 * Keep asking about the same passage. Only rendered once the chain's last turn
 * is `done`: a follow-up folds the prior answers into its prompt, so asking
 * before one has settled would thread off a half-written answer.
 */
function ExplainFollowUp({ busy, onSubmit }: { busy: boolean; onSubmit: (question: string) => void }) {
  const [draft, setDraft] = useState("");
  return (
    <form
      className={styles.follow}
      onSubmit={(e) => {
        e.preventDefault();
        const q = draft.trim();
        if (!q || busy) return;
        onSubmit(q);
        setDraft("");
      }}
    >
      <input
        className={styles.followInput}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        placeholder={t("继续追问这段话…")}
        aria-label={t("继续追问")}
      />
      <button type="submit" className={styles.followSend} disabled={busy || !draft.trim()}>
        {t("发送")}
      </button>
    </form>
  );
}
