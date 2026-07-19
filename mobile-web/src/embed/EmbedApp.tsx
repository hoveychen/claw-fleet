import { useEffect, useMemo, useState } from "react";
import { CloudApiClient } from "../data/CloudApiClient";
import { CloudWorkspace } from "../cloud/CloudWorkspace";
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
    <EmbeddedTask apiBaseUrl={apiBaseUrl} token={token} taskId={taskId} parentOrigin={parentOrigin} view={view} />
  );
}

function EmbeddedTask({
  apiBaseUrl,
  token,
  taskId,
  parentOrigin,
  view,
}: {
  apiBaseUrl: string;
  token: string;
  taskId?: string;
  parentOrigin: string;
  view: EmbedView;
}) {
  const client = useMemo(() => new CloudApiClient({
      baseUrl: apiBaseUrl,
      token: () => tokenMemory.get(),
      headers: { "fleet-embed-parent-origin": parentOrigin },
    }), [apiBaseUrl, parentOrigin, token]);
  return <CloudWorkspace client={client} initialTaskId={taskId} initialView={view} embedded />;
}
