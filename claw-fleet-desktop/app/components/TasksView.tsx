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
          onAccept={() => acceptTask(selectedTask.id)}
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
          await refresh(selectedProjectId ?? undefined);
        }}
      />
    </div>
  );
}
