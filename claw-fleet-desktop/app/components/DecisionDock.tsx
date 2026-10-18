// The decision card pinned to the bottom of a task's conversation. Every card
// the task is waiting on — its main conversation's and its sub-tasks' — lands
// here one at a time, so answering never means scrolling the transcript to
// find the question. Digits pick options, ⌘↵ sends, Esc denies a guard.

import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDecisionStore, useUIStore } from "../store";
import type { PendingDecision } from "../types";
import { SkeletonCard } from "./loading";
import { dockAction, fromSubtask } from "./dockKeys";
import styles from "./DecisionDock.module.css";

// DecisionPanel embeds SessionDetail, which embeds this dock; keep the edge
// back to DecisionPanel lazy so the modules do not form an eager ESM cycle.
const DecisionCard = lazy(() =>
  import("./DecisionPanel").then(({ DecisionCard }) => ({ default: DecisionCard })),
);

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    target.isContentEditable
  );
}

export function DecisionDock({
  cards,
  projectName = null,
  taskTitle = null,
  keyboard = true,
}: {
  cards: PendingDecision[];
  projectName?: string | null;
  taskTitle?: string | null;
  /** Off for a pane that is mounted but hidden, so only the visible dock
   *  answers the keyboard. */
  keyboard?: boolean;
}) {
  const { t } = useTranslation();
  const [activeId, setActiveId] = useState<string | null>(null);
  const index = Math.max(0, cards.findIndex((c) => c.id === activeId));
  const active = cards[index] ?? null;

  const toggleElicitationOption = useDecisionStore((s) => s.toggleElicitationOption);
  const toggleFleetAskOption = useDecisionStore((s) => s.toggleFleetAskOption);
  const setElicitationStep = useDecisionStore((s) => s.setElicitationStep);
  const setFleetAskStep = useDecisionStore((s) => s.setFleetAskStep);
  const submitElicitation = useDecisionStore((s) => s.submitElicitation);
  const submitFleetAsk = useDecisionStore((s) => s.submitFleetAsk);
  const respond = useDecisionStore((s) => s.respond);
  const respondToPermissionPrompt = useDecisionStore((s) => s.respondToPermissionPrompt);

  useEffect(() => {
    if (!keyboard || !active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      // Settings sits on top and owns its own keys (Esc closes it).
      if (useUIStore.getState().settingsOpen) return;
      const action = dockAction(active, {
        key: e.key,
        metaKey: e.metaKey,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        inEditable: isEditable(e.target),
      });
      if (!action) return;
      e.preventDefault();
      switch (action.kind) {
        case "pick":
          if (active.kind === "elicitation") {
            toggleElicitationOption(active.id, action.question, action.label, action.multi);
          } else if (active.kind === "fleet-ask") {
            toggleFleetAskOption(active.id, action.question, action.label, action.multi);
          }
          break;
        case "next":
          if (active.kind === "elicitation") setElicitationStep(active.id, active.step + 1);
          else if (active.kind === "fleet-ask") setFleetAskStep(active.id, active.step + 1);
          break;
        case "submit":
          if (active.kind === "elicitation") void submitElicitation(active.id);
          else if (active.kind === "fleet-ask") void submitFleetAsk(active.id);
          break;
        case "deny":
          if (active.kind === "guard") void respond(active.id, false);
          else if (active.kind === "permission-prompt") void respondToPermissionPrompt(active.id, false);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    keyboard,
    active,
    toggleElicitationOption,
    toggleFleetAskOption,
    setElicitationStep,
    setFleetAskStep,
    submitElicitation,
    submitFleetAsk,
    respond,
    respondToPermissionPrompt,
  ]);

  const subtask = active && fromSubtask(active.request) ? active.request.task : null;
  const crumbs = useMemo(() => {
    if (!subtask) return [];
    return [
      projectName ?? active?.request.workspaceName ?? null,
      taskTitle ?? subtask.taskTitle ?? null,
      t("home.dock_from_subtask", "From a sub-task"),
    ].filter((c): c is string => !!c);
  }, [subtask, projectName, taskTitle, active?.request.workspaceName, t]);

  if (!active) return null;
  const questionCard = active.kind === "elicitation" || active.kind === "fleet-ask";

  return (
    <section className={styles.dock} data-testid="decision-dock" aria-label={t("home.dock_title", "Needs your decision")}>
      <header className={styles.head}>
        <span className={styles.dot} aria-hidden />
        <span className={styles.title}>{t("home.dock_title", "Needs your decision")}</span>
        {subtask && (
          <span className={styles.source}>
            {t("home.dock_subtask_source", {
              name: subtask.subtaskTitle || t("home.dock_subtask_untitled", "Sub-task"),
              defaultValue: "· from sub-task “{{name}}”",
            })}
          </span>
        )}
        <span className={styles.spacer} />
        {cards.length > 1 && (
          <span className={styles.pager}>
            <button
              type="button"
              className={styles.pager_btn}
              disabled={index === 0}
              onClick={() => setActiveId(cards[index - 1]!.id)}
              aria-label={t("home.dock_prev", "Previous card")}
            >
              ‹
            </button>
            <span className={styles.pager_count}>
              {index + 1} / {cards.length}
            </span>
            <button
              type="button"
              className={styles.pager_btn}
              disabled={index === cards.length - 1}
              onClick={() => setActiveId(cards[index + 1]!.id)}
              aria-label={t("home.dock_next", "Next card")}
            >
              ›
            </button>
          </span>
        )}
      </header>
      {crumbs.length > 0 && (
        <div className={styles.crumbs}>{crumbs.join(" › ")}</div>
      )}
      <div className={`${styles.body} ${questionCard ? styles.body_flex : ""}`}>
        <Suspense fallback={<SkeletonCard height={140} />}>
          <DecisionCard key={active.id} decision={active} compact />
        </Suspense>
      </div>
    </section>
  );
}
