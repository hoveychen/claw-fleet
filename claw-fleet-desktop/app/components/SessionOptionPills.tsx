import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ChevronLeft, ChevronRight } from "lucide-react";
import {
  AGENT_TOOL_CHOICES,
  CLAUDE_EFFORT_CHOICES,
  CLAUDE_MODEL_CHOICES,
  CLAUDE_PERMISSION_MODE_CHOICES,
  CODEX_MODEL_CHOICES,
  codexEffortChoices,
  codexProfileChoices,
  dshFindPick,
  dshModelMenu,
  type CodexProfile,
} from "../modelChoices";
import type { DshModelCatalog } from "../generated/types";
import { PillMenu, type PillMenuItem } from "./PillMenu";
import pillStyles from "./PillMenu.module.css";

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
  const isDsh = tool === "dsh";
  const toolLabel = toolChoices.find((x) => x.value === tool)?.label ?? "Claude";
  // Third-party Codex models are discovered from the host's profile files
  // rather than hardcoded — a `[model_providers.<id>]` block names no models,
  // so a profile is the only thing that does. Fetched only for Codex (the
  // Claude picker never shows them) and best-effort: a failure leaves the
  // built-in list intact rather than emptying the picker.
  const [codexProfiles, setCodexProfiles] = useState<CodexProfile[]>([]);
  useEffect(() => {
    if (!isCodex) return;
    let live = true;
    invoke<CodexProfile[]>("list_codex_profiles")
      .then((p) => {
        if (live) setCodexProfiles(p ?? []);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [isCodex]);
  // dsh addresses a model as `provider/model` and publishes the pair — plus a
  // per-model effort scale — through its own `llm.models` RPC, so neither of
  // Fleet's two curated lists applies to it. The catalogue is fetched instead,
  // and best-effort like Codex's profiles: a failure (no dsh on the host, the
  // server not starting, a `fleet serve` too old to know the route) leaves the
  // menu with only its "default" item, which is honest — the session then runs
  // on whatever `~/.dsh/settings.yaml` selects. Offering Claude's list here
  // would be worse than offering nothing: picking `claude-opus-5` for a dsh
  // session silently does nothing, because `dsh_source::split_model` needs a
  // provider prefix and drops a bare name.
  //
  // The fetch is NOT one-shot, though. The first load can land in `dsh web`'s
  // boot window right after the desktop relaunches (the server is reaped and
  // restarted on every app start), and a menu that never retries then lies —
  // "no options" — until the whole dialog is remounted. So the catalogue is
  // re-pulled every time the model popover opens (PillMenu.onOpen), and a
  // failure — transport error, or a partial catalogue whose per-provider
  // `failures` the harness answered — is shown in the menu with a retry,
  // instead of being swallowed.
  const [dshCatalog, setDshCatalog] = useState<DshModelCatalog | null>(null);
  /** Last transport failure of `dsh_models`, or null when the last attempt
   *  succeeded. The last good catalogue survives a failed refetch, so a flaky
   *  retry never blanks a menu that already rendered. */
  const [dshError, setDshError] = useState<string | null>(null);
  const [dshLoading, setDshLoading] = useState(false);
  /** Latest attempt wins — an older response never overwrites a newer one. */
  const dshFetchSeq = useRef(0);
  const loadDshCatalog = useCallback(() => {
    if (!isDsh) return;
    const seq = ++dshFetchSeq.current;
    setDshLoading(true);
    invoke<DshModelCatalog>("dsh_models")
      .then((c) => {
        if (seq !== dshFetchSeq.current) return;
        setDshCatalog(c ?? null);
        setDshError(null);
      })
      .catch((e: unknown) => {
        if (seq !== dshFetchSeq.current) return;
        setDshError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (seq === dshFetchSeq.current) setDshLoading(false);
      });
  }, [isDsh]);
  useEffect(() => {
    loadDshCatalog();
  }, [loadDshCatalog]);
  const dshMenu = useMemo(() => dshModelMenu(isDsh ? dshCatalog : null), [isDsh, dshCatalog]);
  const dshFailures = dshCatalog?.failures ?? [];
  // Which vendor folder the model menu is currently showing, `null` for the top
  // level. Deliberately *not* reset when the popover closes: after picking a
  // Claude model out of the `anthropic` folder, the next open lands back in that
  // folder, which is where the next pick almost always is.
  const [dshFolder, setDshFolder] = useState<string | null>(null);
  const modelChoices = useMemo(() => {
    if (isDsh) return [];
    return isCodex
      ? [...CODEX_MODEL_CHOICES, ...codexProfileChoices(codexProfiles)]
      : CLAUDE_MODEL_CHOICES;
  }, [isCodex, isDsh, codexProfiles]);
  // dsh publishes the ladder per model, so the effort menu follows the current
  // pick rather than a fixed table. A model with no reasoning control offers
  // nothing but "default" — the honest rendering of "this model has no effort
  // knob", where showing the previous model's ladder would invite a value dsh
  // will not honour.
  const dshPick = useMemo(() => dshFindPick(dshMenu, model), [dshMenu, model]);
  const effortChoices = isDsh
    ? (dshPick?.efforts ?? [])
    : isCodex
      ? codexEffortChoices(model)
      : CLAUDE_EFFORT_CHOICES;
  useEffect(() => {
    if (isCodex && effort && !codexEffortChoices(model).includes(effort)) {
      onEffortChange("");
    }
  }, [effort, isCodex, model, onEffortChange]);
  // In the un-chosen ("") state a pill shows only its bare category name
  // ("Model" / "模型"), not a "…: default" value: the prefix+value form made the
  // toolbar too wide to hold one row (English overflowed outright). The menu's
  // own default item still spells out what "no choice" means, and picking a
  // value replaces the label with that value.
  const modelLabel = isDsh
    ? (dshPick?.label ?? (model || t("new_session.model_pill_default")))
    : (modelChoices.find((m) => m.value === model)?.label ??
      t("new_session.model_pill_default"));
  const effortLabel = effort || t("new_session.effort_pill_default");
  // dsh names a per-model default, so the un-chosen effort item can say which
  // one it means instead of a bare "Default".
  const effortDefaultLabel =
    isDsh && dshPick?.defaultEffort
      ? `${t("new_session.effort_default")} (${dshPick.defaultEffort})`
      : t("new_session.effort_default");
  // The model menu's rows. Every agent but dsh has one flat list; dsh gets two
  // levels because its catalogue is the host's whole provider space (278 models
  // across 43 vendors on the machine this was built against), and a flat popover
  // of that is not a menu.
  const openFolder = dshFolder ? dshMenu.folders.find((f) => f.id === dshFolder) : undefined;
  const folderLabel = (vendor: string, count: number) =>
    vendor ? `${vendor} (${count})` : t("new_session.model_other_vendors", { count });
  const modelItems: PillMenuItem[] = openFolder
    ? [
        {
          id: "..",
          label: folderLabel(openFolder.vendor, openFolder.models.length),
          icon: <ChevronLeft size={13} strokeWidth={2.2} />,
          keepOpen: true,
          onSelect: () => setDshFolder(null),
        },
        ...openFolder.models.map((m) => ({
          id: m.value,
          label: m.label,
          checked: m.value === model,
          onSelect: () => onModelChange(m.value),
        })),
      ]
    : [
        {
          id: "",
          label: t("new_session.model_default"),
          checked: model === "",
          onSelect: () => onModelChange(""),
        },
        ...(isDsh ? dshMenu.inline : modelChoices).map((m) => ({
          id: m.value,
          label: m.label,
          checked: m.value === model,
          onSelect: () => onModelChange(m.value),
        })),
        ...dshMenu.folders.map((f) => ({
          id: f.id,
          label: folderLabel(f.vendor, f.models.length),
          icon: <ChevronRight size={13} strokeWidth={2.2} />,
          keepOpen: true,
          onSelect: () => setDshFolder(f.id),
        })),
      ];
  const permissionLabel = permissionMode
    ? t(`new_session.permission_${permissionMode}`)
    : t("new_session.permission_pill_default");
  // The dsh catalogue's fetch status, pinned above the model menu's rows. A
  // transport failure and the harness's own per-provider failures both show
  // here with one shared retry; a quiet "refreshing" note covers the in-flight
  // state. The last good catalogue stays listed under it, so a failed refetch
  // degrades to "stale + why", never to a bare "default".
  const dshStatusHeader = () => (
    <>
      {dshLoading && <div className={pillStyles.menu_note}>{t("new_session.model_loading")}</div>}
      {(dshError !== null || dshFailures.length > 0) && (
        <div className={pillStyles.menu_error}>
          <span className={pillStyles.menu_error_text}>
            {dshError !== null
              ? t("new_session.model_load_error", { message: dshError })
              : dshFailures
                  .map((f) =>
                    t("new_session.model_provider_failed", {
                      name: f.name,
                      message: f.message,
                    }),
                  )
                  .join("\n")}
          </span>
          <button
            type="button"
            className={pillStyles.menu_retry}
            disabled={dshLoading}
            onClick={() => loadDshCatalog()}
          >
            {t("new_session.model_retry")}
          </button>
        </div>
      )}
    </>
  );
  return (
    <>
      {onToolChange && toolChoices.length > 1 && (
        <PillMenu
          placement={placement}
          compact={compact}
          label={toolLabel}
          title={t("new_session.tool")}
          testId="agent-pill"
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
        testId="model-pill"
        disabled={disabled}
        items={modelItems}
        onOpen={isDsh ? () => loadDshCatalog() : undefined}
        menuHeader={isDsh ? dshStatusHeader : undefined}
      />
      <PillMenu
        placement={placement}
        compact={compact}
        label={effortLabel}
        title={t("new_session.effort")}
        testId="effort-pill"
        disabled={disabled}
        items={[
          {
            id: "",
            label: effortDefaultLabel,
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
      {/* Neither Codex nor dsh has a --permission-mode analogue: Codex's
          sandbox/approval mapping is a later milestone, and dsh models the same
          ground as a preset plus an approval policy that only its own RPC can
          switch (`setApprovalPolicy` and friends answer "not found" — probed
          live). So the permission pill is Claude-only. Hosts with no permission
          concept (e.g. schedule edit) hide it outright. */}
      {showPermission && !isCodex && !isDsh && (
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
