// ── Background-task kind → icon / label / colour ───────────────────────────────
// The Stop hook reports each outstanding background task's `type` (see
// `bg_guard::BackgroundTask`). We surface a distinct icon and i18n label per kind
// so a session waiting on a monitor reads differently from one waiting on a
// shell. `dataType` drives the label colour via a `[data-bgtype]` CSS rule.
//
// Shared by the card row (`SessionCard`, aggregated single row) and the detail
// tab (`SessionDetail`, one row per task) so both read the same icon per kind.
export const BG_TASK_KINDS: Record<
  string,
  { icon: string; labelKey: string; dataType: string }
> = {
  monitor: { icon: "👁️", labelKey: "card.bg_monitor", dataType: "monitor" },
  shell: { icon: "⏳", labelKey: "card.bg_shell", dataType: "shell" },
  subagent: { icon: "🤖", labelKey: "card.bg_subagent", dataType: "subagent" },
  workflow: { icon: "⚙", labelKey: "card.bg_workflow", dataType: "workflow" },
  // Mixed kinds (or an unknown type) — stay generic rather than mislabel.
  mixed: { icon: "⏳", labelKey: "card.bg_tasks", dataType: "mixed" },
};

/** Icon for a single task's `type`, falling back to the generic mixed icon for
 *  an unknown kind. */
export function bgTaskIcon(type: string): string {
  return (BG_TASK_KINDS[type] ?? BG_TASK_KINDS.mixed).icon;
}

/** `dataType` for a single task's `type`, for the `[data-bgtype]` colour rule. */
export function bgTaskDataType(type: string): string {
  return (BG_TASK_KINDS[type] ?? BG_TASK_KINDS.mixed).dataType;
}
