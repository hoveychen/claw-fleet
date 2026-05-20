import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/// Mirrors `claw_fleet_core::registry::RegistryEntry`. One per live fleet-task
/// process the desktop has discovered via `~/.fleet/runtime/`.
export interface RegistryEntry {
  task_id: string;
  pid: number;
  port: number;
  started_at: string;
}

interface DisappearedPayload {
  task_id: string;
}

interface RuntimeTasksState {
  /// Keyed by task_id. Updated by the watcher events from Rust.
  byTaskId: Record<string, RegistryEntry>;
  /// True once the initial `list_runtime_tasks` call has resolved.
  initialised: boolean;
  /// Pull the current snapshot from Rust and hydrate the store. Idempotent.
  refresh: () => Promise<void>;
  /// Subscribe to the three watcher events; returns a cleanup function.
  subscribe: () => Promise<() => void>;
}

export const useRuntimeTasksStore = create<RuntimeTasksState>((set, get) => ({
  byTaskId: {},
  initialised: false,
  refresh: async () => {
    try {
      const entries = await invoke<RegistryEntry[]>("list_runtime_tasks");
      const byTaskId: Record<string, RegistryEntry> = {};
      for (const e of entries) byTaskId[e.task_id] = e;
      set({ byTaskId, initialised: true });
    } catch {
      set({ initialised: true });
    }
  },
  subscribe: async () => {
    const unlisteners: UnlistenFn[] = [];

    unlisteners.push(
      await listen<RegistryEntry>("runtime-task-appeared", (e) => {
        const next = { ...get().byTaskId, [e.payload.task_id]: e.payload };
        set({ byTaskId: next });
      }),
    );

    unlisteners.push(
      await listen<DisappearedPayload>("runtime-task-disappeared", (e) => {
        const next = { ...get().byTaskId };
        delete next[e.payload.task_id];
        set({ byTaskId: next });
      }),
    );

    unlisteners.push(
      await listen<RegistryEntry>("runtime-task-changed", (e) => {
        const next = { ...get().byTaskId, [e.payload.task_id]: e.payload };
        set({ byTaskId: next });
      }),
    );

    return () => {
      for (const u of unlisteners) u();
    };
  },
}));

/// Convenience selector: is the given task currently backed by a live
/// fleet-task process?
export function selectIsTaskLive(task_id: string) {
  return (state: RuntimeTasksState) => Boolean(state.byTaskId[task_id]);
}

/// Convenience selector: registry entry for a task (port / pid / started_at)
/// or undefined if not running.
export function selectRuntimeEntry(task_id: string) {
  return (state: RuntimeTasksState) => state.byTaskId[task_id];
}
