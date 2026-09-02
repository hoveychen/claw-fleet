import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { WorkflowTree, WorkflowNode, WorkflowAgent } from "../../types";
import styles from "./WorkflowDag.module.css";

const NODE_W = 156;
const NODE_H = 66;
const H_GAP = 60;
const V_GAP = 22;
const PAD = 12;
const RESULT_PREVIEW = 360;

interface Props {
  tree: WorkflowTree;
  /** Open the session for a workflow agent (id = `agent-<agentId>`). When
   *  omitted, the "open session" affordance is hidden. */
  onOpenAgent?: (agentId: string) => void;
}

interface Placed {
  node: WorkflowNode;
  x: number;
  y: number;
}

/**
 * Layered DAG renderer for a Claude Code workflow run. Nodes (one per agent()
 * call-site) are placed left→right by longest-path layer; edges (pipeline
 * chains + phase progression) are drawn as SVG bezier curves. Live status comes
 * from the node's bound runtime agents (pending / running / done).
 *
 * Falls back to a flat agent list when the script could not be parsed into a
 * DAG (no nodes).
 */
export function WorkflowDag({ tree, onOpenAgent }: Props) {
  const { t } = useTranslation();
  const layout = useMemo(() => computeLayout(tree), [tree]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const agentById = useMemo(
    () => new Map(tree.agents.map((a) => [a.agentId, a])),
    [tree],
  );
  const selectedNode = selectedId
    ? tree.nodes.find((n) => n.id === selectedId)
    : undefined;

  if (tree.nodes.length === 0) {
    // Unparseable script → honest flat fallback.
    return (
      <div className={styles.fallback}>
        <div className={styles.fallbackHint}>
          {t("workflow.dag_unparsed", "无法解析编排结构，按 agent 列表展示：")}
        </div>
        <div className={styles.chips}>
          {tree.agents.map((a) => (
            <span
              key={a.agentId}
              className={`${styles.chip} ${styles[`s_${a.status}`]}`}
              title={a.agentType ?? undefined}
            >
              {a.agentType ?? a.agentId.slice(0, 8)}
            </span>
          ))}
        </div>
      </div>
    );
  }

  const { placed, width, height, byId } = layout;

  return (
    <div className={styles.wrap}>
      <div
        className={styles.canvas}
        style={{ width: width + PAD * 2, height: height + PAD * 2 }}
      >
        <svg
          className={styles.edges}
          width={width + PAD * 2}
          height={height + PAD * 2}
        >
          <defs>
            <marker
              id="wfdag-arrow"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="7"
              markerHeight="7"
              orient="auto-start-reverse"
            >
              <path d="M0,0 L8,4 L0,8 z" className={styles.arrowHead} />
            </marker>
          </defs>
          {tree.edges.map((e, i) => {
            const a = byId.get(e.from);
            const b = byId.get(e.to);
            if (!a || !b) return null;
            const x1 = a.x + NODE_W + PAD;
            const y1 = a.y + NODE_H / 2 + PAD;
            const x2 = b.x + PAD;
            const y2 = b.y + NODE_H / 2 + PAD;
            const mx = (x1 + x2) / 2;
            return (
              <path
                key={i}
                d={`M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`}
                className={styles.edge}
                markerEnd="url(#wfdag-arrow)"
              />
            );
          })}
        </svg>
        {placed.map(({ node, x, y }) => (
          <div
            key={node.id}
            role="button"
            tabIndex={0}
            className={`${styles.node} ${styles[`status_${node.status}`]} ${selectedId === node.id ? styles.selected : ""}`}
            style={{ left: x + PAD, top: y + PAD, width: NODE_W, height: NODE_H }}
            title={nodeTitle(node)}
            onClick={() =>
              setSelectedId((cur) => (cur === node.id ? null : node.id))
            }
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setSelectedId((cur) => (cur === node.id ? null : node.id));
              }
            }}
          >
            <div className={styles.nodeHead}>
              <span className={`${styles.kind} ${styles[`kind_${node.kind}`]}`}>
                {kindLabel(node.kind)}
              </span>
              {node.agentIds.length > 0 && (
                <span className={styles.count}>×{node.agentIds.length}</span>
              )}
              {node.approximate && (
                <span className={styles.approx} title={t("workflow.dag_approx_tip", "启发式绑定（近似）")}>
                  ≈
                </span>
              )}
            </div>
            <div className={styles.label}>{node.label}</div>
            {node.phase && <div className={styles.phase}>{node.phase}</div>}
            <span className={styles.dot} />
          </div>
        ))}
      </div>

      {selectedNode && (
        <div className={styles.detail}>
          <div className={styles.detailHead}>
            <span className={styles.detailTitle}>
              {selectedNode.label}
            </span>
            <span className={styles.detailMeta}>
              {kindLabel(selectedNode.kind)} · {selectedNode.status} ·{" "}
              {selectedNode.agentIds.length} agent
              {selectedNode.approximate ? ` · ≈ ${t("workflow.dag_approx", "启发式绑定")}` : ""}
            </span>
            <button
              className={styles.detailClose}
              onClick={() => setSelectedId(null)}
              title={t("workflow.dag_collapse", "收起")}
            >
              ✕
            </button>
          </div>
          {selectedNode.resolvedPrompt && (
            <div className={styles.resolvedPrompt} title={t("workflow.dag_resolved_prompt_tip", "执行时解析出的 prompt（… 为插值点）")}>
              {selectedNode.resolvedPrompt}
            </div>
          )}
          {selectedNode.agentIds.length === 0 ? (
            <div className={styles.detailEmpty}>
              {t("workflow.dag_no_runtime_agent", "尚无运行时 agent（未启动）")}
            </div>
          ) : (
            <div className={styles.agentRows}>
              {selectedNode.agentIds.map((id) => (
                <AgentRow
                  key={id}
                  agentId={id}
                  agent={agentById.get(id)}
                  onOpen={onOpenAgent}
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function AgentRow({
  agentId,
  agent,
  onOpen,
}: {
  agentId: string;
  agent: WorkflowAgent | undefined;
  onOpen?: (agentId: string) => void;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const result = agent?.result ?? "";
  const long = result.length > RESULT_PREVIEW;
  const shown = expanded || !long ? result : result.slice(0, RESULT_PREVIEW) + "…";
  const status = agent?.status ?? "running";
  return (
    <div className={styles.agentRow}>
      <div className={styles.agentRowHead}>
        <span className={`${styles.agentDot} ${styles[`st_${status}`]}`} />
        <span className={styles.agentType}>
          {agent?.agentType ?? "agent"}
        </span>
        <span className={styles.agentId}>{agentId.slice(0, 10)}</span>
        <span className={`${styles.agentStatus} ${styles[`st_${status}`]}`}>
          {status}
        </span>
        {onOpen && (
          <button
            className={styles.openBtn}
            onClick={() => onOpen(agentId)}
            title={t("workflow.dag_open_agent_session", "在 Fleet 里打开该 agent 的 session")}
          >
            {t("workflow.dag_open_session", "打开 session")} ↗
          </button>
        )}
      </div>
      {result ? (
        <pre
          className={styles.result}
          onClick={() => long && setExpanded((v) => !v)}
          style={{ cursor: long ? "pointer" : "default" }}
          title={
            long
              ? expanded
                ? t("workflow.dag_click_collapse", "点击收起")
                : t("workflow.dag_click_expand", "点击展开")
              : undefined
          }
        >
          {shown}
        </pre>
      ) : (
        <div className={styles.detailEmpty}>
          {status === "done"
            ? t("workflow.dag_no_result_text", "（无结果文本）")
            : t("workflow.dag_running_no_result", "运行中，暂无结果")}
        </div>
      )}
    </div>
  );
}

function kindLabel(k: WorkflowNode["kind"]): string {
  switch (k) {
    case "parallel":
      return "∥ parallel";
    case "pipeline":
      return "→ pipeline";
    default:
      return "• single";
  }
}

function nodeTitle(n: WorkflowNode): string {
  const parts = [
    `${n.label} (${n.kind})`,
    `status: ${n.status}`,
    `agents: ${n.agentIds.length}`,
  ];
  if (n.approximate) parts.push("binding: approximate (heuristic)");
  if (n.resolvedPrompt) parts.push("", n.resolvedPrompt);
  return parts.join("\n");
}

function computeLayout(tree: WorkflowTree) {
  const nodes = tree.nodes;
  const byNodeId = new Map(nodes.map((n) => [n.id, n]));

  // longest-path layer assignment over the DAG edges
  const layer: Record<string, number> = {};
  nodes.forEach((n) => (layer[n.id] = 0));
  for (let pass = 0; pass < nodes.length; pass++) {
    let changed = false;
    for (const e of tree.edges) {
      if (!byNodeId.has(e.from) || !byNodeId.has(e.to)) continue;
      const cand = layer[e.from] + 1;
      if (cand > layer[e.to]) {
        layer[e.to] = cand;
        changed = true;
      }
    }
    if (!changed) break;
  }

  // group by layer, preserving original node order within a layer
  const rows: Record<number, number> = {};
  const placed: Placed[] = nodes.map((node) => {
    const l = layer[node.id] ?? 0;
    const r = rows[l] ?? 0;
    rows[l] = r + 1;
    return {
      node,
      x: l * (NODE_W + H_GAP),
      y: r * (NODE_H + V_GAP),
    };
  });

  const byId = new Map(placed.map((p) => [p.node.id, p]));
  const maxLayer = Math.max(0, ...placed.map((p) => p.x / (NODE_W + H_GAP)));
  const maxRows = Math.max(1, ...Object.values(rows));
  const width = (maxLayer + 1) * NODE_W + maxLayer * H_GAP;
  const height = maxRows * NODE_H + (maxRows - 1) * V_GAP;

  return { placed, byId, width, height };
}
