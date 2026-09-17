import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";

import type { ErrorAction, SyntheticErrorInfo } from "../../../../shared-ts/syntheticError";
import type { ProcRecord } from "../../types";
import { modelChoicesFor } from "../../modelChoices";
import { useModelCatalog } from "../../useModelCatalog";
import { canResumeSession, resumeErrorText, resumeSession } from "../sessionResume";
import { ProcTerminal } from "../ProcTerminal";
import { ApiErrorBlock } from "./ApiErrorBlock";
import styles from "./ApiErrorBlock.module.css";

/** What a failed-turn card needs to know about the session it sits in. Passed
 *  down instead of the whole `SessionInfo` because `MessageList` deliberately
 *  does not take one — it renders transcripts from several sources, some of
 *  which have no live session behind them at all. */
export interface ApiErrorContext {
  sessionId: string;
  workspacePath: string;
  agentSource: string;
  isSubagent?: boolean;
  ideName?: string | null;
}

/**
 * The failed-turn card, with its buttons connected.
 *
 * Split from `ApiErrorBlock` so the card stays a pure render: everything here
 * has a side effect (a resume, a spawned login shell, a browser tab) and needs
 * the session context, while the card itself is reused by the phone, which
 * reaches the same backend through an entirely different transport.
 *
 * With no `ctx` — a transcript opened for a session Fleet cannot act on — this
 * degrades to the card alone. That is the honest rendering: the classification
 * is still worth showing, the buttons would be lies.
 */
export function ApiErrorActions({
  info,
  ctx,
}: {
  info: SyntheticErrorInfo;
  ctx?: ApiErrorContext | null;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<ErrorAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<ErrorAction | null>(null);
  const [picking, setPicking] = useState(false);
  const [loginProc, setLoginProc] = useState<ProcRecord | null>(null);
  const catalog = useModelCatalog();

  const resumable = ctx
    ? canResumeSession({
        isSubagent: ctx.isSubagent ?? false,
        ideName: ctx.ideName ?? null,
        agentSource: ctx.agentSource,
      })
    : false;

  const models = useMemo(
    () => modelChoicesFor(catalog, ctx?.agentSource === "codex" ? "codex" : "claude"),
    [catalog, ctx?.agentSource],
  );

  /** Run a resume, reporting whatever the backend said if it refused. */
  const fire = useCallback(
    async (action: ErrorAction, args: { prompt?: string; model?: string }) => {
      if (!ctx) return;
      setBusy(action);
      setError(null);
      try {
        await resumeSession({
          sessionId: ctx.sessionId,
          workspacePath: ctx.workspacePath,
          agentSource: ctx.agentSource,
          ...args,
        });
        setDone(action);
      } catch (e) {
        setError(resumeErrorText(e));
      } finally {
        setBusy(null);
      }
    },
    [ctx],
  );

  const onAction = useCallback(
    (action: ErrorAction) => {
      if (!ctx) return;
      switch (action) {
        case "retry":
          // No prompt: the resumed agent picks up the turn the failure cut off,
          // which is the same thing the rate-limit control and the auto-resume
          // scheduler do.
          void fire("retry", {});
          return;
        case "compact":
          void fire("compact", { prompt: "/compact" });
          return;
        case "switchModel":
          setPicking((v) => !v);
          return;
        case "openUrl":
          if (info.url) {
            void import("@tauri-apps/plugin-opener").then(({ openUrl }) =>
              openUrl(info.url!).catch((e: unknown) => setError(resumeErrorText(e))),
            );
          }
          return;
        case "login":
          // `claude auth login` is an OAuth handshake: it prints a URL and then
          // waits for a pasted code, so it needs a real pty and a place to type
          // — not a fire-and-forget spawn. Fleet already owns both (the
          // Terminal page's proc runner), so the card borrows them
          // inline rather than sending the user off to find a terminal.
          setBusy("login");
          setError(null);
          invoke<ProcRecord>("run_workspace_proc", {
            workspacePath: ctx.workspacePath,
            command: "claude auth login",
            cols: 80,
            rows: 24,
          })
            .then((rec) => setLoginProc(rec))
            .catch((e: unknown) => setError(resumeErrorText(e)))
            .finally(() => setBusy(null));
          return;
      }
    },
    [ctx, fire, info.url],
  );

  // A button that cannot work must not be drawn. `openUrl` and `login` stand on
  // their own; everything else routes through a resume, so they disappear for a
  // subagent / IDE-attached / non-resumable session rather than failing on click.
  const shown = ctx
    ? info.actions.filter((a) => resumable || a === "openUrl" || a === "login")
    : [];
  const view: SyntheticErrorInfo = shown.length === info.actions.length ? info : { ...info, actions: shown };

  return (
    <>
      <ApiErrorBlock
        info={view}
        onAction={ctx && shown.length > 0 ? onAction : undefined}
        busy={busy}
      />
      {picking && (
        <div className={styles.picker} data-testid="api-error-model-picker">
          <span className={styles.pickerLabel}>{t("detail.api_error.pick_model", "换成")}</span>
          {models.length === 0 ? (
            <span className={styles.note}>{t("detail.api_error.no_models", "模型列表加载中…")}</span>
          ) : (
            models.map((m) => (
              <button
                key={m.value}
                type="button"
                className={styles.pickerItem}
                data-model={m.value}
                disabled={busy != null}
                onClick={() => {
                  setPicking(false);
                  void fire("switchModel", { model: m.value });
                }}
              >
                {m.label}
              </button>
            ))
          )}
        </div>
      )}
      {loginProc && (
        <div className={styles.login} data-testid="api-error-login-terminal">
          <ProcTerminal
            proc={loginProc}
            height={220}
            onMissing={() => setLoginProc(null)}
            onRecord={(rec) => {
              // Authentication finished; the turn can be retried now, so say so
              // rather than leaving a dead shell on screen.
              if (rec.status === "exited") setLoginProc(null);
            }}
          />
        </div>
      )}
      {error && (
        <div className={styles.note} data-testid="api-error-failure" title={error}>
          {t("detail.api_error.failed", { defaultValue: "没能执行：{{err}}", err: error })}
        </div>
      )}
      {done && !error && (
        <div className={styles.note} data-testid="api-error-done">
          {t("detail.api_error.resumed", "已重新拉起，等会话接上…")}
        </div>
      )}
    </>
  );
}
