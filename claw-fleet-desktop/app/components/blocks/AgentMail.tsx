import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { ContentBlock } from "../../types";
import { useSessionsStore, useUIStore } from "../../store";
import { mentionTitleOf } from "../../../../shared-ts/sessionMentions";
import {
  splitAgentMail,
  type AgentMailPart,
  type AgentMailSender,
} from "../../../../shared-ts/agentMail";
import { UserText } from "./UserContent";
import styles from "./AgentMail.module.css";

/** The mail parts of a user turn, or `null` when it is an ordinary prompt.
 *  Only a turn whose text is entirely mail-shaped qualifies — images or
 *  tool results alongside it mean a person was involved. */
export function agentMailOf(content: ContentBlock[] | string): AgentMailPart[] | null {
  if (typeof content === "string") return splitAgentMail(content);
  if (!Array.isArray(content)) return null;
  const texts = content.filter((b) => b.type === "text");
  if (texts.length === 0 || texts.length !== content.length) return null;
  return splitAgentMail(texts.map((b) => (b as { text: string }).text).join("\n\n"));
}

function SenderChip({ from }: { from: AgentMailSender }) {
  const { t } = useTranslation();
  const row = useSessionsStore((s) => s.sessions.find((x) => x.id === from.id));
  const title = (row && mentionTitleOf(row)) || from.id.slice(0, 8);
  const workspace = row?.workspaceName || from.workspace;
  return (
    <>
      {row ? (
        <button
          type="button"
          className={styles.sender}
          title={t("detail.open_mentioned_session", "打开这个任务")}
          onClick={() => useUIStore.getState().requestOpenTask(from.id)}
        >
          @{title}
        </button>
      ) : (
        <span className={`${styles.sender} ${styles.sender_gone}`} title={from.id}>
          @{title}
        </span>
      )}
      {workspace && <span className={styles.workspace}>· {workspace}</span>}
    </>
  );
}

interface Props {
  parts: AgentMailPart[];
  renderText: (text: string, key: number) => ReactNode;
  /** Timestamp / pending marker, rendered under the last part. */
  footer?: ReactNode;
}

/**
 * A turn another Fleet session sent this one, drawn as incoming mail on the
 * left instead of a right-hand bubble — the right side means "the user said
 * this", and these were not typed by the user. The reply-address footer the
 * receiving model needs is dropped from view; its sender becomes a chip.
 */
export function AgentMail({ parts, renderText, footer }: Props) {
  const { t } = useTranslation();
  return (
    <div className={styles.root}>
      {parts.map((p, i) => (
        <div key={i} className={styles.part}>
          {p.from && (
            <div className={styles.header}>
              <span className={styles.icon} aria-hidden>
                ↪
              </span>
              <span
                className={styles.from_label}
                title={t("detail.agent_mail_hint", "另一个任务发给它的消息，不是你输入的")}
              >
                {t("detail.agent_mail_from", "来自")}
              </span>
              <SenderChip from={p.from} />
            </div>
          )}
          <UserText text={p.body} index={i} renderText={renderText} />
        </div>
      ))}
      {footer && <div className={styles.footer}>{footer}</div>}
    </div>
  );
}
