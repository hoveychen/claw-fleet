import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";

/** Result of `export_chain_bundle` (`ChainExportSummary` in core). */
export interface ChainExportSummary {
  path: string;
  bytes: number;
  members: number;
  sessions: number;
  missing: string[];
  elapsedMs: number;
}

/**
 * Ask where to save, then pack the relay chain containing `sessionId` — or
 * just that session when it is on no chain — into a `.flt` debug bundle
 * (`claw-fleet-core/src/chain_export.rs`). Resolves `null` when the user
 * cancels the save dialog; rejects with the backend's error otherwise.
 *
 * `onStart` fires once a destination is picked, i.e. when the slow part begins.
 * Desktop only: the dialog and the multi-GB log scan both need the host.
 */
export async function exportChainBundle(
  sessionId: string,
  onStart?: () => void,
): Promise<ChainExportSummary | null> {
  const defaultPath = await invoke<string>("chain_bundle_file_name", { sessionId });
  const dest = await save({
    defaultPath,
    filters: [{ name: "Fleet debug bundle", extensions: ["flt"] }],
  });
  if (!dest) return null;
  onStart?.();
  return invoke<ChainExportSummary>("export_chain_bundle", { sessionId, dest });
}
