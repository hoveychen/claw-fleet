import { useTranslation } from "react-i18next";

import type { PathLinkContext } from "../markdown/pathLinks";
import { useAgentTranscript } from "../useAgentTranscript";
import { isLiveMember, type SessionInfo } from "../types";
import { MessageList } from "./MessageList";
import styles from "./SessionDetail.module.css";

/**
 * The reader for a subagent card — its transcript, in the rail, in place.
 *
 * A subagent has exactly one thing worth showing: what it is saying. It has no
 * composer, no plan, no decision history of its own worth a page. So clicking
 * its card no longer *navigates* to a session view (which cost you the
 * conversation you were reading, plus a trip back); it expands the same
 * in-place reader a file gets, with the messages in it.
 *
 * The transcript is fetched and followed by `useAgentTranscript` rather than
 * through `useDetailStore`, which is a singleton bound to the *parent* session
 * — see the hook for why routing this through it would close the very
 * conversation the card floats over.
 */
export function SessionAuxAgent({
  agent,
  paths,
}: {
  agent: SessionInfo;
  /** Workspace context that turns path-shaped inline code into clickable
   *  chips, same as the parent transcript gets. A subagent runs in the
   *  parent's workspace, so the parent's context is the right one. */
  paths?: PathLinkContext;
}) {
  const { t } = useTranslation();
  const live = isLiveMember(agent);
  const { messages, isLoading, stalled, retry } = useAgentTranscript(
    agent.jsonlPath,
    live,
  );

  return (
    <div className={styles.aux_doc_pane}>
      <MessageList
        messages={messages}
        isLoading={isLoading}
        stalled={stalled}
        onRetry={retry}
        // Drives the activity indicator under the newest message, so a
        // thinking agent reads as thinking rather than as stopped.
        status={live ? agent.status : null}
        paths={paths}
        jsonlPath={agent.jsonlPath}
      />
      {/* A subagent's transcript is short and complete; there is no
          "load earlier" because the preview opens on the whole of a run that
          fits in its window. Say so only when the window is genuinely empty —
          an agent the scan has seen but that has not written a record yet. */}
      {!isLoading && !stalled && messages.length === 0 && (
        <p className={styles.rail_empty}>
          {t("detail.agent_pane_empty", "该 Agent 还没有写入任何消息")}
        </p>
      )}
    </div>
  );
}
