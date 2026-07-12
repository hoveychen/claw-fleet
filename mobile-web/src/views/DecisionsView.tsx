import { useCallback, useEffect, useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { RelayClient } from "../relay";
import type {
  ElicitationQuestion,
  FleetAskFormField,
  FleetAskQuestion,
  FleetAskRequest,
  GuardRequest,
  PendingDecision,
  PermissionPromptRequest,
  PlanApprovalRequest,
  SessionInfo,
} from "../types";
import styles from "./DecisionsView.module.css";

const KIND_LABEL: Record<string, string> = {
  guard: "命令审批",
  elicitation: "问题请示",
  "fleet-ask": "决策卡",
  "plan-approval": "计划审批",
  "permission-prompt": "权限请求",
};

interface Props {
  decisions: PendingDecision[];
  client: RelayClient | null;
  agentOnline: boolean;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
}

export function DecisionsView({ decisions, client, agentOnline, workspaceOf, onAnswered }: Props) {
  if (decisions.length === 0) {
    return (
      <div className={styles.empty}>
        {agentOnline ? "没有待处理的决策 🎉" : "桌面端离线，暂时收不到新决策"}
      </div>
    );
  }
  const sorted = [...decisions].sort((a, b) => a.arrivedAt - b.arrivedAt);
  return (
    <div className={styles.list}>
      {sorted.map((d) => (
        <DecisionCard
          key={d.id}
          decision={d}
          client={client}
          workspaceOf={workspaceOf}
          onAnswered={onAnswered}
        />
      ))}
    </div>
  );
}

interface CardProps {
  decision: PendingDecision;
  client: RelayClient | null;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
}

function DecisionCard({ decision, client, workspaceOf, onAnswered }: CardProps) {
  const req = decision.request;
  const session = workspaceOf(req.sessionId);
  const workspace = req.workspaceName || session?.workspaceName || "Fleet";
  const aiTitle = req.aiTitle || session?.aiTitle;

  const submit = useCallback(
    (fields: Record<string, unknown>) => {
      if (!client) return;
      if (client.answer(decision.kind, decision.id, fields)) {
        onAnswered(decision.id);
      }
    },
    [client, decision.kind, decision.id, onAnswered],
  );

  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        <span className={styles.kindChip} data-kind={decision.kind}>
          {KIND_LABEL[decision.kind] ?? decision.kind}
        </span>
        <span className={styles.workspace}>{workspace}</span>
      </div>
      {aiTitle && <div className={styles.aiTitle}>{aiTitle}</div>}
      {decision.kind === "guard" && <GuardCard request={req as GuardRequest} submit={submit} />}
      {decision.kind === "permission-prompt" && (
        <PermissionCard request={req as PermissionPromptRequest} submit={submit} />
      )}
      {decision.kind === "plan-approval" && (
        <PlanCard request={req as PlanApprovalRequest} submit={submit} />
      )}
      {(decision.kind === "elicitation" || decision.kind === "fleet-ask") && (
        <QuestionsCard
          request={req as FleetAskRequest}
          isFleetAsk={decision.kind === "fleet-ask"}
          client={client}
          submit={submit}
        />
      )}
    </div>
  );
}

// ── guard ────────────────────────────────────────────────────────────────────

function GuardCard({ request, submit }: { request: GuardRequest; submit: (f: Record<string, unknown>) => void }) {
  const [reason, setReason] = useState("");
  const [showReason, setShowReason] = useState(false);
  return (
    <div>
      <pre className={styles.command}>{request.command}</pre>
      {request.riskTags.length > 0 && (
        <div className={styles.chipRow}>
          {request.riskTags.map((t) => (
            <span key={t} className={styles.riskChip}>
              {t}
            </span>
          ))}
        </div>
      )}
      {showReason && (
        <textarea
          className={styles.reasonInput}
          placeholder="拒绝理由（可选，会转告给 AI）"
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          rows={2}
        />
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!showReason) {
              setShowReason(true);
              return;
            }
            submit({ allow: false, reason: reason || undefined });
          }}
        >
          {showReason ? "确认拒绝" : "拒绝"}
        </button>
        <button className={styles.primaryButton} onClick={() => submit({ allow: true })}>
          允许
        </button>
      </div>
    </div>
  );
}

// ── permission-prompt ────────────────────────────────────────────────────────

function PermissionCard({
  request,
  submit,
}: {
  request: PermissionPromptRequest;
  submit: (f: Record<string, unknown>) => void;
}) {
  const [reason, setReason] = useState("");
  const [showReason, setShowReason] = useState(false);
  const input = useMemo(() => {
    try {
      return JSON.stringify(request.toolInput, null, 2);
    } catch {
      return String(request.toolInput);
    }
  }, [request.toolInput]);
  return (
    <div>
      <div className={styles.toolName}>
        工具：<code>{request.toolName}</code>
      </div>
      {input && input !== "null" && <pre className={styles.command}>{truncate(input, 800)}</pre>}
      {showReason && (
        <textarea
          className={styles.reasonInput}
          placeholder="拒绝理由（可选）"
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          rows={2}
        />
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!showReason) {
              setShowReason(true);
              return;
            }
            submit({ allow: false, reason: reason || undefined });
          }}
        >
          {showReason ? "确认拒绝" : "拒绝"}
        </button>
        <button className={styles.primaryButton} onClick={() => submit({ allow: true })}>
          允许
        </button>
      </div>
    </div>
  );
}

// ── plan-approval ────────────────────────────────────────────────────────────

function PlanCard({
  request,
  submit,
}: {
  request: PlanApprovalRequest;
  submit: (f: Record<string, unknown>) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [rejecting, setRejecting] = useState(false);
  const [feedback, setFeedback] = useState("");
  const long = request.planContent.length > 600;
  const content = expanded || !long ? request.planContent : request.planContent.slice(0, 600);
  return (
    <div>
      <div className={styles.markdown}>
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
        {long && (
          <button className={styles.expandButton} onClick={() => setExpanded((v) => !v)}>
            {expanded ? "收起" : "展开完整计划"}
          </button>
        )}
      </div>
      {rejecting && (
        <textarea
          className={styles.reasonInput}
          placeholder="驳回意见（会转告给 AI 修改计划）"
          value={feedback}
          onChange={(e) => setFeedback(e.target.value)}
          rows={3}
        />
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!rejecting) {
              setRejecting(true);
              return;
            }
            submit({ decision: "reject", feedback: feedback || undefined });
          }}
        >
          {rejecting ? "确认驳回" : "驳回"}
        </button>
        <button className={styles.primaryButton} onClick={() => submit({ decision: "approve" })}>
          批准
        </button>
      </div>
    </div>
  );
}

// ── elicitation / fleet-ask ──────────────────────────────────────────────────

const OTHER = "__other__";

function QuestionsCard({
  request,
  isFleetAsk,
  client,
  submit,
}: {
  request: FleetAskRequest;
  isFleetAsk: boolean;
  client: RelayClient | null;
  submit: (f: Record<string, unknown>) => void;
}) {
  // question text → selected labels (OTHER = custom text active)
  const [selections, setSelections] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  const [form, setForm] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {};
    for (const q of request.questions) {
      for (const f of q.formFields ?? []) {
        if (f.default !== undefined && f.default !== null) init[f.name] = String(f.default);
        else if (f.kind === "checkbox") init[f.name] = "false";
        else if (f.kind === "range") init[f.name] = String(f.min ?? 0);
      }
    }
    return init;
  });
  const [error, setError] = useState<string | null>(null);

  const toggle = (q: ElicitationQuestion | FleetAskQuestion, label: string) => {
    setSelections((prev) => {
      const cur = prev[q.question] ?? [];
      if (q.multiSelect) {
        return {
          ...prev,
          [q.question]: cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label],
        };
      }
      return { ...prev, [q.question]: cur.includes(label) ? [] : [label] };
    });
  };

  const doSubmit = (declined: boolean) => {
    if (declined) {
      submit(isFleetAsk ? { cancelled: true, answers: {} } : { declined: true, answers: {} });
      return;
    }
    const answers: Record<string, string> = {};
    for (const q of request.questions) {
      const picked = (selections[q.question] ?? []).filter((l) => l !== OTHER);
      const customText = (selections[q.question] ?? []).includes(OTHER)
        ? (custom[q.question] ?? "").trim()
        : "";
      const parts = [...picked];
      if (customText) parts.push(customText);
      const hasOptions = (q.options?.length ?? 0) > 0;
      if (hasOptions && parts.length === 0) {
        setError(`「${q.header || truncate(q.question, 20)}」还没有作答`);
        return;
      }
      if (parts.length > 0) answers[q.question] = parts.join(", ");
    }
    for (const q of request.questions) {
      for (const f of q.formFields ?? []) {
        const v = form[f.name] ?? "";
        if (f.required && !v.trim()) {
          setError(`「${f.label}」是必填项`);
          return;
        }
        answers[f.name] = v;
      }
    }
    submit(isFleetAsk ? { cancelled: false, answers } : { declined: false, answers });
  };

  return (
    <div>
      {request.questions.map((q, qi) => (
        <div key={qi} className={styles.question}>
          {q.header && <div className={styles.questionHeader}>{q.header}</div>}
          <div className={styles.markdown}>
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{stripTtsDivider(q.question)}</ReactMarkdown>
          </div>
          {isFleetAsk && (q as FleetAskQuestion).html && (
            <HtmlPreview
              html={(q as FleetAskQuestion).html!}
              images={(q as FleetAskQuestion).images}
              requestId={request.id}
              qidx={qi}
              client={client}
            />
          )}
          {isFleetAsk && !(q as FleetAskQuestion).html && (q as FleetAskQuestion).images?.length ? (
            <ImageGallery
              images={(q as FleetAskQuestion).images!}
              requestId={request.id}
              qidx={qi}
              client={client}
            />
          ) : null}
          {(q.options ?? []).map((o) => {
            const selected = (selections[q.question] ?? []).includes(o.label);
            return (
              <button
                key={o.label}
                className={styles.option}
                data-selected={selected}
                onClick={() => toggle(q, o.label)}
              >
                <span className={styles.optionMark} data-multi={q.multiSelect}>
                  {selected ? "✓" : ""}
                </span>
                <span className={styles.optionBody}>
                  <span className={styles.optionLabel}>{o.label}</span>
                  {o.description && <span className={styles.optionDesc}>{o.description}</span>}
                </span>
              </button>
            );
          })}
          {(q.options?.length ?? 0) > 0 && (
            <>
              <button
                className={styles.option}
                data-selected={(selections[q.question] ?? []).includes(OTHER)}
                onClick={() => toggle(q, OTHER)}
              >
                <span className={styles.optionMark} data-multi={q.multiSelect}>
                  {(selections[q.question] ?? []).includes(OTHER) ? "✓" : ""}
                </span>
                <span className={styles.optionBody}>
                  <span className={styles.optionLabel}>其他…</span>
                </span>
              </button>
              {(selections[q.question] ?? []).includes(OTHER) && (
                <textarea
                  className={styles.reasonInput}
                  placeholder="自定义回答"
                  value={custom[q.question] ?? ""}
                  onChange={(e) => setCustom((p) => ({ ...p, [q.question]: e.target.value }))}
                  rows={2}
                />
              )}
            </>
          )}
          {(q.formFields ?? []).map((f) => (
            <FormFieldControl
              key={f.name}
              field={f}
              value={form[f.name] ?? ""}
              onChange={(v) => setForm((p) => ({ ...p, [f.name]: v }))}
            />
          ))}
        </div>
      ))}
      {error && <div className={styles.error}>{error}</div>}
      <div className={styles.actions}>
        <button className={styles.ghostButton} onClick={() => doSubmit(true)}>
          {isFleetAsk ? "取消" : "拒绝回答"}
        </button>
        <button className={styles.primaryButton} onClick={() => doSubmit(false)}>
          提交
        </button>
      </div>
    </div>
  );
}

function FormFieldControl({
  field,
  value,
  onChange,
}: {
  field: FleetAskFormField;
  value: string;
  onChange: (v: string) => void;
}) {
  const label = (
    <div className={styles.fieldLabel}>
      {field.label}
      {field.required && <span className={styles.required}>*</span>}
    </div>
  );
  switch (field.kind) {
    case "textarea":
      return (
        <div className={styles.field}>
          {label}
          <textarea
            rows={3}
            placeholder={field.placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    case "select":
      return (
        <div className={styles.field}>
          {label}
          <select value={value} onChange={(e) => onChange(e.target.value)}>
            <option value="">请选择…</option>
            {(field.options ?? []).map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        </div>
      );
    case "radio":
      return (
        <div className={styles.field}>
          {label}
          <div className={styles.radioRow}>
            {(field.options ?? []).map((o) => (
              <button
                key={o}
                className={styles.radioChip}
                data-selected={value === o}
                onClick={() => onChange(o)}
              >
                {o}
              </button>
            ))}
          </div>
        </div>
      );
    case "checkbox":
      return (
        <div className={styles.field}>
          <button
            className={styles.checkboxRow}
            onClick={() => onChange(value === "true" ? "false" : "true")}
          >
            <span className={styles.optionMark} data-multi={true}>
              {value === "true" ? "✓" : ""}
            </span>
            {field.label}
          </button>
        </div>
      );
    case "range":
      return (
        <div className={styles.field}>
          {label}
          <div className={styles.rangeRow}>
            <input
              type="range"
              min={field.min ?? 0}
              max={field.max ?? 100}
              step={field.step ?? 1}
              value={value || String(field.min ?? 0)}
              onChange={(e) => onChange(e.target.value)}
            />
            <span className={styles.rangeValue}>{value || String(field.min ?? 0)}</span>
          </div>
        </div>
      );
    default: {
      const type =
        field.kind === "datetime" ? "datetime-local" : field.kind === "number" ? "number" : field.kind;
      return (
        <div className={styles.field}>
          {label}
          <input
            type={type}
            placeholder={field.placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    }
  }
}

// ── fleet-ask html / images ──────────────────────────────────────────────────

interface AssetReply {
  mime: string;
  base64: string;
}

function useAssets(
  names: string[],
  requestId: string,
  qidx: number,
  client: RelayClient | null,
): Record<string, string> {
  const [uris, setUris] = useState<Record<string, string>>({});
  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    for (const name of names) {
      client
        .request<AssetReply>("decision_asset", { id: requestId, qidx: `q${qidx}`, rel: name })
        .then((a) => {
          if (!cancelled) {
            setUris((p) => ({ ...p, [name]: `data:${a.mime};base64,${a.base64}` }));
          }
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestId, qidx, client, names.join("|")]);
  return uris;
}

function HtmlPreview({
  html,
  images,
  requestId,
  qidx,
  client,
}: {
  html: string;
  images: { name: string }[] | undefined;
  requestId: string;
  qidx: number;
  client: RelayClient | null;
}) {
  const names = useMemo(() => (images ?? []).map((i) => i.name), [images]);
  const uris = useAssets(names, requestId, qidx, client);
  const resolved = useMemo(() => {
    let out = html;
    for (const [name, uri] of Object.entries(uris)) {
      out = out.split(`src="${name}"`).join(`src="${uri}"`);
    }
    return out;
  }, [html, uris]);
  return (
    <iframe
      className={styles.htmlPreview}
      sandbox=""
      srcDoc={`<style>body{margin:8px;font-family:-apple-system,sans-serif;color:#e6e6e6;background:#101113;font-size:14px}img{max-width:100%}</style>${resolved}`}
      title="预览"
    />
  );
}

function ImageGallery({
  images,
  requestId,
  qidx,
  client,
}: {
  images: { name: string; caption?: string }[];
  requestId: string;
  qidx: number;
  client: RelayClient | null;
}) {
  const names = useMemo(() => images.map((i) => i.name), [images]);
  const uris = useAssets(names, requestId, qidx, client);
  return (
    <div className={styles.gallery}>
      {images.map((img) => (
        <figure key={img.name}>
          {uris[img.name] ? (
            <img src={uris[img.name]} alt={img.caption ?? img.name} />
          ) : (
            <div className={styles.imgLoading}>加载图片…</div>
          )}
          {img.caption && <figcaption>{img.caption}</figcaption>}
        </figure>
      ))}
    </div>
  );
}

// ── helpers ──────────────────────────────────────────────────────────────────

/** Drop the Fleet TTS divider (a lone `---` line splits summary/body). */
function stripTtsDivider(text: string): string {
  const idx = text.split("\n").findIndex((l) => l.trim() === "---");
  if (idx === -1) return text;
  const lines = text.split("\n");
  return [...lines.slice(0, idx), ...lines.slice(idx + 1)].join("\n");
}

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n) + "…" : s;
}
