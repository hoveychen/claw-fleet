import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";

import { appendTailDelta } from "./tailDelta";
import type { RawMessage, TailDelta } from "./types";

/** Tail window the rail's agent preview opens with.
 *
 *  Much smaller than the detail pane's `INITIAL_TAIL` (150) on purpose: a
 *  subagent is one Task run, and the preview is a card floating over someone
 *  else's conversation — "what is it doing" is answered by the last few dozen
 *  records, not by its whole history. It grows by following, never by
 *  re-reading a wider window. */
export const AGENT_PREVIEW_TAIL = 80;

/** Follow cadence, matching SessionDetail's standalone live tail. */
const POLL_MS = 1500;

export interface AgentTranscript {
  messages: RawMessage[];
  isLoading: boolean;
  /** The fetch failed or blew past its first attempt with nothing to show. */
  stalled: boolean;
  retry: () => void;
}

/**
 * The transcript behind a rail subagent card, fetched and followed on its own.
 *
 * It cannot use `useDetailStore`: that store is a **singleton** with one open
 * session, one `session-tail` listener and a backend watcher that
 * `stop_watching_session` takes no argument to scope — opening a subagent
 * through it would close the parent conversation the card is floating over.
 * That is exactly the "clicking a subagent navigates away" behaviour this card
 * exists to replace, so the preview reads the file itself.
 *
 * Follow is incremental (`get_messages_since` from a byte cursor), the same
 * mechanism the detail pane uses, so a running fan-out costs one small delta
 * read per tick rather than re-parsing the window. Sources with no file behind
 * the path (dsh://) report offset 0; those fall back to re-reading the window.
 *
 * @param jsonlPath transcript to read; `undefined` parks the hook (no card
 *   expanded) and clears whatever the last one showed.
 * @param live whether the agent is still running — the only reason to poll.
 */
export function useAgentTranscript(
  jsonlPath: string | undefined,
  live: boolean,
): AgentTranscript {
  const [messages, setMessages] = useState<RawMessage[]>([]);
  const [isLoading, setLoading] = useState(false);
  const [stalled, setStalled] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const offsetRef = useRef<number | null>(null);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  // Initial window. Takes the follow cursor *before* reading the window and
  // awaits it, so a record written between the two arrives in both and is
  // deduped by `appendTailDelta` — the other order drops it (see SessionDetail).
  useEffect(() => {
    offsetRef.current = null;
    if (!jsonlPath) {
      setMessages([]);
      setLoading(false);
      setStalled(false);
      return;
    }
    let cancelled = false;
    setMessages([]);
    setLoading(true);
    setStalled(false);
    void invoke<TailDelta>("get_messages_since", { jsonlPath, offset: null })
      .then((d) => {
        if (!cancelled && d.offset > 0) offsetRef.current = d.offset;
      })
      .catch(() => {})
      .then(() =>
        invoke<RawMessage[]>("get_messages_tail", {
          jsonlPath,
          tail: AGENT_PREVIEW_TAIL,
        }),
      )
      .then((msgs) => {
        if (cancelled) return;
        setMessages(msgs ?? []);
        setLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        setLoading(false);
        setStalled(true);
      });
    return () => {
      cancelled = true;
    };
  }, [jsonlPath, attempt]);

  // Follow while it runs. A finished agent's transcript is final, so the
  // poller stops the moment the scan says so — this card can be left open.
  useEffect(() => {
    if (!jsonlPath || !live) return;
    let cancelled = false;
    let inFlight = false;
    const poll = () => {
      // A hidden window has nobody reading the card.
      if (inFlight || document.hidden) return;
      inFlight = true;
      const offset = offsetRef.current;
      const done = () => {
        inFlight = false;
      };
      if (offset !== null) {
        void invoke<TailDelta>("get_messages_since", { jsonlPath, offset })
          .then((d) => {
            if (cancelled) return;
            offsetRef.current = d.offset;
            if (d.messages.length > 0) {
              setMessages((prev) => appendTailDelta(prev, d.messages));
            }
          })
          // Drop back to the window path rather than going quiet: a follower
          // that stops delivering looks exactly like an idle agent.
          .catch(() => {
            if (!cancelled) offsetRef.current = null;
          })
          .finally(done);
        return;
      }
      void invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath,
        tail: AGENT_PREVIEW_TAIL,
      })
        .then((msgs) => {
          if (!cancelled) setMessages(msgs ?? []);
        })
        .catch(() => {})
        .finally(done);
    };
    const timer = window.setInterval(poll, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [jsonlPath, live]);

  return { messages, isLoading, stalled, retry };
}
