import type { Attempt, Decision, TaskDetail, TaskEvent, TaskStatus } from "./types";

export interface CloudTranscriptMessage {
  id: string;
  role: "user" | "assistant" | "system";
  text: string;
  occurredAt: string;
}

export interface CloudTaskState {
  task: TaskDetail;
  attempts: Attempt[];
  decisions: Decision[];
  messages: CloudTranscriptMessage[];
  refetchAfterGap: { expected: number; received: number } | null;
}

export function initialCloudTaskState(detail: TaskDetail): CloudTaskState {
  return {
    task: { ...detail },
    attempts: [...detail.attempts],
    decisions: [...detail.decisions],
    messages: [],
    refetchAfterGap: null,
  };
}

function upsert<T extends { id: string }>(items: T[], item: T): T[] {
  const index = items.findIndex((candidate) => candidate.id === item.id);
  if (index < 0) return [...items, item];
  return items.map((candidate, candidateIndex) => (candidateIndex === index ? item : candidate));
}

export function applyTaskEvent(state: CloudTaskState, event: TaskEvent): CloudTaskState {
  if (event.task_id !== state.task.id || event.sequence <= state.task.event_cursor) return state;
  if (event.sequence !== state.task.event_cursor + 1) {
    return {
      ...state,
      refetchAfterGap: {
        expected: state.task.event_cursor + 1,
        received: event.sequence,
      },
    };
  }

  let task: TaskDetail = { ...state.task, event_cursor: event.sequence, updated_at: event.occurred_at };
  let attempts = state.attempts;
  let decisions = state.decisions;
  let messages = state.messages;

  if (event.type === "task.status_changed" && typeof event.data.status === "string") {
    task = { ...task, status: event.data.status as TaskStatus, version: task.version + 1 };
  } else if (event.type === "attempt.started" && event.data.attempt) {
    const attempt = event.data.attempt as unknown as Attempt;
    attempts = upsert(attempts, attempt);
    task = { ...task, current_attempt_id: attempt.id };
  } else if (event.type === "attempt.ended") {
    const attempt = event.data.attempt as unknown as Attempt | undefined;
    if (attempt) attempts = upsert(attempts, attempt);
  } else if (event.type === "decision.opened" && event.data.decision) {
    const decision = event.data.decision as unknown as Decision;
    decisions = upsert(decisions, decision);
    task = {
      ...task,
      status: decision.blocking ? "waiting_for_input" : task.status,
      waiting_decision_count: decisions.filter((candidate) => candidate.status === "open").length,
    };
  } else if (event.type === "decision.resolved" && event.data.decision) {
    const decision = event.data.decision as unknown as Decision;
    decisions = upsert(decisions, decision);
    task = {
      ...task,
      waiting_decision_count: decisions.filter((candidate) => candidate.status === "open").length,
    };
  } else if (
    event.type === "transcript.message" &&
    (event.data.role === "user" || event.data.role === "assistant" || event.data.role === "system") &&
    typeof event.data.text === "string"
  ) {
    messages = [
      ...messages,
      {
        id: event.id,
        role: event.data.role,
        text: event.data.text,
        occurredAt: event.occurred_at,
      },
    ];
  }

  return { task, attempts, decisions, messages, refetchAfterGap: null };
}
