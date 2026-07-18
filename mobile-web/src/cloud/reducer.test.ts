import { describe, expect, it } from "vitest";
import { applyTaskEvent, initialCloudTaskState } from "./reducer";
import type { Task, TaskEvent } from "./types";

const task: Task = {
  id: "task-1",
  organization_id: "org-1",
  project_id: "project-1",
  external_id: null,
  title: "Cloud spike",
  prompt: "ship it",
  status: "queued",
  current_attempt_id: null,
  waiting_decision_count: 0,
  event_cursor: 1,
  version: 1,
  created_at: "2026-07-18T00:00:00Z",
  updated_at: "2026-07-18T00:00:00Z",
};

function event(sequence: number, type: string, data: Record<string, unknown>): TaskEvent {
  return {
    id: `event-${sequence}`,
    organization_id: "org-1",
    project_id: "project-1",
    task_id: "task-1",
    attempt_id: null,
    sequence,
    type,
    occurred_at: `2026-07-18T00:00:0${sequence}Z`,
    recorded_at: `2026-07-18T00:00:0${sequence}Z`,
    producer: { type: "runner", id: "runner-1" },
    schema_version: "1.0",
    data,
  };
}

describe("cloud task event reducer", () => {
  it("projects task, attempt, decision and transcript events in sequence", () => {
    let state = initialCloudTaskState({ ...task, attempts: [], decisions: [] });
    state = applyTaskEvent(
      state,
      event(2, "attempt.started", {
        attempt: {
          id: "attempt-1",
          task_id: "task-1",
          runner_id: "runner-1",
          workspace_id: "workspace-1",
          agent_source: "codex",
          agent_session_id: "session-1",
          ordinal: 1,
          reason: "initial",
          status: "running",
          started_at: "2026-07-18T00:00:02Z",
          ended_at: null,
        },
      }),
    );
    state = applyTaskEvent(
      state,
      event(3, "decision.opened", {
        decision: {
          id: "decision-1",
          task_id: "task-1",
          attempt_id: "attempt-1",
          kind: "fleet_ask",
          blocking: true,
          schema_version: "1.0",
          presentation: { question: "Ship?" },
          status: "open",
          response: null,
          created_at: "2026-07-18T00:00:03Z",
          resolved_at: null,
        },
      }),
    );
    state = applyTaskEvent(
      state,
      event(4, "transcript.message", { role: "assistant", text: "Ready to ship." }),
    );
    state = applyTaskEvent(state, event(5, "task.status_changed", { status: "waiting_for_input" }));

    expect(state.task).toMatchObject({
      status: "waiting_for_input",
      current_attempt_id: "attempt-1",
      waiting_decision_count: 1,
      event_cursor: 5,
    });
    expect(state.attempts).toHaveLength(1);
    expect(state.decisions).toHaveLength(1);
    expect(state.messages).toEqual([
      { id: "event-4", role: "assistant", text: "Ready to ship.", occurredAt: "2026-07-18T00:00:04Z" },
    ]);
  });

  it("ignores duplicate or out-of-order events", () => {
    const state = applyTaskEvent(
      initialCloudTaskState({ ...task, event_cursor: 4, attempts: [], decisions: [] }),
      event(4, "task.status_changed", { status: "failed" }),
    );
    expect(state.task.status).toBe("queued");
    expect(state.task.event_cursor).toBe(4);
  });
});
