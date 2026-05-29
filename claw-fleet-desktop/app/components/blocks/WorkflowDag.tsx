import { useMemo } from "react";
import type { WorkflowTree, WorkflowNode } from "../../types";
import styles from "./WorkflowDag.module.css";

const NODE_W = 156;
const NODE_H = 66;
const H_GAP = 60;
const V_GAP = 22;
const PAD = 12;

interface Props {
  tree: WorkflowTree;
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
export function WorkflowDag({ tree }: Props) {
  const layout = useMemo(() => computeLayout(tree), [tree]);

  if (tree.nodes.length === 0) {
    // Unparseable script → honest flat fallback.
    return (
      <div className={styles.fallback}>
        <div className={styles.fallbackHint}>
          无法解析编排结构，按 agent 列表展示：
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
            className={`${styles.node} ${styles[`status_${node.status}`]}`}
            style={{ left: x + PAD, top: y + PAD, width: NODE_W, height: NODE_H }}
            title={nodeTitle(node)}
          >
            <div className={styles.nodeHead}>
              <span className={`${styles.kind} ${styles[`kind_${node.kind}`]}`}>
                {kindLabel(node.kind)}
              </span>
              {node.agentIds.length > 0 && (
                <span className={styles.count}>×{node.agentIds.length}</span>
              )}
              {node.approximate && (
                <span className={styles.approx} title="启发式绑定（近似）">
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
