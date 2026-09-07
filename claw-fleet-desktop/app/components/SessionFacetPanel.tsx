import { useTranslation } from "react-i18next";
import type {
  BackgroundTask,
  DecisionHistoryRecord,
  SessionInfo,
  TaskPlanDetail,
  WorkflowTree,
} from "../types";
import type { AuxFacet } from "../detailAux";
import { bgTaskDataType, bgTaskIcon } from "../bgTaskKinds";
import { tokenPanelForAgentSource } from "../modelChoices";
import { CodexTokenPanel } from "./CodexTokenPanel";
import { DecisionHistory } from "./DecisionHistory";
import { DshTokenPanel } from "./DshTokenPanel";
import { ScratchpadView } from "./ScratchpadView";
import { SkillHistory } from "./SkillHistory";
import { TokenSpendPanel } from "./TokenSpendPanel";
import { WorkflowDag } from "./blocks/WorkflowDag";
import styles from "./SessionDetail.module.css";

/**
 * One facet of a session, rendered inside the detail's auxiliary column.
 *
 * These panels used to be seven mutually-exclusive branches inline in
 * `SessionDetail`, each swapping out the whole content area — reading a token
 * receipt meant losing sight of the conversation. They are the same panels; the
 * only change is that they now render *beside* the transcript, which is why
 * they moved out into a component that takes what it shows as props.
 */
export function SessionFacetPanel({
  facet,
  session,
  decisionRecords,
  taskPlans,
  bgTasks,
  workflowTrees,
  sessions,
  onOpenAgent,
}: {
  facet: AuxFacet;
  session: SessionInfo;
  decisionRecords: DecisionHistoryRecord[];
  taskPlans: TaskPlanDetail[];
  bgTasks: BackgroundTask[];
  workflowTrees: WorkflowTree[];
  /** The live session list — a background task of kind `subagent` correlates to
   *  its own scanned `agent-<id>` session, whose *current* status beats the
   *  last-Stop snapshot the task record carries. */
  sessions: SessionInfo[];
  onOpenAgent: (agentId: string) => void;
}) {
  const { t } = useTranslation();

  switch (facet) {
    case "scratchpad":
      return <ScratchpadView workspace={session.workspacePath} sessionId={session.id} />;

    case "decisions":
      return <DecisionHistory records={decisionRecords} mode="tab" />;

    case "skills":
      return (
        <div className={styles.skills_panel}>
          <SkillHistory jsonlPath={session.jsonlPath} mode="tab" />
        </div>
      );

    case "tokens":
      // `jsonlPath` carries each source's own handle: a file path for Claude, a
      // `codex://` rollout URI, a `dsh://` session id.
      return {
        codex: <CodexTokenPanel jsonlPath={session.jsonlPath} />,
        dsh: <DshTokenPanel uri={session.jsonlPath} />,
        claude: (
          <TokenSpendPanel
            jsonlPath={session.jsonlPath}
            workspacePath={session.workspacePath}
          />
        ),
      }[tokenPanelForAgentSource(session.agentSource)];

    case "tasks":
      return (
        <div className={styles.tasks_panel}>
          {taskPlans.map((plan, pi) => {
            const done = plan.items.filter((it) => it.done).length;
            const total = plan.items.length;
            const allDone = total > 0 && done === total;
            // Prefer the human-readable `**Plan:**` title; fall back to the
            // sentinel id, then to the anonymous label.
            const title = plan.title ?? plan.id ?? t("detail.tasks_anonymous");
            // Keep the id as a secondary tag only when a title is present —
            // otherwise the title already *is* the id, no need to repeat it.
            const showId = Boolean(plan.title && plan.id);
            // The first still-pending item is "current" for this plan — the
            // visible answer to "做到第几个 P 了".
            const currentIdx = plan.items.findIndex((it) => !it.done);
            return (
              <div key={plan.id ?? `plan-${pi}`} className={styles.tasks_plan}>
                <div className={styles.tasks_plan_head}>
                  <span className={styles.tasks_plan_title}>{title}</span>
                  <span
                    className={`${styles.tasks_plan_status} ${allDone ? styles.tasks_plan_status_done : styles.tasks_plan_status_active}`}
                  >
                    {allDone
                      ? t("detail.tasks_status_done")
                      : t("detail.tasks_status_active")}
                  </span>
                  <span className={styles.tasks_plan_count}>
                    {done}/{total}
                  </span>
                </div>
                {(showId || plan.source) && (
                  <div className={styles.tasks_plan_sub}>
                    {showId && <span className={styles.tasks_plan_id}>{plan.id}</span>}
                    {plan.source && (
                      <span className={styles.tasks_plan_source} title={plan.source}>
                        {plan.source}
                      </span>
                    )}
                  </div>
                )}
                <ul className={styles.tasks_items}>
                  {plan.items.map((it, ii) => {
                    const isCurrent = ii === currentIdx;
                    return (
                      <li
                        key={ii}
                        className={`${styles.tasks_item} ${it.done ? styles.tasks_item_done : ""} ${isCurrent ? styles.tasks_item_current : ""}`}
                      >
                        <span className={styles.tasks_check} aria-hidden>
                          {it.done ? "☑" : isCurrent ? "▶" : "☐"}
                        </span>
                        <span className={styles.tasks_text}>{it.text}</span>
                      </li>
                    );
                  })}
                </ul>
              </div>
            );
          })}
        </div>
      );

    case "bgtasks":
      return (
        <div className={styles.bgtasks_panel}>
          {bgTasks.map((bt) => {
            const icon = bgTaskIcon(bt.type);
            const dataType = bgTaskDataType(bt.type);
            const label = bt.description || bt.command || bt.id;
            // A subagent task correlates to its own scanned session
            // (`agent-<id>`, see session.rs / openAgentSession). When that
            // session is present we read its *live* status and let the row open
            // it — far more accurate than the last-Stop snapshot, which for a
            // subagent can be minutes stale.
            const linked =
              bt.type === "subagent"
                ? sessions.find((s) => s.id === `agent-${bt.id}`)
                : undefined;
            if (linked) {
              return (
                <button
                  key={bt.id}
                  className={styles.bgtask_item_link}
                  onClick={() => onOpenAgent(bt.id)}
                  title={t("detail.bgtask_open_hint")}
                >
                  <span className={styles.bgtask_icon} aria-hidden>{icon}</span>
                  <span className={styles.tab_dot} data-status={linked.status} />
                  <span className={styles.bgtask_type} data-bgtype={dataType}>
                    {linked.agentType ?? bt.type}
                  </span>
                  <span className={styles.bgtask_desc}>{linked.aiTitle || label}</span>
                </button>
              );
            }
            // Shell / monitor, or a subagent whose session hasn't surfaced (or
            // already aged out): no live source, so this row reflects the *last
            // Stop* only — flag it as such rather than imply it's current.
            return (
              <div key={bt.id} className={styles.bgtask_item}>
                <span className={styles.bgtask_icon} aria-hidden>{icon}</span>
                <span className={styles.bgtask_type} data-bgtype={dataType}>
                  {bt.type}
                </span>
                <span className={styles.bgtask_desc}>{label}</span>
                <span
                  className={styles.bgtask_stale}
                  title={
                    session.lastActivityMs
                      ? new Date(session.lastActivityMs).toLocaleString()
                      : undefined
                  }
                >
                  {t("detail.bgtask_as_of_stop")}
                </span>
              </div>
            );
          })}
          <div className={styles.bgtasks_note}>{t("detail.bgtasks_note")}</div>
        </div>
      );

    case "workflow":
      return (
        <div className={styles.workflow_panel}>
          {workflowTrees.map((tree) => {
            const done = tree.agents.filter((a) => a.status === "done").length;
            return (
              <div key={tree.runId} className={styles.workflow_run}>
                <div className={styles.workflow_run_head}>
                  <span className={styles.workflow_run_name}>{tree.name ?? tree.runId}</span>
                  <span className={styles.workflow_run_meta}>
                    {tree.runId} · {done}/{tree.agents.length} agents
                  </span>
                </div>
                <WorkflowDag tree={tree} onOpenAgent={onOpenAgent} />
              </div>
            );
          })}
        </div>
      );
  }
}
