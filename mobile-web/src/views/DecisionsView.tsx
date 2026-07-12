import type { RelayClient } from "../relay";
import type { PendingDecision, SessionInfo } from "../types";
import styles from "./DecisionsView.module.css";

interface Props {
  decisions: PendingDecision[];
  client: RelayClient | null;
  agentOnline: boolean;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
}

export function DecisionsView({ decisions }: Props) {
  if (decisions.length === 0) {
    return <div className={styles.empty}>没有待处理的决策 🎉</div>;
  }
  return (
    <div className={styles.list}>
      {decisions.map((d) => (
        <div key={d.id} className={styles.card}>
          {d.kind}: {d.id}
        </div>
      ))}
    </div>
  );
}
