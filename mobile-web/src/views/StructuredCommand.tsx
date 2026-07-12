// Structured (AST) rendering for a guard card's command — the mobile port of
// the desktop's StructuredCommandView: one row per executable leaf, connector
// badges between rows, "audit" badge on triggering leaves, nested scripts
// (bash -c / eval / …) indented recursively.

import { Fragment } from "react";
import type { CmdConnector, CommandLeaf, CommandView, NestedScript } from "../types";
import styles from "./StructuredCommand.module.css";

const CONNECTOR_SYMBOL: Record<CmdConnector, string> = {
  and: "&&",
  or: "||",
  pipe: "|",
  semi: ";",
};

const NESTED_LABEL: Record<string, string> = {
  "bash-c": "bash -c 内嵌脚本",
  "sh-c": "sh -c 内嵌脚本",
  "zsh-c": "zsh -c 内嵌脚本",
  "python-c": "python -c 内嵌代码",
  "node-e": "node -e 内嵌代码",
  eval: "eval 内嵌脚本",
};

/** python/node payloads are opaque code, not shell — show them raw. */
function isOpaqueScript(kind: string): boolean {
  return kind === "python-c" || kind === "node-e";
}

export function StructuredCommand({
  command,
  view,
}: {
  command: string;
  view?: CommandView | null;
}) {
  if (!view || view.leaves.length === 0) {
    return <pre className={styles.fallback}>{command}</pre>;
  }
  return (
    <div className={styles.root}>
      <ViewBody view={view} />
    </div>
  );
}

function ViewBody({ view }: { view: CommandView }) {
  return (
    <>
      {view.leaves.map((leaf, i) => (
        <Fragment key={i}>
          {i > 0 && (
            <div className={styles.connectorRow}>
              <span className={styles.connector}>
                {CONNECTOR_SYMBOL[view.connectors[i - 1]] ?? ";"}
              </span>
            </div>
          )}
          <LeafRow leaf={leaf} />
        </Fragment>
      ))}
    </>
  );
}

function LeafRow({ leaf }: { leaf: CommandLeaf }) {
  const triggering = leaf.triggering === true;
  const covered = leaf.already_allowed === true;
  // When the leaf carries a nested script the last argv entry is the raw
  // script itself — the nested block below renders it, so drop it here.
  const visible = leaf.nested ? leaf.argv.slice(0, -1) : leaf.argv;
  return (
    <div className={styles.leaf} data-triggering={triggering}>
      <div className={styles.argv}>
        {visible.map((tok, j) => (
          <span key={j} className={j === 0 ? styles.binary : styles.arg}>
            {tok}
          </span>
        ))}
        {triggering && (
          <span className={styles.triggerBadge} data-covered={covered}>
            {covered ? "触发审计 · 已有规则" : "触发审计"}
          </span>
        )}
      </div>
      {leaf.nested && <NestedBlock nested={leaf.nested} />}
    </div>
  );
}

function NestedBlock({ nested }: { nested: NestedScript }) {
  return (
    <div className={styles.nested}>
      <div className={styles.nestedLabel}>{NESTED_LABEL[nested.kind] ?? "内嵌脚本"}</div>
      {isOpaqueScript(nested.kind) ? (
        <pre className={styles.script}>{nested.raw}</pre>
      ) : (
        <ViewBody view={nested.view} />
      )}
    </div>
  );
}
