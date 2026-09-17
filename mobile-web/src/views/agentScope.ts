// "Scope" (a main session + sidechains it spawned) terminology for mobile.
//
// These functions lived in views/AgentScopeSwitcher.tsx originally — a
// dropdown hung on the header. Dropped it (was taking 83px of 390px with just
// "you're looking at the main session" noise), moved the family list to the ☰
// menu entirely, leaving the phrasing here for reuse.
//
// Desktop equivalent is claw-fleet-desktop's AgentScopeSwitcher.

import { t } from "../i18n";
import type { SessionInfo } from "../types";

/** Identity label for a family member: ◈ main session, or ⎇ plus its
 *  agentType (falls back to generic "sidechain" when type not recorded).
 *
 *  Codex sidechains almost never have agentType — their `thread_spawn`'s
 *  `agent_role` is basically null; what distinguishes them is `agent_nickname`
 *  (Kuhn / Ohm / Bohr), which core puts on aiTitle (`codex_source` uses it as
 *  the preferred ai_title source). So codex sidechains fall back to aiTitle;
 *  otherwise three rows of sidechains read identically. */
export function agentLabel(s: SessionInfo): string {
  if (!s.isSubagent) return `◈ ${t("主进程")}`;
  const codexNickname = s.agentSource === "codex" ? s.aiTitle : null;
  return `⎇ ${s.agentType || codexNickname || t("子代理")}`;
}

/** Tail of sidechain id to distinguish two same-type sidechains (e.g., two
 *  `general-purpose` wouldn't read as one line). Take the tail: real ids
 *  (`agent-<uuid>`) share `agent-` prefix, only the tail distinguishes them. */
export function agentIdTail(id: string): string {
  const raw = id.replace(/^agent-/, "");
  return `#${raw.slice(-6)}`;
}
