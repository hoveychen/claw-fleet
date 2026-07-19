import { useMemo, useState } from "react";
import { CloudApiClient } from "../data/CloudApiClient";
import { CloudWorkspace, type CloudView } from "./CloudWorkspace";
import styles from "./CloudWorkspace.module.css";

function route() { const parts = location.pathname.split("/").filter(Boolean); const project = parts[0] === "project" ? parts[1] : undefined; const task = parts[2] === "tasks" ? parts[3] : undefined; return { project, task }; }

export function HostedApp({ apiBaseUrl }: { apiBaseUrl: string }) {
  const initial = route(); const [token, setToken] = useState(""); const [draft, setDraft] = useState(""); const [projectId, setProjectId] = useState(initial.project || "");
  const client = useMemo(() => token ? new CloudApiClient({ baseUrl: apiBaseUrl, token: () => token }) : null, [apiBaseUrl, token]);
  if (!client) return <main className={styles.signIn}><div><p className={styles.eyebrow}>FLEET CLOUD / PRIVATE PILOT</p><h1>Bring your runners.<br/>Keep command.</h1><p>Connect with a Project API Key. It stays in memory and disappears when this tab closes or reloads.</p><label>Project ID<input value={projectId} onChange={(e) => setProjectId(e.target.value)} placeholder="proj_…" /></label><label>Project API Key<input type="password" value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="flk_…" /></label><button className={styles.primary} disabled={!projectId.trim() || !draft.trim()} onClick={() => setToken(draft.trim())}>Open console</button></div><aside><span>01</span><p>Outbound-only runners</p><span>02</span><p>Durable decisions</p><span>03</span><p>Full redacted record</p></aside></main>;
  return <CloudWorkspace client={client} projectId={projectId} initialTaskId={initial.task} onNavigate={(taskId, view: CloudView = "tasks") => { const path = taskId ? `/project/${encodeURIComponent(projectId)}/tasks/${encodeURIComponent(taskId)}` : `/project/${encodeURIComponent(projectId)}${view === "decision_inbox" ? "/decisions" : ""}`; history.pushState({}, "", path); }} />;
}
