import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Ban,
  CircleCheck,
  Copy,
  FolderOpen,
  Inbox,
  Sparkles,
  TriangleAlert,
  Undo2,
  Upload,
  X,
} from "lucide-react";

import {
  draftedFolders,
  groupIngestItems,
  newFolderItems,
  ingestProgress,
  isIngestRunning,
  type IngestBatch,
  type IngestBatchSummary,
  type IngestItem,
} from "../../../shared-ts/libraryIngest";
import type { LibraryFolderRec } from "../../../shared-ts/libraryTree";
import { itemNoteKey, rejectedReasonKey } from "../libraryUpload";
import { Spinner } from "./loading";
import { timeAgo } from "./timeAgo";
import styles from "./LibraryBatch.module.css";

/** Polling period while a batch is being filed. */
export const BATCH_POLL_MS = 1500;

function durationText(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  return `${m}m${String(s % 60).padStart(2, "0")}s`;
}

/** One upload under its project's header: status, progress, age. */
export function BatchRow({
  batch,
  depth,
  active,
  onOpen,
}: {
  batch: IngestBatchSummary;
  depth: number;
  active: boolean;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const running = isIngestRunning(batch.status);
  const { done, total } = ingestProgress(batch);
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  return (
    <button
      type="button"
      className={`${styles.batch_row}${active ? ` ${styles.batch_row_active}` : ""}`}
      style={{ paddingLeft: 8 + depth * 14 }}
      onClick={onOpen}
      data-status={batch.status}
    >
      <span className={styles.batch_icon} data-status={batch.status}>
        {running ? (
          <Spinner size={12} />
        ) : batch.status === "done" ? (
          <CircleCheck size={14} strokeWidth={1.6} />
        ) : batch.status === "failed" ? (
          <TriangleAlert size={14} strokeWidth={1.6} />
        ) : batch.status === "undone" ? (
          <Undo2 size={14} strokeWidth={1.6} />
        ) : (
          <Upload size={14} strokeWidth={1.6} />
        )}
      </span>
      <span className={styles.batch_body}>
        <span className={styles.batch_title}>
          {t("library.batch_label", "上传 · {{count}} 个文件", { count: batch.total })}
          <span className={styles.batch_status}>
            {t(`library.batch_status_${batch.status}`)}
            {running && total > 0 && batch.status !== "staging" && batch.status !== "drafting"
              ? ` ${done}/${total}`
              : ""}
          </span>
        </span>
        {running ? (
          <span className={styles.progress} aria-hidden>
            <span className={styles.progress_fill} style={{ width: `${Math.max(pct, 4)}%` }} />
          </span>
        ) : (
          <span className={styles.batch_meta}>
            {t("library.stat_filed", "归档 {{n}}", { n: batch.filed - batch.unfiled })}
            {batch.unfiled > 0 && ` · ${t("library.stat_unfiled", "未归档 {{n}}", { n: batch.unfiled })}`}
            {batch.duplicates > 0 && ` · ${t("library.stat_dup", "重复 {{n}}", { n: batch.duplicates })}`}
            {batch.updatedMs > 0 && ` · ${timeAgo(batch.updatedMs, t)}`}
          </span>
        )}
      </span>
    </button>
  );
}

/**
 * The batch summary — "what went into which folder". Every filed item shows
 * the AI's title and summary and can be moved to another folder or undone;
 * the whole batch can be undone once it finished.
 */
export function BatchDetail({
  id,
  folders,
  onChanged,
  onOpenArtifact,
  onClose,
}: {
  id: string;
  /** The project's schema folders, for the move picker. */
  folders: LibraryFolderRec[];
  /** Something was filed, moved or undone: the tree should reload. */
  onChanged: () => void;
  onOpenArtifact: (artifactId: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [batch, setBatch] = useState<IngestBatch | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setBatch(await invoke<IngestBatch>("library_ingest_batch", { id }));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  const running = batch ? isIngestRunning(batch.status) : false;
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => void load(), BATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [running, load]);

  const act = useCallback(
    async (key: string, op: () => Promise<IngestBatch | null>) => {
      setBusy(key);
      try {
        const next = await op();
        if (next) setBatch(next);
        setError(null);
        onChanged();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(null);
      }
    },
    [onChanged],
  );

  const grouped = useMemo(() => (batch ? groupIngestItems(batch) : null), [batch]);

  if (!batch || !grouped) {
    return (
      <div className={styles.detail}>
        {error ? <div className={styles.error}>{error}</div> : <Spinner size={16} />}
      </div>
    );
  }

  const filed = batch.items.filter((i) => i.status === "filed");
  const elapsed = (batch.finishedMs ?? Date.now()) - batch.createdMs;
  const stats: string[] = [
    t("library.stat_total", "{{n}} 个文件", { n: batch.items.length }),
    t("library.stat_filed", "归档 {{n}}", { n: filed.filter((i) => i.folder).length }),
  ];
  const unfiled = filed.filter((i) => !i.folder).length;
  if (unfiled) stats.push(t("library.stat_unfiled", "未归档 {{n}}", { n: unfiled }));
  if (grouped.duplicates.length) stats.push(t("library.stat_dup", "重复 {{n}}", { n: grouped.duplicates.length }));
  if (grouped.failed.length) stats.push(t("library.stat_failed", "失败 {{n}}", { n: grouped.failed.length }));
  if (batch.rejected.length) stats.push(t("library.stat_rejected", "未导入 {{n}}", { n: batch.rejected.length }));
  const undone = batch.items.filter((i) => i.status === "undone").length;
  if (undone) stats.push(t("library.stat_undone", "已撤销 {{n}}", { n: undone }));
  stats.push(t("library.stat_time", "用时 {{t}}", { t: durationText(elapsed) }));
  if (batch.llmCalls > 0) {
    stats.push(
      t("library.stat_cost", "AI 花费 ${{cost}}（{{calls}} 次调用）", {
        cost: batch.costUsd.toFixed(3),
        calls: batch.llmCalls,
      }),
    );
  }

  const move = (item: IngestItem, folder: string) =>
    void act(item.id, () =>
      invoke<IngestBatch>("library_ingest_move", { batchId: batch.id, itemId: item.id, folder }),
    );
  const undoItem = (item: IngestItem) =>
    void act(item.id, () =>
      invoke<IngestBatch>("library_ingest_undo", { batchId: batch.id, itemId: item.id }),
    );
  const undoBatch = () => {
    const n = filed.length;
    if (
      !window.confirm(
        t(
          "library.undo_batch_confirm",
          "撤销这次上传？已归档的 {{n}} 个文件会从资料里删除，AI 新建且已空的目录也会删掉。",
          { n },
        ),
      )
    )
      return;
    void act("batch", () => invoke<IngestBatch>("library_ingest_undo", { batchId: batch.id, itemId: null }));
  };
  const dismiss = () =>
    void act("dismiss", async () => {
      await invoke("library_ingest_dismiss", { batchId: batch.id });
      onClose();
      return null;
    });

  return (
    <div className={styles.detail}>
      <header className={styles.head}>
        <div className={styles.head_title}>
          <Upload size={16} strokeWidth={1.6} />
          <span>{t("library.batch_title", "上传整理")}</span>
          <span className={styles.chip} data-status={batch.status} data-batch-status={batch.status}>
            {running && <Spinner size={10} />}
            {t(`library.batch_status_${batch.status}`)}
          </span>
          <span className={styles.head_project}>{batch.workspaceName}</span>
          <button type="button" className={styles.close} onClick={onClose} aria-label="Close">
            <X size={14} />
          </button>
        </div>
        <div className={styles.stats}>{stats.join(" · ")}</div>
        {batch.schemaDrafted && draftedFolders(batch).length > 0 && (
          <div className={styles.notice}>
            <Sparkles size={13} strokeWidth={1.6} />
            <span>
              {t(
                "library.schema_drafted",
                "这个项目原来没有目录结构，AI 按项目内容新建了：{{folders}}。可以在左侧树里改名、改说明。",
                { folders: draftedFolders(batch).join("、") },
              )}
            </span>
          </div>
        )}
        {(batch.newFolders ?? []).length > 0 && (
          <div className={styles.new_folders} data-new-folders>
            <div className={styles.new_folders_head}>
              <Sparkles size={13} strokeWidth={1.6} />
              {t("library.new_folders_head", "已有目录都不合适，AI 新建了 {{n}} 个目录（在左侧树里带标记，可保留、改名或删除）", {
                n: (batch.newFolders ?? []).length,
              })}
            </div>
            {(batch.newFolders ?? []).map((n) => (
              <div key={n.path} className={styles.new_folder}>
                <span className={styles.new_folder_path}>
                  <FolderOpen size={12} strokeWidth={1.5} />
                  {n.path}
                  <span className={styles.group_count}>
                    {t("library.new_folder_items", "{{n}} 个文件", { n: newFolderItems(batch, n.path) })}
                  </span>
                </span>
                <span className={styles.new_folder_reason}>
                  {t("library.new_folder_reason", "原因：{{reason}}", { reason: n.reason })}
                </span>
              </div>
            ))}
          </div>
        )}
        {batch.error && <div className={styles.error}>{batch.error}</div>}
        {error && <div className={styles.error}>{error}</div>}
        <div className={styles.head_actions}>
          {!running && filed.length > 0 && batch.status !== "undone" && (
            <button type="button" className={styles.button} onClick={undoBatch} disabled={busy !== null}>
              {busy === "batch" ? <Spinner size={11} /> : <Undo2 size={12} />}
              {t("library.undo_batch", "撤销整批")}
            </button>
          )}
          {!running && (
            <button type="button" className={styles.button_quiet} onClick={dismiss} disabled={busy !== null}>
              {t("library.dismiss", "清除记录")}
            </button>
          )}
        </div>
      </header>

      <div className={styles.body} data-batch-body>
        {grouped.pending.length > 0 && (
          <section className={styles.group}>
            <h4 className={styles.group_head}>
              <Spinner size={11} />
              {t("library.pending_section", "处理中")} ({grouped.pending.length})
            </h4>
            {grouped.pending.map((it) => (
              <div key={it.id} className={styles.item_pending}>
                {it.relPath}
              </div>
            ))}
          </section>
        )}

        {grouped.groups.map((g) => (
          <section key={g.folder || "\u0000"} className={styles.group}>
            <h4 className={styles.group_head}>
              {g.folder ? <FolderOpen size={13} strokeWidth={1.5} /> : <Inbox size={13} strokeWidth={1.5} />}
              {g.folder || t("library.unfiled_group", "未归档（没有合适的目录）")}
              <span className={styles.group_count}>{g.items.length}</span>
            </h4>
            {g.items.map((it) => (
              <ItemCard
                key={it.id}
                item={it}
                folders={folders}
                busy={busy === it.id}
                onMove={(f) => move(it, f)}
                onUndo={() => undoItem(it)}
                onOpen={() => it.artifactId && it.status === "filed" && onOpenArtifact(it.artifactId)}
              />
            ))}
          </section>
        ))}

        {grouped.duplicates.length > 0 && (
          <section className={styles.group}>
            <h4 className={styles.group_head}>
              <Copy size={13} strokeWidth={1.5} />
              {t("library.dup_section", "重复的文件（未导入）")}
              <span className={styles.group_count}>{grouped.duplicates.length}</span>
            </h4>
            {grouped.duplicates.map((it) => (
              <div key={it.id} className={styles.line}>
                <span className={styles.line_path}>{it.relPath}</span>
                <span className={styles.line_note}>
                  {t("library.dup_of", "与「{{title}}」相同", { title: it.duplicateOf?.title ?? "" })}
                </span>
              </div>
            ))}
          </section>
        )}

        {(grouped.failed.length > 0 || batch.rejected.length > 0) && (
          <section className={styles.group}>
            <h4 className={styles.group_head}>
              <Ban size={13} strokeWidth={1.5} />
              {t("library.rejected_section", "未导入")}
              <span className={styles.group_count}>{grouped.failed.length + batch.rejected.length}</span>
            </h4>
            {grouped.failed.map((it) => (
              <div key={it.id} className={styles.line}>
                <span className={styles.line_path}>{it.relPath}</span>
                <span className={styles.line_note} title={it.error ?? ""}>
                  {it.error && itemNoteKey(it.error) ? t(itemNoteKey(it.error)) : it.error}
                </span>
              </div>
            ))}
            {batch.rejected.map((r) => {
              const { key, params } = rejectedReasonKey(r.reason);
              return (
                <div key={r.relPath} className={styles.line}>
                  <span className={styles.line_path}>{r.relPath}</span>
                  <span className={styles.line_note}>{key ? t(key, params) : r.reason}</span>
                </div>
              );
            })}
          </section>
        )}
        {batch.skippedJunk > 0 && (
          <div className={styles.footnote}>
            {t("library.skipped_junk", "另跳过 {{n}} 个系统垃圾文件（.DS_Store、__MACOSX 等）", {
              n: batch.skippedJunk,
            })}
          </div>
        )}
      </div>
    </div>
  );
}

function ItemCard({
  item,
  folders,
  busy,
  onMove,
  onUndo,
  onOpen,
}: {
  item: IngestItem;
  folders: LibraryFolderRec[];
  busy: boolean;
  onMove: (folder: string) => void;
  onUndo: () => void;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const undone = item.status === "undone";
  const filed = item.status === "filed";
  return (
    <div className={`${styles.item}${undone ? ` ${styles.item_undone}` : ""}`} data-item={item.id}>
      <div className={styles.item_top}>
        <button type="button" className={styles.item_title} onClick={onOpen} disabled={!filed}>
          {item.title || item.name}
        </button>
        {item.textMethod === "vision" && (
          <span className={styles.badge} title={t("library.read_by_vision", "AI 看图识别")}>
            <Sparkles size={10} strokeWidth={1.8} />
            {t("library.read_by_vision", "AI 看图识别")}
          </span>
        )}
        {undone && <span className={styles.badge_dim}>{t("library.undone_badge", "已撤销")}</span>}
        {busy && <Spinner size={11} />}
      </div>
      <div className={styles.item_path} title={item.relPath}>
        {item.relPath}
      </div>
      {item.summary && <div className={styles.item_summary}>{item.summary}</div>}
      {item.error && (
        <div className={styles.item_note} title={item.error}>
          {itemNoteKey(item.error) ? t(itemNoteKey(item.error)) : item.error}
        </div>
      )}
      <div className={styles.item_foot}>
        {item.tags.map((tag) => (
          <span key={tag} className={styles.tag}>
            {tag}
          </span>
        ))}
        <span className={styles.spacer} />
        {filed && (
          <>
            <select
              className={styles.move}
              value={item.folder ?? ""}
              disabled={busy}
              aria-label={t("library.move_to", "移到…")}
              onChange={(e) => onMove(e.target.value)}
            >
              <option value="">{t("library.unfiled", "未归档")}</option>
              {folders.map((f) => (
                <option key={f.path} value={f.path}>
                  {f.path}
                </option>
              ))}
            </select>
            <button type="button" className={styles.button_quiet} onClick={onUndo} disabled={busy}>
              <Undo2 size={11} />
              {t("library.undo_item", "撤销")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
