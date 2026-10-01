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
  RefreshCw,
} from "lucide-react";
import appStyles from "../App.module.css";
import { EmptyState } from "../views/EmptyState";
import { ConnIcon, type ConnIconKind } from "../views/ConnIcon";
import { HttpFleetCloudClient, type FleetCloudClient } from "./client";
import { applyTaskEvent, initialCloudTaskState, type CloudTaskState } from "./reducer";
import type { Decision, Task, TaskDetail, TaskStatus } from "./types";
import { Skeleton, SkeletonCard, SkeletonList, SkeletonNumber, Spinner, TopProgress } from "../views/loading";
import styles from "./CloudApp.module.css";
import { t, useI18n } from "../i18n";

type CloudTab = "tasks" | "decisions";

type SyncState = "online" | "syncing" | "offline";

/** Cloud header uses the same connection icon as the main app. There's no link strength to measure here,
 *  only three sync states, so we borrow the icon's "full / reconnecting / offline" appearances. */
function cloudConnKind(s: SyncState): ConnIconKind {
  return s === "online" ? "good" : s === "syncing" ? "connecting" : "offline";
}

function cloudConnText(s: SyncState): string {
  return s === "online" ? t("Cloud 在线") : s === "syncing" ? t("同步中") : t("连接失败");
}

/** Values are i18n keys; translate at the use site with `t()`. */
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
  return task.title?.trim() || task.prompt.split("\n")[0]?.slice(0, 88) || t("未命名任务");
}

function timeAgo(timestamp: string): string {
  const elapsed = Math.max(0, Date.now() - new Date(timestamp).getTime());
  if (elapsed < 60_000) return t("刚刚");
  if (elapsed < 3_600_000) return t("{0} 分钟前", Math.floor(elapsed / 60_000));
  if (elapsed < 86_400_000) return t("{0} 小时前", Math.floor(elapsed / 3_600_000));
  return t("{0} 天前", Math.floor(elapsed / 86_400_000));
}

function decisionQuestion(decision: Decision): string {
  const value = decision.presentation.question ?? decision.presentation.title ?? decision.presentation.prompt;
  return typeof value === "string" ? value : t("Agent 正在等待你的决定");
}

interface CloudAppProps {
  client?: FleetCloudClient;
}

export function CloudApp({ client: suppliedClient }: CloudAppProps) {
  useI18n();
  const client = useMemo(() => suppliedClient ?? defaultClient(), [suppliedClient]);
  const embedTaskId = suppliedClient ? null : import.meta.env.VITE_FLEET_CLOUD_EMBED_TASK_ID || null;
  const [tab, setTab] = useState<CloudTab>("tasks");
  const [tasks, setTasks] = useState<Task[]>([]);
  const [details, setDetails] = useState<Record<string, TaskDetail>>({});
  const [selectedId, setSelectedId] = useState<string | null>(embedTaskId);
  const [detailState, setDetailState] = useState<CloudTaskState | null>(null);
  const [detailReload, setDetailReload] = useState(0);
  const [loading, setLoading] = useState(true);
  // First successful reads: the task list, then every task's detail (which is
  // where the open decisions come from). Until each lands its counts and list
  // are unknown, not zero.
  const [tasksLoaded, setTasksLoaded] = useState(false);
  const [detailsLoaded, setDetailsLoaded] = useState(false);
  // The detail is being re-read after an event gap; the old one stays on screen.
  const [detailRefetching, setDetailRefetching] = useState(false);
  // Event cursor the task had when its detail was read: the replay from cursor 0
  // has caught up once the reduced state reaches it.
  const [replayTarget, setReplayTarget] = useState(0);
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
        setTasksLoaded(true);
        setDetailsLoaded(true);
        setSyncState("online");
        return;
      }
      const page = await client.listTasks({ limit: 100 });
      setTasks(page.data);
      setTasksLoaded(true);
      const loaded = await Promise.all(page.data.map((task) => client.getTask(task.id)));
      setDetails(Object.fromEntries(loaded.map((detail) => [detail.id, detail])));
      setDetailsLoaded(true);
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
        setReplayTarget(detail.event_cursor);
        setDetailState(seed);
        setDetailRefetching(false);
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
        setDetailRefetching(false);
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
    // Keep the stale detail on screen under a progress bar; the re-read replaces it.
    setDetailRefetching(true);
    setDetailReload((current) => current + 1);
  }, [detailState?.refetchAfterGap]);

  // Leaving a task must not leave the next one showing its detail meanwhile.
  useEffect(() => {
    setDetailState(null);
    setDetailRefetching(false);
    setReplayTarget(0);
  }, [selectedId]);

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
        refetching={detailRefetching}
        replaying={!!detailState && detailState.task.event_cursor < replayTarget}
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
          <div className={styles.projectLabel}>{t("托管工作区")}</div>
        </div>
        <button className={styles.refresh} onClick={() => void refresh()} aria-label={t("刷新")} disabled={loading}>
          {loading ? <Spinner size={15} /> : <RefreshCw size={15} />}
        </button>
        <span className={appStyles.connIcon} data-kind={cloudConnKind(syncState)} role="img" aria-label={cloudConnText(syncState)} title={cloudConnText(syncState)}>
          <ConnIcon kind={cloudConnKind(syncState)} />
        </span>
      </header>

      <main className={`${appStyles.main} ${styles.main}`}>
        <div className={styles.rail}>
          <button data-active={tab === "tasks"} onClick={() => setTab("tasks")}>
            <ListTodo size={16} />{t("任务")}{" "}
            <span>{tasksLoaded ? tasks.length : loading ? <SkeletonNumber width={16} /> : "—"}</span>
          </button>
          <button data-active={tab === "decisions"} onClick={() => setTab("decisions")}>
            <Inbox size={16} />{t("决策")}{" "}
            <span>
              {detailsLoaded ? openDecisions.length : loading ? <SkeletonNumber width={16} /> : "—"}
            </span>
          </button>
        </div>

        <section className={styles.content}>
          <div className={styles.sectionHead}>
            <div>
              <div className={styles.eyebrow}>{tab === "tasks" ? "RUN QUEUE" : "HUMAN LOOP"}</div>
              <h1>{tab === "tasks" ? t("云任务") : t("待处理决策")}</h1>
            </div>
            <Cloud size={20} aria-hidden="true" />
          </div>

          {error && <ErrorBanner message={error} onRetry={() => void refresh()} />}
          {loading && !tasksLoaded ? (
            <SkeletonList rows={6} />
          ) : tab === "tasks" ? (
            <TaskList tasks={tasks} onOpen={setSelectedId} />
          ) : loading && !detailsLoaded ? (
            // Open decisions come from the per-task details, which are still landing.
            <SkeletonList rows={3} />
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
    return <EmptyState icon={ListTodo} title={t("还没有云任务")} description={t("通过公开 Task API 创建的工作会出现在这里。")} />;
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
            <span className={styles.statusText} data-status={task.status}>{t(STATUS_LABEL[task.status])}</span>
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
    return <EmptyState icon={CheckCircle2} title={t("没有待处理的决策")} description={t("Agent 请求人工输入时，会连同所属 Attempt 出现在这里。")} />;
  }
  return (
    <div className={styles.decisionList}>
      {decisions.map((decision) => (
        <div key={decision.id} className={styles.decisionCard}>
          <button className={styles.decisionRow} onClick={() => onOpenTask(decision.task_id)}>
            <CircleAlert size={17} />
            <span>
              <strong>{decisionQuestion(decision)}</strong>
              <small>{taskTitle(details[decision.task_id] ?? ({ title: null, prompt: t("云任务") } as Task))} · {decision.kind.replaceAll("_", " ")}</small>
            </span>
            <ChevronRight size={16} />
          </button>
          <DecisionResponder client={client} decision={decision} onResolved={onResolved} />
        </div>
      ))}
    </div>
  );
}

function CloudDetail({ client, state, refetching, replaying, error, syncState, onBack, onDecisionResolved }: { client: FleetCloudClient; state: CloudTaskState | null; refetching: boolean; replaying: boolean; error: string | null; syncState: "online" | "syncing" | "offline"; onBack: () => void; onDecisionResolved: (decision: Decision) => void }) {
  const detail = state?.task;
  return (
    <div className={appStyles.app}>
      <header className={`${appStyles.header} ${styles.detailHeader}`}>
        <button className={styles.back} onClick={onBack}><ArrowLeft size={18} />{t("返回")}</button>
        <div className={styles.detailHeaderTitle}>
          {detail ? taskTitle(detail) : error ? t("读取任务") : <Skeleton width="45%" height={14} />}
        </div>
        <span className={appStyles.connIcon} data-kind={cloudConnKind(syncState)} role="img" aria-label={cloudConnText(syncState)} title={cloudConnText(syncState)}>
          <ConnIcon kind={cloudConnKind(syncState)} />
        </span>
      </header>
      <main className={`${appStyles.main} ${styles.detailMain}`}>
        <TopProgress active={refetching} />
        {error && <ErrorBanner message={error} />}
        {!state || !detail ? (
          // A failed read already shows its banner; only a pending one gets a placeholder.
          !error && (
            <div className={styles.detailSkeleton}>
              <SkeletonCard height={150} />
              <SkeletonList rows={5} />
            </div>
          )
        ) : (
          <>
            <section className={styles.detailIntro}>
              <div className={styles.eyebrow}>TASK / {detail.id.slice(0, 8)}</div>
              <h1>{taskTitle(detail)}</h1>
              <p>{detail.prompt}</p>
              <div className={styles.detailFacts}>
                <span data-status={detail.status}>{t(STATUS_LABEL[detail.status])}</span>
                {/* The event replay from cursor 0 is still catching up: these
                    count up from 0 until it has. */}
                <span>{replaying ? <SkeletonNumber width={56} /> : t("{0} 次尝试", state.attempts.length)}</span>
                <span>{replaying ? <SkeletonNumber width={56} /> : `Event #${detail.event_cursor}`}</span>
              </div>
            </section>

            <div className={styles.detailGrid}>
              <section className={styles.transcript}>
                <h2>{t("会话记录")}</h2>
                {state.messages.length === 0 && replaying ? (
                  <SkeletonList rows={4} />
                ) : state.messages.length === 0 ? (
                  <div className={styles.muted}>{t("还没有会话消息。")}</div>
                ) : state.messages.map((message) => (
                  <article key={message.id} data-role={message.role}>
                    <div><Bot size={14} />{message.role}<time>{new Date(message.occurredAt).toLocaleTimeString()}</time></div>
                    <p>{message.text}</p>
                  </article>
                ))}
              </section>

              <aside className={styles.attempts}>
                <h2>{t("尝试链")}</h2>
                {state.attempts.length === 0 && replaying ? <SkeletonList rows={2} /> : state.attempts.length === 0 ? <div className={styles.muted}>{t("等待 Runner 接单")}</div> : state.attempts.map((attempt) => (
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
  return <div className={styles.errorBanner} role="alert"><CircleAlert size={16} /><span>{message}</span>{onRetry && <button onClick={onRetry}>{t("重试")}</button>}</div>;
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
  // Which button is in flight, so only that one spins; both stay blocked.
  const [submitting, setSubmitting] = useState<"answer" | "decline" | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const submit = async (action: "answer" | "decline") => {
    if (submitting) return;
    if (action === "answer" && !answer.trim()) return;
    setSubmitting(action);
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
      setSubmitting(null);
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
        <input value={answer} onChange={(event) => setAnswer(event.target.value)} placeholder={t("输入答复")} aria-label={t("决策答复")} />
      )}
      <div className={styles.responseActions}>
        <button onClick={() => void submit("decline")} disabled={submitting !== null}>
          {submitting === "decline" && <Spinner size={12} />}
          {t("拒绝")}
        </button>
        <button data-primary onClick={() => void submit("answer")} disabled={submitting !== null || !answer.trim()}>
          {submitting === "answer" && <Spinner size={12} />}
          {submitting === "answer" ? t("提交中…") : t("提交答复")}
        </button>
      </div>
      {submitError && <small className={styles.submitError}>{submitError}</small>}
    </div>
  );
}
