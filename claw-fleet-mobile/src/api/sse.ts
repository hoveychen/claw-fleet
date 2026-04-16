import { useEffect, useRef } from "react";
import type {
  SessionInfo,
  WaitingAlert,
  GuardRequest,
  ElicitationRequest,
} from "../types";

interface SSEHandlers {
  onSessionsUpdated?: (sessions: SessionInfo[]) => void;
  onWaitingAlert?: (alert: WaitingAlert) => void;
  onGuardRequest?: (request: GuardRequest) => void;
  onElicitationRequest?: (request: ElicitationRequest) => void;
  onConnect?: () => void;
  onDisconnect?: () => void;
}

/**
 * SSE consumer using fetch + ReadableStream.
 *
 * React Native 0.81+ supports ReadableStream via the Hermes polyfill,
 * which is sufficient for text/event-stream parsing.
 * Falls back to polling if streaming isn't available.
 */
export function useSSE(sseUrl: string | null, handlers: SSEHandlers) {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  useEffect(() => {
    if (!sseUrl) return;

    let cancelled = false;

    async function connect() {
      while (!cancelled) {
        try {
          const res = await fetch(sseUrl!, {
            headers: { Accept: "text/event-stream" },
          });

          if (!res.ok || !res.body) {
            // Server not ready or body streaming not supported — retry after delay
            await delay(5000);
            continue;
          }

          handlersRef.current.onConnect?.();

          const reader = res.body.getReader();
          const decoder = new TextDecoder();
          let buffer = "";

          while (!cancelled) {
            const { done, value } = await reader.read();
            if (done) break;

            buffer += decoder.decode(value, { stream: true });

            // Parse SSE frames: "event: <type>\ndata: <json>\n\n"
            const frames = buffer.split("\n\n");
            // Keep the last (potentially incomplete) frame in buffer
            buffer = frames.pop() ?? "";

            for (const frame of frames) {
              if (!frame.trim()) continue;
              const parsed = parseSSEFrame(frame);
              if (!parsed) continue;
              dispatch(parsed.event, parsed.data, handlersRef.current);
            }
          }

          reader.cancel().catch(() => {});
        } catch {
          // Connection failed or dropped
        }

        if (!cancelled) {
          handlersRef.current.onDisconnect?.();
          await delay(3000);
        }
      }
    }

    connect();

    return () => {
      cancelled = true;
    };
  }, [sseUrl]);
}

function parseSSEFrame(
  frame: string,
): { event: string; data: string } | null {
  let event = "message";
  let data = "";

  for (const line of frame.split("\n")) {
    if (line.startsWith("event: ")) {
      event = line.slice(7).trim();
    } else if (line.startsWith("data: ")) {
      data = line.slice(6);
    } else if (line.startsWith(":")) {
      // Comment line (heartbeat) — ignore
    }
  }

  if (!data) return null;
  return { event, data };
}

function dispatch(event: string, data: string, handlers: SSEHandlers) {
  try {
    switch (event) {
      case "sessions-updated": {
        const sessions: SessionInfo[] = JSON.parse(data);
        handlers.onSessionsUpdated?.(sessions);
        break;
      }
      case "waiting-alert": {
        const alert: WaitingAlert = JSON.parse(data);
        handlers.onWaitingAlert?.(alert);
        break;
      }
      case "guard-request": {
        const req: GuardRequest = JSON.parse(data);
        handlers.onGuardRequest?.(req);
        break;
      }
      case "elicitation-request": {
        const req: ElicitationRequest = JSON.parse(data);
        handlers.onElicitationRequest?.(req);
        break;
      }
    }
  } catch {
    // Malformed JSON — skip
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
