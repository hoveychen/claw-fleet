import { useCallback, useEffect, useMemo, useState } from "react";
import type { Decision, FleetDataClient, OperationsIssue, OperationsSnapshot, Task, TranscriptRecord, Usage } from "../data/FleetDataClient";
import styles from "./CloudWorkspace.module.css";

export type CloudView = "tasks" | "task_detail" | "decision_inbox" | "decision_card" | "usage" | "operations";

interface Props {
  client: FleetDataClient;
  projectId?: string;
  initialTaskId?: string;
  initialView?: CloudView;
  embedded?: boolean;
  onNavigate?: (taskId?: string, view?: CloudView) => void;
}

export function CloudWorkspace({ client, projectId, initialTaskId, initialView = "tasks", embedded, onNavigate }: Props) {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [taskId, setTaskId] = useState(initialTaskId);
  const [view, setView] = useState<CloudView>(initialView);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    if (embedded) return;
    try { setTasks((await client.listTasks({ projectId, limit: 100 })).data); setError(null); }
    catch (value) { setError(message(value)); }
  }, [client, embedded, projectId]);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => { const sub = client.streamEvents({ projectId, taskId, onEvent: () => void refresh(), onError: () => {} }); return () => sub.close(); }, [client, projectId, refresh, taskId]);
  const navigate = (nextTask?: string, nextView: CloudView = nextTask ? "task_detail" : "tasks") => {
    setTaskId(nextTask); setView(nextView); onNavigate?.(nextTask, nextView);
  };
  return <div className={styles.shell} data-embedded={embedded || undefined}>
    {!embedded && <header className={styles.header}><div><b>Fleet Cloud</b><span>Hosted Console</span></div><nav>
      <button data-active={view === "tasks"} onClick={() => navigate()}>Tasks</button>
      <button data-active={view === "decision_inbox"} onClick={() => navigate(undefined, "decision_inbox")}>Decisions</button>
      <button data-active={view === "operations"} onClick={() => navigate(undefined, "operations")}>Operations</button>
    </nav></header>}
    {error && <div className={styles.error} role="alert">{error}</div>}
    <main className={styles.main}>
      {view === "tasks" && <TaskList tasks={tasks} onOpen={(id) => navigate(id)} />}
      {view === "decision_inbox" && <DecisionInbox client={client} projectId={projectId} taskId={taskId} />}
      {view === "decision_card" && <DecisionInbox client={client} projectId={projectId} taskId={taskId} single />}
      {view === "operations" && <OperationsView client={client} />}
      {(view === "task_detail" || view === "usage") && taskId && <TaskDetail client={client} taskId={taskId} usageOnly={view === "usage"} onBack={() => navigate()} />}
    </main>
  </div>;
}

function OperationsView({ client }: { client: FleetDataClient }) {
  const [snapshot, setSnapshot] = useState<OperationsSnapshot>();
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    if (!client.getOperations) { setError("Operations are unavailable for this connection."); return; }
    void client.getOperations().then((value) => { setSnapshot(value); setError(null); }, (value) => setError(message(value)));
  }, [client]);
  useEffect(() => { load(); const timer = window.setInterval(load, 15_000); return () => window.clearInterval(timer); }, [load]);
  const sections: Array<[string, string, OperationsIssue[]]> = snapshot ? [
    ["Webhook delivery", "FAILED", snapshot.failed_webhooks],
    ["Runner heartbeat", "OFFLINE", snapshot.offline_runners],
    ["Command queue", "STALE", snapshot.stale_commands],
    ["Retention", "FAILED", snapshot.retention_failures],
  ] : [];
  const total = sections.reduce((sum, section) => sum + section[2].length, 0);
  return <section><div className={styles.sectionHead}><div><p className={styles.eyebrow}>CONTROL PLANE / LIVE</p><h1>Operations</h1></div><span>{snapshot ? `${total} open signals` : "loading"}</span></div>
    {error && <div className={styles.error}>{error}</div>}
    <div className={styles.opsLedger}>{sections.map(([title, state, issues]) => <section key={title} className={styles.opsSection}><header><h2>{title}</h2><span>{issues.length}</span></header>
      {issues.length === 0 ? <p className={styles.opsClear}>CLEAR</p> : issues.map((issue, index) => <div className={styles.opsRow} key={issue.id || issue.job || index}><b>{issue.name || issue.job || issue.id}</b><code>{issue.event_id || issue.status || issue.error || "requires attention"}</code><time>{relative(issue.updated_at || issue.created_at || issue.last_heartbeat_at || new Date().toISOString())}</time><span>{state}</span></div>)}
    </section>)}</div>
  </section>;
}

function TaskList({ tasks, onOpen }: { tasks: Task[]; onOpen: (id: string) => void }) {
  return <section><div className={styles.sectionHead}><div><p className={styles.eyebrow}>PROJECT QUEUE</p><h1>Tasks</h1></div><span>{tasks.length} total</span></div>
    <div className={styles.taskList}>{tasks.map((task) => <button key={task.id} className={styles.taskRow} onClick={() => onOpen(task.id)}>
      <span className={styles.status} data-status={task.status} />
      <span className={styles.taskCopy}><b>{task.title || task.goal}</b><small>{task.external_id || task.id}</small></span>
      <span className={styles.taskMeta}>{task.status.replace("_", " ")}<small>{relative(task.updated_at)}</small></span>
    </button>)}</div>
    {tasks.length === 0 && <div className={styles.empty}>No tasks in this project yet.</div>}
  </section>;
}

function TaskDetail({ client, taskId, usageOnly, onBack }: { client: FleetDataClient; taskId: string; usageOnly: boolean; onBack: () => void }) {
  const [task, setTask] = useState<Task>(); const [records, setRecords] = useState<TranscriptRecord[]>([]);
  const [usage, setUsage] = useState<Usage>(); const [decisions, setDecisions] = useState<Decision[]>([]);
  const [tab, setTab] = useState<"messages"|"decisions"|"usage">(usageOnly ? "usage" : "messages");
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => { try {
    const next = await client.getTask(taskId); setTask(next);
    setDecisions((await client.listDecisions({ taskId, limit: 100 })).data);
    if (next.active_run_id) {
      const [messages, totals] = await Promise.all([client.listRunMessages(next.active_run_id), client.getRunUsage(next.active_run_id)]);
      setRecords(messages.data); setUsage(totals);
    } setError(null);
  } catch (value) { setError(message(value)); } }, [client, taskId]);
  useEffect(() => { void load(); const sub = client.streamEvents({ taskId, onEvent: () => void load() }); return () => sub.close(); }, [client, load, taskId]);
  if (!task) return <div className={styles.empty}>{error || "Loading task…"}</div>;
  return <section><button className={styles.back} onClick={onBack}>← All tasks</button>
    <div className={styles.detailHead}><div><p className={styles.eyebrow}>{task.external_id || task.id}</p><h1>{task.title || task.goal}</h1><p>{task.goal}</p></div><span className={styles.stateChip}>{task.status}</span></div>
    <div className={styles.tabs}>{(["messages","decisions","usage"] as const).map((name) => <button key={name} data-active={tab === name} onClick={() => setTab(name)}>{name}</button>)}</div>
    {tab === "messages" && <Transcript records={records} />}
    {tab === "decisions" && <DecisionList client={client} decisions={decisions} onChanged={load} />}
    {tab === "usage" && <UsagePanel usage={usage} />}
  </section>;
}

function Transcript({ records }: { records: TranscriptRecord[] }) { return <div className={styles.transcript}>{records.map((record) => <article key={record.id} data-role={record.role}><header>{record.role}<time>{new Date(record.occurred_at).toLocaleString()}</time></header><div>{record.content.map((part, i) => <pre key={i}>{contentText(part)}</pre>)}</div></article>)}{records.length === 0 && <div className={styles.empty}>No transcript records yet.</div>}</div>; }

function DecisionInbox({ client, projectId, taskId, single }: { client: FleetDataClient; projectId?: string; taskId?: string; single?: boolean }) {
  const [decisions, setDecisions] = useState<Decision[]>([]); const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => client.listDecisions({ projectId, taskId, status: "pending", limit: 100 }).then((p) => setDecisions(p.data), (e) => setError(message(e))), [client, projectId, taskId]);
  useEffect(() => { load(); const sub = client.streamEvents({ projectId, taskId, onEvent: load }); return () => sub.close(); }, [client, load, projectId, taskId]);
  return <section><div className={styles.sectionHead}><div><p className={styles.eyebrow}>ACTION REQUIRED</p><h1>Decisions</h1></div><span>{decisions.length} pending</span></div>{error && <div className={styles.error}>{error}</div>}<DecisionList client={client} decisions={single ? decisions.slice(0,1) : decisions} onChanged={load} /></section>;
}

function DecisionList({ client, decisions, onChanged }: { client: FleetDataClient; decisions: Decision[]; onChanged: () => void }) { return <div className={styles.decisionList}>{decisions.map((decision) => <DecisionCard key={decision.id} client={client} decision={decision} onChanged={onChanged} />)}{decisions.length === 0 && <div className={styles.empty}>Nothing is waiting for you.</div>}</div>; }

function DecisionCard({ client, decision, onChanged }: { client: FleetDataClient; decision: Decision; onChanged: () => void }) {
  const [answer, setAnswer] = useState(""); const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null);
  const payload = decision.payload as Record<string, unknown>; const options = Array.isArray(payload.options) ? payload.options : [];
  const richCardUrl = safeRichCardUrl(payload.rich_html_url);
  const submit = async (action: "answer"|"decline", value?: unknown) => { setBusy(true); try { await client.respondToDecision(decision.id, decision.version, { action, ...(action === "answer" ? { answers: { value: value ?? answer } } : {}) }, crypto.randomUUID()); onChanged(); } catch (e) { setError(message(e)); } finally { setBusy(false); } };
  return <article className={styles.decision}><header><span>{decision.kind.replaceAll("_", " ")}</span><small>{decision.id}</small></header><h2>{String(payload.title || payload.question || "Decision required")}</h2>{payload.description != null && <p>{String(payload.description)}</p>}
    {richCardUrl && <iframe className={styles.richCard} src={richCardUrl} title="Decision card" sandbox="allow-forms allow-scripts" />}
    {options.length > 0 ? <div className={styles.options}>{options.map((option, i) => { const item = option as Record<string, unknown>; const value = item.value ?? item.id ?? item.label ?? option; return <button disabled={busy} key={i} onClick={() => void submit("answer", value)}><b>{String(item.label ?? option)}</b>{item.description ? <small>{String(item.description)}</small> : null}</button>; })}</div> : <textarea value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder="Your answer" rows={3} />}
    <footer>{options.length === 0 && <button disabled={busy || !answer.trim()} className={styles.primary} onClick={() => void submit("answer")}>Submit answer</button>}<button disabled={busy} onClick={() => void submit("decline")}>Decline</button></footer>{error && <p className={styles.error}>{error}</p>}
  </article>;
}

function UsagePanel({ usage }: { usage?: Usage }) { const rows = useMemo(() => usage ? [["Input", usage.input_tokens],["Output",usage.output_tokens],["Cache read",usage.cache_read_tokens],["Reasoning",usage.reasoning_tokens],["Run time",`${usage.run_seconds}s`],["Artifacts",formatBytes(usage.artifact_bytes)]] : [], [usage]); return <div className={styles.usage}>{usage ? <><div className={styles.cost}><small>Provider cost</small><b>${usage.provider_cost_usd.toFixed(2)}</b></div>{rows.map(([label,value]) => <div key={String(label)}><span>{label}</span><b>{typeof value === "number" ? value.toLocaleString() : value}</b></div>)}</> : <div className={styles.empty}>Usage is not available yet.</div>}</div>; }

function contentText(part: { type: string; [key: string]: unknown }): string { const value = part.text ?? part.content ?? part.output ?? part; return typeof value === "string" ? value : JSON.stringify(value, null, 2); }
function message(value: unknown) { return value instanceof Error ? value.message : String(value); }
function relative(value: string) { const minutes = Math.max(0, Math.floor((Date.now() - new Date(value).getTime()) / 60000)); return minutes < 1 ? "now" : minutes < 60 ? `${minutes}m` : `${Math.floor(minutes/60)}h`; }
function formatBytes(value: number) { return value < 1024 ? `${value} B` : `${(value/1024).toFixed(1)} KB`; }
function safeRichCardUrl(value: unknown): string | null { if (typeof value !== "string") return null; try { const url = new URL(value); const allowed = import.meta.env.VITE_CLOUD_RICH_CARD_ORIGIN || "https://fleet-cards.muveeai.com"; return url.origin === allowed ? url.toString() : null; } catch { return null; } }
