import { useTranslation } from "react-i18next";
import {
  CLAUDE_EFFORT_CHOICES,
  CLAUDE_MODEL_CHOICES,
  CLAUDE_PERMISSION_MODE_CHOICES,
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
}: {
  model: string;
  effort: string;
  permissionMode: string;
  onModelChange: (v: string) => void;
  onEffortChange: (v: string) => void;
  onPermissionModeChange: (v: string) => void;
  disabled?: boolean;
  /** Label for the "" permission item; defaults to new_session.permission_default. */
  permissionDefaultLabel?: string;
  /** Popover side. Hosts near the top of a clipping panel (e.g. the resume
   *  composer in SessionDetail's header) must open below or the menu is cut
   *  off by the panel's overflow:hidden. */
  placement?: "above" | "below";
}) {
  const { t } = useTranslation();
  const modelLabel =
    CLAUDE_MODEL_CHOICES.find((m) => m.value === model)?.label ??
    t("new_session.model_pill_default");
  const effortLabel = effort || t("new_session.effort_pill_default");
  const permissionLabel = permissionMode
    ? t(`new_session.permission_${permissionMode}`)
    : t("new_session.permission_pill_default");
  return (
    <>
      <PillMenu
        placement={placement}
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
          ...CLAUDE_MODEL_CHOICES.map((m) => ({
            id: m.value,
            label: m.label,
            checked: m.value === model,
            onSelect: () => onModelChange(m.value),
          })),
        ]}
      />
      <PillMenu
        placement={placement}
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
          ...CLAUDE_EFFORT_CHOICES.map((e) => ({
            id: e,
            label: e,
            checked: e === effort,
            onSelect: () => onEffortChange(e),
          })),
        ]}
      />
      <PillMenu
        placement={placement}
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
    </>
  );
}
