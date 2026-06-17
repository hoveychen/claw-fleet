// Task-as-Unit V2 — single-column tasks view.
//
// Two states, gated on selectedTaskId:
//   - selectedTaskId == null → the centered TaskComposer (default new-task
//     entry point). The flat task list lives in the sidebar now.
//   - selectedTaskId != null → the rebuilt TaskDetail (来龙去脉 timeline +
//     DAG plan + deliverables + metrics rail).

import { useEffect, useMemo } from "react";
import styles from "./TasksView.module.css";
import { TaskComposer } from "./TaskComposer";
import { TaskDetail } from "./TaskDetail";
import { useProjectsStore, useTasksStore } from "../store";
import { PROJECTS_FEATURE_ENABLED } from "../featureFlags";

export function TasksView() {
  if (!PROJECTS_FEATURE_ENABLED) return null;
  const { projects, refresh: refreshProjects } = useProjectsStore();
  const selectedProjectId = useProjectsStore((s) => s.selectedProjectId);
  const {
    tasks,
    selectedTaskId,
    setSelectedTaskId,
    refresh,
    startTask,
    acceptTask,
    rerunTaskE2e,
    updateTaskTitle,
    deleteTask,
  } = useTasksStore();

  useEffect(() => {
    refreshProjects();
    refresh(selectedProjectId ?? undefined);
  }, [refreshProjects, refresh, selectedProjectId]);

  const selectedTask = useMemo(
    () => tasks.find((tk) => tk.id === selectedTaskId) ?? null,
    [tasks, selectedTaskId],
  );

  if (selectedTask) {
    const taskProject = projects.find((p) => p.id === selectedTask.projectId) ?? null;
    return (
      <div className={styles.root}>
        <TaskDetail
          task={selectedTask}
          project={taskProject}
          onBack={() => setSelectedTaskId(null)}
          onStart={() => startTask(selectedTask.id)}
          onAccept={(mode) => acceptTask(selectedTask.id, mode)}
          onRerunE2e={() => rerunTaskE2e(selectedTask.id)}
          onRename={(title) => updateTaskTitle(selectedTask.id, title)}
          onDelete={() => deleteTask(selectedTask.id)}
        />
      </div>
    );
  }

  return (
    <div className={styles.root}>
      <TaskComposer
        defaultProjectId={selectedProjectId}
        onCreated={async (task) => {
          setSelectedTaskId(task.id);
          // Auto-start: creating a task means the user wants it to run, so kick
          // off planning immediately instead of stranding it in `drafting`
          // behind a manual ▶ Start click. start_task creates the fleet/<slug>
          // git branch + flips to running. If it fails (e.g. the workspace
          // isn't a git repo) the task stays in `drafting` and TaskDetail's
          // Start button is the recovery path — so we swallow the error here.
          try {
            await startTask(task.id);
          } catch {
            /* leave in drafting; Start button surfaces the error on retry */
          }
          await refresh(selectedProjectId ?? undefined);
        }}
      />
    </div>
  );
}
