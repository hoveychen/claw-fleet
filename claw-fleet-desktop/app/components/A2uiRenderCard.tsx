import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { A2uiSurface, basicCatalog } from "@a2ui/react/v0_9";
import { MessageProcessor, SurfaceModel } from "@a2ui/web_core/v0_9";
import { useDecisionStore } from "../store";
import type { A2uiRenderDecision } from "../types";
import styles from "./DecisionPanel.module.css";

// Flatten the agent's `userAction.context` (arbitrary JSON values) into the
// `Record<String, String>` shape the Rust backend expects. Non-string values
// are JSON-stringified so structured input survives the wire as parseable
// text. Mirrors the fleet__ask answers map shape so agents that learned one
// can read both.
function flattenActionContext(ctx: Record<string, unknown>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(ctx ?? {})) {
    if (v === undefined || v === null) continue;
    out[k] = typeof v === "string" ? v : JSON.stringify(v);
  }
  return out;
}

export function A2uiRenderCard({ decision }: { decision: A2uiRenderDecision }) {
  const { t } = useTranslation();
  const submitA2uiRender = useDecisionStore((s) => s.submitA2uiRender);
  const cancelA2uiRender = useDecisionStore((s) => s.cancelA2uiRender);
  const setA2uiActionPayload = useDecisionStore((s) => s.setA2uiActionPayload);

  const processor = useMemo(
    () =>
      new MessageProcessor([basicCatalog], (action) => {
        setA2uiActionPayload(
          decision.id,
          action.name,
          flattenActionContext(action.context),
        );
      }),
    [decision.id, setA2uiActionPayload],
  );
  const [surface, setSurface] = useState<SurfaceModel | null>(null);

  useEffect(() => {
    try {
      const msg = decision.request.messageTree;
      // Accept both shapes A2UI v0.9 allows: bare array of messages or a
      // `{ messages: [...] }` wrapper. Otherwise wrap single object in array.
      const messages = Array.isArray(msg)
        ? msg
        : msg && typeof msg === "object" && "messages" in (msg as Record<string, unknown>)
          ? (msg as { messages: unknown[] }).messages
          : [msg];
      processor.processMessages(messages as never);
    } catch (e) {
      console.error("A2UI processMessages failed:", e);
    }
    const sync = () => {
      const first = Array.from(processor.model.surfacesMap.values())[0] ?? null;
      setSurface(first);
    };
    sync();
    const created = processor.onSurfaceCreated(sync);
    const deleted = processor.onSurfaceDeleted(sync);
    return () => {
      created.unsubscribe();
      deleted.unsubscribe();
    };
  }, [processor, decision.request.messageTree]);

  return (
    <div className={styles.card}>
      <div className={styles.card_header}>
        <svg
          className={styles.card_icon_question}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <line x1="9" y1="9" x2="15" y2="9" />
          <line x1="9" y1="13" x2="15" y2="13" />
          <line x1="9" y1="17" x2="13" y2="17" />
        </svg>
        <span className={styles.card_title}>
          {t("a2ui_render.title", "Agent UI (A2UI)")}
        </span>
        {decision.request.workspaceName && (
          <span className={styles.card_workspace}>{decision.request.workspaceName}</span>
        )}
      </div>

      {decision.request.aiTitle && (
        <div className={styles.card_subtitle}>{decision.request.aiTitle}</div>
      )}

      <div style={{ padding: "0.4rem 0" }}>
        {surface ? (
          // Upstream @a2ui/react@0.10.0 papercut: MessageProcessor's
          // SurfaceModel<ComponentApi<ZodTypeAny>> doesn't satisfy
          // A2uiSurface's SurfaceModel<ReactComponentImplementation>. Same
          // hits the README quickstart. Cast through `never` until upstream
          // aligns the generics — see memory:project_a2ui_evaluation.md.
          <A2uiSurface surface={surface as never} />
        ) : (
          <div style={{ opacity: 0.6, padding: "0.6rem 0" }}>
            {t("a2ui_render.waiting", "Rendering A2UI surface…")}
          </div>
        )}
      </div>

      <div className={styles.actions}>
        <button
          className={`${styles.btn} ${styles.btn_secondary}`}
          onClick={() => cancelA2uiRender(decision.id)}
          disabled={decision.submitting}
        >
          {t("a2ui_render.cancel", "Cancel")}
        </button>
        <div className={styles.actions_spacer} />
        <button
          className={`${styles.btn} ${styles.btn_allow}`}
          onClick={() => submitA2uiRender(decision.id)}
          disabled={decision.submitting}
        >
          {decision.submitting
            ? t("a2ui_render.submitting", "Submitting…")
            : decision.actionPayload
              ? t("a2ui_render.submit_with_action", "Submit ({{action}})", {
                  action: decision.actionPayload.name ?? "no-action",
                })
              : t("a2ui_render.submit_no_action", "Submit (no action yet)")}
        </button>
      </div>
    </div>
  );
}
