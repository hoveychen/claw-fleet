import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { HandoffChain, SessionInfo } from "../types";
import styles from "./HandoffChainRow.module.css";
import { isKeyboardActivationKey } from "../keyboard";
import { HandoffChainModal } from "./HandoffChainModal";

/**
 * Chip showing the relay chain position (n of N hops) that opens a scrollable modal listing the full relay
 * chain: every leg's session, the relay note it left, and when the baton
 * passed. The chain lazy-loads on first open.
 *
 * Callers must guard on `session.handoff` being present.
 *
 * `inline` drops the row wrapper so the chip can sit in a chip row someone else
 * owns (the detail header's meta row), instead of claiming a line of its own.
 */
export function HandoffChainRow({
  session,
  inline = false,
}: {
  session: SessionInfo;
  inline?: boolean;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [chain, setChain] = useState<HandoffChain | null>(null);
  const [loading, setLoading] = useState(false);
  const info = session.handoff!;

  const openModal = async (e: React.SyntheticEvent) => {
    e.stopPropagation();
    setOpen(true);
    if (!chain && !loading) {
      setLoading(true);
      try {
        const c = await invoke<HandoffChain | null>("get_handoff_chain", {
          sessionId: session.id,
        });
        setChain(c ?? null);
      } catch {
        // modal shows the loading label going away; chip stays usable
      }
      setLoading(false);
    }
  };

  const body = (
    <>
      <span
        className={styles.handoff_chip}
        role="button"
        tabIndex={0}
        title={t("card.tip_handoff", { hop: info.hop, len: info.chainLen })}
        onClick={openModal}
        onKeyDown={(e) => {
          if (!isKeyboardActivationKey(e.key)) return;
          e.preventDefault();
          void openModal(e);
        }}
      >
        🔗 {t("card.handoff_chip", { hop: info.hop, len: info.chainLen })}
      </span>
      {open && (
        <HandoffChainModal
          chain={chain}
          loading={loading}
          currentSessionId={session.id}
          hop={info.hop}
          len={info.chainLen}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );

  return inline ? body : <div className={styles.handoff_row}>{body}</div>;
}
