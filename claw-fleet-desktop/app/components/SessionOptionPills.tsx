import { useTranslation } from "react-i18next";
import {
  AGENT_TOOL_CHOICES,
  CLAUDE_EFFORT_CHOICES,
  CLAUDE_MODEL_CHOICES,
  CLAUDE_PERMISSION_MODE_CHOICES,
  CODEX_EFFORT_CHOICES,
  CODEX_MODEL_CHOICES,
} from "../modelChoices";
import { PillMenu } from "./PillMenu";

/** The model / effort / permission-mode ghost pills shared by the new-session
 *  modal and the history panel's resume composer. `""` means "don't pass the
 *  flag" — the default item's label is supplied per host because the semantics
 *  differ (new session: CLI default; resume: keep the session's own settings). */
export function SessionOptionPills({
  model,
  effort,
  permissionMode,
  onModelChange,
  onEffortChange,
  onPermissionModeChange,
  disabled,
  permissionDefaultLabel,
  placement = "above",
  tool = "claude",
  onToolChange,
  toolChoices = AGENT_TOOL_CHOICES,
  showPermission = true,
  compact = false,
}: {
  model: string;
  effort: string;
  permissionMode: string;
  onModelChange: (v: string) => void;
  onEffortChange: (v: string) => void;
  onPermissionModeChange: (v: string) => void;
  disabled?: boolean;
  /** Hide the permission-mode pill entirely. A schedule stores no permission
   *  mode (fired sessions inherit the CLI default), so its edit form omits it. */
  showPermission?: boolean;
  /** Label for the "" permission item; defaults to new_session.permission_default. */
  permissionDefaultLabel?: string;
  /** Popover side. Hosts near the top of a clipping panel (e.g. the resume
   *  composer in SessionDetail's header) must open below or the menu is cut
   *  off by the panel's overflow:hidden. */
  placement?: "above" | "below";
  /** Which agent tool the model/effort choices belong to. Codex has its own
   *  model ids + reasoning-effort scale and ignores Claude's permission modes,
   *  so the permission pill is hidden for it. */
  tool?: string;
  /** When provided, a leading tool-selector pill is shown (new-session only).
   *  Resume composers omit it — you can't change tool on an existing session. */
  onToolChange?: (v: string) => void;
  /** The agent tools offered by the tool-selector pill, already filtered to the
   *  monitored sources by the host. Defaults to the full catalog. When only one
   *  tool remains the pill is hidden — there is nothing to switch between. */
  toolChoices?: { value: string; label: string }[];
  /** Narrow-host (lite) mode: tighter pill chrome for the 340px strip. Labels are
   *  the same in both modes — lite used to swap in a shorter "默认" / "Default" for
   *  the un-chosen state, but now that a default pill just shows its category name
   *  there is nothing left to shorten (measured in the lite strip: identical row
   *  height, ≤12px of extra pill width, and that row wraps to two lines either
   *  way). */
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const isCodex = tool === "codex";
  const toolLabel = toolChoices.find((x) => x.value === tool)?.label ?? "Claude";
  const modelChoices = isCodex ? CODEX_MODEL_CHOICES : CLAUDE_MODEL_CHOICES;
  const effortChoices = isCodex ? CODEX_EFFORT_CHOICES : CLAUDE_EFFORT_CHOICES;
  // In the un-chosen ("") state a pill shows only its bare category name
  // ("Model" / "模型"), not a "…: default" value: the prefix+value form made the
  // toolbar too wide to hold one row (English overflowed outright). The menu's
  // own default item still spells out what "no choice" means, and picking a
  // value replaces the label with that value.
  const modelLabel =
    modelChoices.find((m) => m.value === model)?.label ?? t("new_session.model_pill_default");
  const effortLabel = effort || t("new_session.effort_pill_default");
  const permissionLabel = permissionMode
    ? t(`new_session.permission_${permissionMode}`)
    : t("new_session.permission_pill_default");
  return (
    <>
      {onToolChange && toolChoices.length > 1 && (
        <PillMenu
          placement={placement}
          compact={compact}
          label={toolLabel}
          title={t("new_session.tool")}
          disabled={disabled}
          items={toolChoices.map((x) => ({
            id: x.value,
            label: x.label,
            checked: x.value === tool,
            onSelect: () => onToolChange(x.value),
          }))}
        />
      )}
      <PillMenu
        placement={placement}
        compact={compact}
        label={modelLabel}
        title={t("new_session.model")}
        disabled={disabled}
        items={[
          {
            id: "",
            label: t("new_session.model_default"),
            checked: model === "",
            onSelect: () => onModelChange(""),
          },
          ...modelChoices.map((m) => ({
            id: m.value,
            label: m.label,
            checked: m.value === model,
            onSelect: () => onModelChange(m.value),
          })),
        ]}
      />
      <PillMenu
        placement={placement}
        compact={compact}
        label={effortLabel}
        title={t("new_session.effort")}
        disabled={disabled}
        items={[
          {
            id: "",
            label: t("new_session.effort_default"),
            checked: effort === "",
            onSelect: () => onEffortChange(""),
          },
          ...effortChoices.map((e) => ({
            id: e,
            label: e,
            checked: e === effort,
            onSelect: () => onEffortChange(e),
          })),
        ]}
      />
      {/* Codex has no --permission-mode analogue (its sandbox/approval mapping
          is a later milestone), so the permission pill is Claude-only. Hosts
          with no permission concept (e.g. schedule edit) hide it outright. */}
      {showPermission && !isCodex && (
        <PillMenu
          placement={placement}
          compact={compact}
          label={permissionLabel}
          title={t("new_session.permission")}
          disabled={disabled}
          items={[
            {
              id: "",
              label: permissionDefaultLabel ?? t("new_session.permission_default"),
              checked: permissionMode === "",
              onSelect: () => onPermissionModeChange(""),
            },
            ...CLAUDE_PERMISSION_MODE_CHOICES.map((m) => ({
              id: m,
              label: t(`new_session.permission_${m}`),
              checked: m === permissionMode,
              onSelect: () => onPermissionModeChange(m),
            })),
          ]}
        />
      )}
    </>
  );
}
