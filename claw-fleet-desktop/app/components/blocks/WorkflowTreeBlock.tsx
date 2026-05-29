import { useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  ToolResultBlock,
  ToolUseBlock as ToolUseBlockType,
  WorkflowTree,
} from "../../types";
import styles from "./WorkflowTreeBlock.module.css";

interface Props {
  block: ToolUseBlockType;
  result?: ToolResultBlock;
  /** Live tree discovered on disk (journal + phases), matched by runId. */
  tree?: WorkflowTree;
}

/** Pull the `wf_<id>` run id out of the tool_result's first lines. */
export function extractRunId(result?: ToolResultBlock): string | null {
  if (!result) return null;
  const text =
    typeof result.content === "string"
      ? result.content
      : JSON.stringify(result.content);
  // Transcript dir / scriptPath / resumeFromRunId all contain `wf_<id>`.
  const m = text.match(/wf_[A-Za-z0-9_-]+/);
  return m ? m[0] : null;
}

/** Pull the human Summary line out of the tool_result. */
function extractSummary(result?: ToolResultBlock): string | null {
  if (!result) return null;
  const text =
    typeof result.content === "string"
      ? result.content
      : JSON.stringify(result.content);
  const m = text.match(/Summary:\s*(.+)/);
  return m ? m[1].trim() : null;
}

export function WorkflowTreeBlock({ block, result, tree }: Props) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);

  const runId = tree?.runId ?? extractRunId(result);
  const name =
    tree?.name ??
    (typeof block.input?.name === "string" ? block.input.name : undefined);
  const summary = tree?.description ?? extractSummary(result);
  const phases = tree?.phases ?? [];
  const agents = tree?.agents ?? [];

  const doneCount = agents.filter((a) => a.status === "done").length;
  const running = agents.some((a) => a.status === "running");
  const total = agents.length;

  return (
    <div className={styles.card}>
      <button
        type="button"
        className={styles.header}
        onClick={() => setExpanded((e) => !e)}
      >
        <span className={styles.chevron} data-open={expanded}>
          ▸
        </span>
        <span className={styles.icon}>⚙</span>
        <span className={styles.title}>
          {name ?? t("workflow.title", "Workflow")}
        </span>
        {runId && <span className={styles.runId}>{runId}</span>}
        <span className={styles.spacer} />
        {total > 0 && (
          <span
            className={styles.statusPill}
            data-running={running ? "true" : "false"}
          >
            {running
              ? t("workflow.running", "{{done}}/{{total}} running", {
                  done: doneCount,
                  total,
                })
              : t("workflow.done", "{{total}} done", { total })}
          </span>
        )}
      </button>

      {expanded && (
        <div className={styles.body}>
          {summary && <div className={styles.summary}>{summary}</div>}

          {phases.length > 0 && (
            <div className={styles.phases}>
              <div className={styles.sectionLabel}>
                {t("workflow.phases", "Phases")}
              </div>
              <ol className={styles.phaseList}>
                {phases.map((p, i) => (
                  <li key={i} className={styles.phase}>
                    <span className={styles.phaseTitle}>{p.title}</span>
                    {p.detail && (
                      <span className={styles.phaseDetail}>{p.detail}</span>
                    )}
                  </li>
                ))}
              </ol>
            </div>
          )}

          <div className={styles.agents}>
            <div className={styles.sectionLabel}>
              {t("workflow.agents", "Agents")}
              {total > 0 ? ` (${total})` : ""}
            </div>
            {total === 0 ? (
              <div className={styles.empty}>
                {t("workflow.noAgents", "No agent activity recorded yet.")}
              </div>
            ) : (
              <ul className={styles.agentList}>
                {agents.map((a) => (
                  <li key={a.key} className={styles.agent}>
                    <span
                      className={styles.dot}
                      data-status={a.status}
                      title={a.status}
                    />
                    <span className={styles.agentId}>{a.agentId}</span>
                    <span
                      className={styles.agentStatus}
                      data-status={a.status}
                    >
                      {a.status === "done"
                        ? t("workflow.agentDone", "done")
                        : t("workflow.agentRunning", "running")}
                    </span>
                    {a.result && (
                      <span className={styles.agentResultLen}>
                        {t("workflow.resultChars", "{{n}} chars", {
                          n: a.result.length,
                        })}
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
