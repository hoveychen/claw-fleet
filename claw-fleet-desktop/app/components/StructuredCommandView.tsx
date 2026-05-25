import { Fragment } from "react";
import { useTranslation } from "react-i18next";
import type {
  CommandLeaf,
  CommandView,
  Connector,
  NestedKind,
  NestedScript,
} from "../types";
import styles from "./StructuredCommandView.module.css";

const CONNECTOR_SYMBOL: Record<Connector, string> = {
  and: "&&",
  or: "||",
  pipe: "|",
  semi: ";",
};

const NESTED_LABEL_KEY: Record<NestedKind, string> = {
  "bash-c": "command.nested.bash",
  "sh-c": "command.nested.sh",
  "zsh-c": "command.nested.zsh",
  "python-c": "command.nested.python",
  "node-e": "command.nested.node",
  eval: "command.nested.eval",
};

const NESTED_LABEL_FALLBACK: Record<NestedKind, string> = {
  "bash-c": "bash -c",
  "sh-c": "sh -c",
  "zsh-c": "zsh -c",
  "python-c": "python -c",
  "node-e": "node -e",
  eval: "eval",
};

function isOpaqueScript(kind: NestedKind): boolean {
  return kind === "python-c" || kind === "node-e";
}

interface Props {
  /** The raw command, shown as a fallback when `view` is absent. */
  command: string;
  /** Structured AST view, supplied by newer backends. */
  view?: CommandView | null;
}

export function StructuredCommandView({ command, view }: Props) {
  if (!view || view.leaves.length === 0) {
    return <div className={styles.fallback}>{command}</div>;
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
          {i > 0 && <ConnectorBadge connector={view.connectors[i - 1]} />}
          <LeafRow leaf={leaf} />
        </Fragment>
      ))}
    </>
  );
}

function LeafRow({ leaf }: { leaf: CommandLeaf }) {
  const { t } = useTranslation();
  const triggering = leaf.triggering === true;
  const alreadyAllowed = leaf.already_allowed === true;
  const className = triggering
    ? `${styles.leaf} ${styles.leaf_triggering}`
    : styles.leaf;
  return (
    <div className={className}>
      <ArgvLine argv={leaf.argv} hasNested={!!leaf.nested} />
      {triggering && (
        <span
          className={
            alreadyAllowed
              ? `${styles.trigger_badge} ${styles.trigger_badge_allowed}`
              : styles.trigger_badge
          }
          title={
            alreadyAllowed
              ? t(
                  "guard.triggered_badge_allowed_hint",
                  "This command is what tripped the audit, but your existing allow rule already covers it.",
                )
              : t(
                  "guard.triggered_badge_hint",
                  "This command is what tripped the audit.",
                )
          }
        >
          {alreadyAllowed
            ? t("guard.triggered_badge_allowed", "audit · already allowed")
            : t("guard.triggered_badge", "audit")}
        </span>
      )}
      {leaf.nested && <NestedBlock nested={leaf.nested} />}
    </div>
  );
}

function ArgvLine({
  argv,
  hasNested,
}: {
  argv: string[];
  hasNested: boolean;
}) {
  if (argv.length === 0) return null;
  // When this leaf has a nested script (e.g. bash -c "...") the last argv
  // entry is the raw script, which we don't want to repeat above the nested
  // block.  Drop it from the inline display.
  const visible = hasNested ? argv.slice(0, -1) : argv;
  return (
    <div className={styles.argv}>
      {visible.map((tok, j) => (
        <span
          key={j}
          className={j === 0 ? styles.binary : styles.arg}
        >
          {tok}
        </span>
      ))}
    </div>
  );
}

function NestedBlock({ nested }: { nested: NestedScript }) {
  const { t } = useTranslation();
  const label = t(NESTED_LABEL_KEY[nested.kind], NESTED_LABEL_FALLBACK[nested.kind]);
  return (
    <div className={styles.nested}>
      <div className={styles.nested_label}>{label}</div>
      <div className={styles.nested_body}>
        {isOpaqueScript(nested.kind) ? (
          <pre className={styles.script_block}>{nested.raw}</pre>
        ) : (
          <ViewBody view={nested.view} />
        )}
      </div>
    </div>
  );
}

function ConnectorBadge({ connector }: { connector: Connector }) {
  return (
    <div className={styles.connector_row}>
      <span className={styles.connector}>{CONNECTOR_SYMBOL[connector]}</span>
    </div>
  );
}
