import { useEffect, useMemo, useState } from "react";
import { CloudApiClient } from "../data/CloudApiClient";
import type { Task } from "../data/FleetDataClient";
import {
  EmbedAuthError,
  MemoryEmbedToken,
  parentOriginFromReferrer,
  postToParent,
  validateEmbedToken,
  type EmbedView,
} from "./embedAuth";

const tokenMemory = new MemoryEmbedToken();

interface Props {
  apiBaseUrl: string;
  taskId?: string;
  view: EmbedView;
}

export function EmbedApp({ apiBaseUrl, taskId, view }: Props) {
  const [token, setToken] = useState<string | null>(() => tokenMemory.get());
  const [error, setError] = useState<string | null>(null);
  const parentOrigin = useMemo(() => {
    try {
      return parentOriginFromReferrer(document.referrer);
    } catch {
      return null;
    }
  }, []);

  useEffect(() => {
    if (!parentOrigin) {
      setError("embed_origin_denied");
      return;
    }
    const receive = (event: MessageEvent) => {
      if (event.origin !== parentOrigin || event.source !== window.parent) return;
      if (event.data?.type !== "fleet.embed.token" || typeof event.data.token !== "string") return;
      try {
        validateEmbedToken(event.data.token, { parentOrigin, taskId, view });
        tokenMemory.set(event.data.token);
        setToken(event.data.token);
        setError(null);
      } catch (value) {
        setError(value instanceof EmbedAuthError ? value.code : "embed_token_invalid");
      }
    };
    window.addEventListener("message", receive);
    postToParent(window.parent, { type: "fleet.embed.ready" }, parentOrigin);
    return () => window.removeEventListener("message", receive);
  }, [parentOrigin, taskId, view]);

  if (!parentOrigin) return <main role="alert">embed_origin_denied</main>;
  if (error) return <main role="alert">{error}</main>;
  if (!token) return <main aria-busy="true">Waiting for secure embed token…</main>;
  return (
    <EmbeddedTask
      apiBaseUrl={apiBaseUrl}
      token={token}
      taskId={taskId}
      parentOrigin={parentOrigin}
    />
  );
}

function EmbeddedTask({
  apiBaseUrl,
  token,
  taskId,
  parentOrigin,
}: {
  apiBaseUrl: string;
  token: string;
  taskId?: string;
  parentOrigin: string;
}) {
  const [task, setTask] = useState<Task | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!taskId) return;
    const client = new CloudApiClient({
      baseUrl: apiBaseUrl,
      token: () => tokenMemory.get(),
      headers: { "fleet-embed-parent-origin": parentOrigin },
    });
    client.getTask(taskId).then(setTask, (value) => setError(value instanceof Error ? value.message : String(value)));
  }, [apiBaseUrl, parentOrigin, taskId, token]);
  if (error) return <main role="alert">{error}</main>;
  if (!taskId) return <main>Decision inbox</main>;
  if (!task) return <main aria-busy="true">Loading Task…</main>;
  return (
    <main>
      <p>{task.status}</p>
      <h1>{task.title || task.goal}</h1>
    </main>
  );
}
