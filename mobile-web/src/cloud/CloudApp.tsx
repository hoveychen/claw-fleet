import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ArrowLeft,
  Bot,
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Cloud,
  Inbox,
  ListTodo,
  Loader2,
  RefreshCw,
} from "lucide-react";
import appStyles from "../App.module.css";
import { EmptyState } from "../views/EmptyState";
import { ConnIcon, type ConnIconKind } from "../views/ConnIcon";
import { HttpFleetCloudClient, type FleetCloudClient } from "./client";
import { applyTaskEvent, initialCloudTaskState, type CloudTaskState } from "./reducer";
import type { Decision, Task, TaskDetail, TaskStatus } from "./types";
import styles from "./CloudApp.module.css";

type CloudTab = "tasks" | "decisions";

type SyncState = "online" | "syncing" | "offline";

/** Cloud 的 header 用和主 app 同一枚连接图标。这里没有链路强度可测,只有三档
 *  同步状态,所以只借用图标的「满格 / 重连中 / 断开」三种形态。 */
function cloudConnKind(s: SyncState): ConnIconKind {
  return s === "online" ? "good" : s === "syncing" ? "connecting" : "offline";
}

function cloudConnText(s: SyncState): string {
  return s === "online" ? "Cloud 在线" : s === "syncing" ? "同步中" : "连接失败";
}

const STATUS_LABEL: Record<TaskStatus, string> = {
  queued: "排队中",
  assigned: "已分配",
  running: "运行中",
  waiting_for_input: "等待决策",
  paused: "已暂停",
  rate_limited: "速率受限",
  succeeded: "已完成",
  failed: "失败",
  cancelled: "已取消",
};

function defaultClient(): FleetCloudClient {
  return new HttpFleetCloudClient({
    baseUrl: import.meta.env.VITE_FLEET_CLOUD_API_URL || window.location.origin,
    organizationId: import.meta.env.VITE_FLEET_CLOUD_ORGANIZATION_ID || "11111111-1111-4111-8111-111111111111",
    projectId: import.meta.env.VITE_FLEET_CLOUD_PROJECT_ID || "22222222-2222-4222-8222-222222222222",
    accessToken: import.meta.env.VITE_FLEET_CLOUD_ACCESS_TOKEN || undefined,
    embedToken: import.meta.env.VITE_FLEET_CLOUD_EMBED_TOKEN || undefined,
  });
}

function taskTitle(task: Task): string {
  return task.title?.trim() || task.prompt.split("\n")[0]?.slice(0, 88) || "Untitled task";
}

function timeAgo(timestamp: string): string {
  const elapsed = Math.max(0, Date.now() - new Date(timestamp).getTime());
  if (elapsed < 60_000) return "刚刚";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)} 分钟前`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)} 小时前`;
  return `${Math.floor(elapsed / 86_400_000)} 天前`;
}

function decisionQuestion(decision: Decision): string {
  const value = decision.presentation.question ?? decision.presentation.title ?? decision.presentation.prompt;
  return typeof value === "string" ? value : "Agent 正在等待你的决定";
}

interface CloudAppProps {
  client?: FleetCloudClient;
}

export function CloudApp({ client: suppliedClient }: CloudAppProps) {
  const client = useMemo(() => suppliedClient ?? defaultClient(), [suppliedClient]);
  const embedTaskId = suppliedClient ? null : import.meta.env.VITE_FLEET_CLOUD_EMBED_TASK_ID || null;
  const [tab, setTab] = useState<CloudTab>("tasks");
  const [tasks, setTasks] = useState<Task[]>([]);
  const [details, setDetails] = useState<Record<string, TaskDetail>>({});
  const [selectedId, setSelectedId] = useState<string | null>(embedTaskId);
  const [detailState, setDetailState] = useState<CloudTaskState | null>(null);
  const [detailReload, setDetailReload] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [syncState, setSyncState] = useState<"online" | "syncing" | "offline">("syncing");

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    setSyncState("syncing");
    try {
      if (embedTaskId) {
        const detail = await client.getTask(embedTaskId);
        setTasks([detail]);
        setDetails({ [detail.id]: detail });
        setSyncState("online");
        return;
      }
      const page = await client.listTasks({ limit: 100 });
      setTasks(page.data);
      const loaded = await Promise.all(page.data.map((task) => client.getTask(task.id)));
      setDetails(Object.fromEntries(loaded.map((detail) => [detail.id, detail])));
      setSyncState("online");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      setSyncState("offline");
    } finally {
      setLoading(false);
    }
  }, [client, embedTaskId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!selectedId) {
      setDetailState(null);
      return;
    }
    const controller = new AbortController();
    let active = true;
    setSyncState("syncing");
    void client
      .getTask(selectedId)
      .then(async (detail) => {
        if (!active) return;
        const seed = initialCloudTaskState({ ...detail, event_cursor: 0 });
        setDetailState(seed);
        setSyncState("online");
        await client.streamTaskEvents(
          selectedId,
          0,
          (event) => {
            if (active) setDetailState((current) => (current ? applyTaskEvent(current, event) : current));
          },
          controller.signal,
        );
        if (active) setSyncState("online");
      })
      .catch((caught) => {
        if (!active || controller.signal.aborted) return;
        setError(caught instanceof Error ? caught.message : String(caught));
        setSyncState("offline");
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [client, selectedId, detailReload]);

  useEffect(() => {
    if (!detailState?.refetchAfterGap) return;
    setDetailState(null);
    setDetailReload((current) => current + 1);
  }, [detailState?.refetchAfterGap]);

  const openDecisions = useMemo(
    () =>
      Object.values(details)
        .flatMap((detail) => detail.decisions)
        .filter((decision) => decision.status === "open")
        .sort((left, right) => left.created_at.localeCompare(right.created_at)),
    [details],
  );

  const handleDecisionResolved = useCallback((resolved: Decision) => {
    setDetails((current) => {
      const detail = current[resolved.task_id];
      if (!detail) return current;
      return {
        ...current,
        [resolved.task_id]: {
          ...detail,
          waiting_decision_count: Math.max(0, detail.waiting_decision_count - 1),
          decisions: detail.decisions.map((decision) =>
            decision.id === resolved.id ? resolved : decision,
          ),
        },
      };
    });
    setDetailState((current) =>
      current
        ? {
            ...current,
            task: {
              ...current.task,
              waiting_decision_count: Math.max(0, current.task.waiting_decision_count - 1),
            },
            decisions: current.decisions.map((decision) =>
              decision.id === resolved.id ? resolved : decision,
            ),
          }
        : current,
    );
  }, []);

  if (selectedId) {
    return (
      <CloudDetail
        client={client}
        state={detailState}
        error={error}
        syncState={syncState}
        onBack={() => {
          if (embedTaskId) return;
          setSelectedId(null);
          setError(null);
        }}
        onDecisionResolved={handleDecisionResolved}
      />
    );
  }

  return (
    <div className={appStyles.app}>
      <header className={`${appStyles.header} ${styles.header}`}>
        <div className={styles.brandMark}>F</div>
        <div>
          <div className={appStyles.title}>Fleet Cloud</div>
          <div className={styles.projectLabel}>Hosted workspace</div>
        </div>
        <button className={styles.refresh} onClick={() => void refresh()} aria-label="刷新" disabled={loading}>
          <RefreshCw size={15} className={loading ? styles.spinning : undefined} />
        </button>
        <span className={appStyles.connIcon} data-kind={cloudConnKind(syncState)} role="img" aria-label={cloudConnText(syncState)} title={cloudConnText(syncState)}>
          <ConnIcon kind={cloudConnKind(syncState)} />
        </span>
      </header>

      <main className={`${appStyles.main} ${styles.main}`}>
        <div className={styles.rail}>
          <button data-active={tab === "tasks"} onClick={() => setTab("tasks")}>
            <ListTodo size={16} />任务 <span>{tasks.length}</span>
          </button>
          <button data-active={tab === "decisions"} onClick={() => setTab("decisions")}>
            <Inbox size={16} />决策 <span>{openDecisions.length}</span>
          </button>
        </div>

        <section className={styles.content}>
          <div className={styles.sectionHead}>
            <div>
              <div className={styles.eyebrow}>{tab === "tasks" ? "RUN QUEUE" : "HUMAN LOOP"}</div>
              <h1>{tab === "tasks" ? "云任务" : "待处理决策"}</h1>
            </div>
            <Cloud size={20} aria-hidden="true" />
          </div>

          {error && <ErrorBanner message={error} onRetry={() => void refresh()} />}
          {loading && tasks.length === 0 ? (
            <div className={styles.loading}><Loader2 size={18} className={styles.spinning} />正在读取控制面…</div>
          ) : tab === "tasks" ? (
            <TaskList tasks={tasks} onOpen={setSelectedId} />
          ) : (
            <DecisionList
              client={client}
              decisions={openDecisions}
              details={details}
              onOpenTask={setSelectedId}
              onResolved={handleDecisionResolved}
            />
          )}
        </section>
      </main>
    </div>
  );
}

function TaskList({ tasks, onOpen }: { tasks: Task[]; onOpen: (id: string) => void }) {
  if (tasks.length === 0) {
    return <EmptyState icon={ListTodo} title="还没有云任务" description="通过公开 Task API 创建的工作会出现在这里。" />;
  }
  return (
    <div className={styles.taskList}>
      {tasks.map((task) => (
        <button key={task.id} className={styles.taskRow} onClick={() => onOpen(task.id)}>
          <span className={styles.statusLine} data-status={task.status} />
          <span className={styles.taskCopy}>
            <span className={styles.taskTitle}>{taskTitle(task)}</span>
            <span className={styles.taskPrompt}>{task.prompt}</span>
          </span>
          <span className={styles.taskMeta}>
            <span className={styles.statusText} data-status={task.status}>{STATUS_LABEL[task.status]}</span>
            <span>{timeAgo(task.updated_at)}</span>
          </span>
          <ChevronRight size={16} className={styles.chevron} />
        </button>
      ))}
    </div>
  );
}

function DecisionList({ client, decisions, details, onOpenTask, onResolved }: { client: FleetCloudClient; decisions: Decision[]; details: Record<string, TaskDetail>; onOpenTask: (id: string) => void; onResolved: (decision: Decision) => void }) {
  if (decisions.length === 0) {
    return <EmptyState icon={CheckCircle2} title="没有待处理的决策" description="Agent 请求人工输入时，会连同所属 Attempt 出现在这里。" />;
  }
  return (
    <div className={styles.decisionList}>
      {decisions.map((decision) => (
        <div key={decision.id} className={styles.decisionCard}>
          <button className={styles.decisionRow} onClick={() => onOpenTask(decision.task_id)}>
            <CircleAlert size={17} />
            <span>
              <strong>{decisionQuestion(decision)}</strong>
              <small>{taskTitle(details[decision.task_id] ?? ({ title: null, prompt: "Cloud task" } as Task))} · {decision.kind.replaceAll("_", " ")}</small>
            </span>
            <ChevronRight size={16} />
          </button>
          <DecisionResponder client={client} decision={decision} onResolved={onResolved} />
        </div>
      ))}
    </div>
  );
}

function CloudDetail({ client, state, error, syncState, onBack, onDecisionResolved }: { client: FleetCloudClient; state: CloudTaskState | null; error: string | null; syncState: "online" | "syncing" | "offline"; onBack: () => void; onDecisionResolved: (decision: Decision) => void }) {
  const detail = state?.task;
  return (
    <div className={appStyles.app}>
      <header className={`${appStyles.header} ${styles.detailHeader}`}>
        <button className={styles.back} onClick={onBack}><ArrowLeft size={18} />返回</button>
        <div className={styles.detailHeaderTitle}>{detail ? taskTitle(detail) : "读取任务"}</div>
        <span className={appStyles.connIcon} data-kind={cloudConnKind(syncState)} role="img" aria-label={cloudConnText(syncState)} title={cloudConnText(syncState)}>
          <ConnIcon kind={cloudConnKind(syncState)} />
        </span>
      </header>
      <main className={`${appStyles.main} ${styles.detailMain}`}>
        {error && <ErrorBanner message={error} />}
        {!state || !detail ? (
          <div className={styles.loading}><Loader2 size={18} className={styles.spinning} />同步 Task event log…</div>
        ) : (
          <>
            <section className={styles.detailIntro}>
              <div className={styles.eyebrow}>TASK / {detail.id.slice(0, 8)}</div>
              <h1>{taskTitle(detail)}</h1>
              <p>{detail.prompt}</p>
              <div className={styles.detailFacts}>
                <span data-status={detail.status}>{STATUS_LABEL[detail.status]}</span>
                <span>{state.attempts.length} Attempts</span>
                <span>Event #{detail.event_cursor}</span>
              </div>
            </section>

            <div className={styles.detailGrid}>
              <section className={styles.transcript}>
                <h2>会话记录</h2>
                {state.messages.length === 0 ? (
                  <div className={styles.muted}>还没有 transcript.message 事件。</div>
                ) : state.messages.map((message) => (
                  <article key={message.id} data-role={message.role}>
                    <div><Bot size={14} />{message.role}<time>{new Date(message.occurredAt).toLocaleTimeString()}</time></div>
                    <p>{message.text}</p>
                  </article>
                ))}
              </section>

              <aside className={styles.attempts}>
                <h2>Attempt 链</h2>
                {state.attempts.length === 0 ? <div className={styles.muted}>等待 Runner 接单</div> : state.attempts.map((attempt) => (
                  <div key={attempt.id} className={styles.attemptRow}>
                    <span className={styles.attemptOrdinal}>{attempt.ordinal}</span>
                    <span><strong>{attempt.agent_source}</strong><small>{attempt.reason} · {attempt.status}</small></span>
                  </div>
                ))}
                {state.decisions.filter((decision) => decision.status === "open").map((decision) => (
                  <div key={decision.id} className={styles.openDecisionBlock}>
                    <div className={styles.openDecision}>
                      <CircleAlert size={15} /><span><strong>{decisionQuestion(decision)}</strong><small>{decision.kind.replaceAll("_", " ")}</small></span>
                    </div>
                    <DecisionResponder client={client} decision={decision} onResolved={onDecisionResolved} compact />
                  </div>
                ))}
              </aside>
            </div>
          </>
        )}
      </main>
    </div>
  );
}

function ErrorBanner({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return <div className={styles.errorBanner} role="alert"><CircleAlert size={16} /><span>{message}</span>{onRetry && <button onClick={onRetry}>重试</button>}</div>;
}

function DecisionResponder({ client, decision, onResolved, compact = false }: { client: FleetCloudClient; decision: Decision; onResolved: (decision: Decision) => void; compact?: boolean }) {
  const presentation = decision.presentation;
  const questions = Array.isArray(presentation.questions) ? presentation.questions : [];
  const firstQuestion = (questions[0] ?? presentation) as Record<string, unknown>;
  const rawOptions = Array.isArray(firstQuestion.options) ? firstQuestion.options : [];
  const options = rawOptions
    .map((option) => {
      if (typeof option === "string") return { label: option, value: option };
      if (!option || typeof option !== "object") return null;
      const record = option as Record<string, unknown>;
      const label = record.label ?? record.title ?? record.value;
      if (typeof label !== "string") return null;
      return { label, value: typeof record.value === "string" ? record.value : label };
    })
    .filter((option): option is { label: string; value: string } => option !== null);
  const questionId = typeof firstQuestion.id === "string" ? firstQuestion.id : "answer";
  const [answer, setAnswer] = useState(options[0]?.value ?? "");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const submit = async (action: "answer" | "decline") => {
    if (action === "answer" && !answer.trim()) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      const idempotencyKey = globalThis.crypto?.randomUUID?.() ?? `decision-${Date.now()}-${Math.random()}`;
      const resolved = await client.respondToDecision(
        decision.id,
        action === "answer" ? { action, answers: { [questionId]: answer } } : { action },
        idempotencyKey,
      );
      onResolved(resolved);
    } catch (caught) {
      setSubmitError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className={styles.responder} data-compact={compact}>
      {options.length > 0 ? (
        <div className={styles.optionRow}>
          {options.map((option) => (
            <button key={option.value} data-active={answer === option.value} onClick={() => setAnswer(option.value)}>{option.label}</button>
          ))}
        </div>
      ) : (
        <input value={answer} onChange={(event) => setAnswer(event.target.value)} placeholder="输入答复" aria-label="决策答复" />
      )}
      <div className={styles.responseActions}>
        <button onClick={() => void submit("decline")} disabled={submitting}>拒绝</button>
        <button data-primary onClick={() => void submit("answer")} disabled={submitting || !answer.trim()}>{submitting ? "提交中…" : "提交答复"}</button>
      </div>
      {submitError && <small className={styles.submitError}>{submitError}</small>}
    </div>
  );
}
