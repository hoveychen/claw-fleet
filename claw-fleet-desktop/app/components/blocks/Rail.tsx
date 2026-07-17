import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Bot,
  CircleCheck,
  CircleHelp,
  Clock,
  FileText,
  Globe,
  Image,
  ListTodo,
  MessageSquare,
  Pencil,
  Search,
  Terminal,
  Wrench,
} from "lucide-react";
import styles from "./Rail.module.css";

/**
 * The work-run timeline rail: each step of an expanded run (a thinking segment,
 * a tool call, a grouped read batch) gets a small icon in a left gutter, with a
 * thin connector line running between consecutive icons — the claude.ai work
 * stream look. `RailStep` wraps one step; `RailDone` is the terminal check row
 * a finished run closes with.
 */

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]);
const SHELL_TOOLS = new Set(["Bash", "exec", "exec_command", "write_stdin"]);
const WEB_TOOLS = new Set(["WebSearch", "WebFetch"]);
const SEARCH_TOOLS = new Set(["Grep", "Glob", "Explore", "LSP"]);
const AGENT_TOOLS = new Set(["Agent", "spawn_agent", "wait_agent"]);
const PLAN_TOOLS = new Set(["TodoWrite", "TodoRead", "update_plan"]);

/** Icon for a tool-call step, by tool name. Unknown tools (MCP, future) get a
 *  generic wrench rather than nothing, so the rail never has a hole. */
export function railToolIcon(name: string): ReactNode {
  if (SHELL_TOOLS.has(name)) return <Terminal />;
  if (EDIT_TOOLS.has(name)) return <Pencil />;
  if (name === "Read") return <FileText />;
  if (SEARCH_TOOLS.has(name)) return <Search />;
  if (WEB_TOOLS.has(name)) return <Globe />;
  if (AGENT_TOOLS.has(name)) return <Bot />;
  if (PLAN_TOOLS.has(name)) return <ListTodo />;
  return <Wrench />;
}

/** Icon for a grouped read-only batch: globe when the whole batch is web
 *  lookups, magnifier otherwise (mixed batches read as exploration). */
export function railGroupIcon(names: string[]): ReactNode {
  return names.every((n) => WEB_TOOLS.has(n)) ? <Globe /> : <Search />;
}

export const railThinkingIcon = (<Clock />);
export const railTextIcon = (<MessageSquare />);
export const railMediaIcon = (<Image />);
export const railDecisionIcon = (<CircleHelp />);
export const railUnknownIcon = (<Wrench />);

export function RailStep({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.step}>
      <span className={styles.icon} aria-hidden>
        {icon}
      </span>
      <div className={styles.body}>{children}</div>
    </div>
  );
}

/** Terminal row of a finished run — the claude.ai "Done" check. */
export function RailDone() {
  const { t } = useTranslation();
  return (
    <div className={`${styles.step} ${styles.done}`}>
      <span className={`${styles.icon} ${styles.done_icon}`} aria-hidden>
        <CircleCheck />
      </span>
      <div className={styles.done_label}>{t("detail.work_done")}</div>
    </div>
  );
}
