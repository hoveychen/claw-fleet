// Uploads on the phone: a project's recent batches as rows (progress while
// the host files them), and one batch's summary — "what went into which
// folder" — with item undo, item move, whole-batch undo and the new-folder
// banner. The same derivations as the desktop's LibraryBatch
// (shared-ts/libraryIngest.ts), laid out for one column.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Ban,
  CircleCheck,
  Copy,
  FolderInput,
  FolderOpen,
  Inbox,
  Sparkles,
  TriangleAlert,
  Undo2,
  Upload,
} from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import {
  dismissIngest,
  draftedFolders,
  fetchIngestBatch,
  folderChoices,
  groupIngestItems,
  ingestProgress,
  isIngestRunning,
  moveIngestItem,
  newFolderItems,
  undoIngest,
  type IngestBatch,
  type IngestBatchStatus,
  type IngestBatchSummary,
  type IngestItem,
  type LibraryFolderRec,
} from "../libraryApi";
import { useConfirm } from "../confirmDialog";
import { AppHeader } from "./AppHeader";
import { Spinner } from "./loading";
import styles from "./LibraryBatch.module.css";

/** Polling period while a batch is being filed. */
export const BATCH_POLL_MS = 1500;

const STATUS_LABEL: Record<IngestBatchStatus, string> = {
  receiving: "接收中",
  queued: "排队中",
  staging: "解包中",
  extracting: "读取内容",
  drafting: "起草目录",
  classifying: "AI 分类",
  filing: "归档中",
  done: "已完成",
  failed: "失败",
  undone: "已撤销",
};

export function batchStatusLabel(s: IngestBatchStatus): string {
  return t(STATUS_LABEL[s] ?? s);
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ── Rows ────────────────────────────────────────────────────────────────────

export function BatchRows({
  batches,
  onOpen,
}: {
  batches: IngestBatchSummary[];
  onOpen: (id: string) => void;
}) {
  return (
    <div className={styles.rows}>
      {batches.map((b) => {
        const running = isIngestRunning(b.status);
        const { done, total } = ingestProgress(b);
        const pct = total > 0 ? Math.round((done / total) * 100) : 0;
        return (
          <button
            key={b.id}
            type="button"
            className={styles.row}
            onClick={() => onOpen(b.id)}
            data-batch={b.id}
            data-status={b.status}
          >
            <span className={styles.rowIcon} data-status={b.status}>
              {running ? (
                <Spinner size={13} />
              ) : b.status === "done" ? (
                <CircleCheck size={15} strokeWidth={1.6} />
              ) : b.status === "failed" ? (
                <TriangleAlert size={15} strokeWidth={1.6} />
              ) : b.status === "undone" ? (
                <Undo2 size={15} strokeWidth={1.6} />
              ) : (
                <Upload size={15} strokeWidth={1.6} />
              )}
            </span>
            <span className={styles.rowBody}>
              <span className={styles.rowTitle}>
                {t("上传 · {0} 个文件", b.total)}
                <span className={styles.rowStatus}>
                  {batchStatusLabel(b.status)}
                  {running && total > 0 && b.status !== "staging" && b.status !== "drafting"
                    ? ` ${done}/${total}`
                    : ""}
                </span>
              </span>
              {running ? (
                <span className={styles.progress} aria-hidden="true">
                  <span className={styles.progressFill} style={{ width: `${Math.max(pct, 4)}%` }} />
                </span>
              ) : (
                <span className={styles.rowMeta}>
                  {[
                    t("归档 {0}", b.filed - b.unfiled),
                    b.unfiled > 0 ? t("未归档 {0}", b.unfiled) : "",
                    b.duplicates > 0 ? t("重复 {0}", b.duplicates) : "",
                    b.failed > 0 ? t("失败 {0}", b.failed) : "",
                    (b.newFolders ?? []).length > 0 ? t("新目录 {0}", (b.newFolders ?? []).length) : "",
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              )}
            </span>
          </button>
        );
      })}
    </div>
  );
}

// ── One batch ───────────────────────────────────────────────────────────────

export function BatchView({
  client,
  batchId,
  folders,
  skipped,
  onChanged,
  onOpenArtifact,
  onBack,
}: {
  client: FleetTransport | null;
  batchId: string;
  /** The project's schema folders, for the move picker. */
  folders: LibraryFolderRec[];
  /** Files this phone could not send into the batch, with why. */
  skipped?: Array<{ name: string; reason: string }>;
  /** Something was filed, moved or undone: the tree should reload. */
  onChanged: () => void;
  onOpenArtifact: (id: string) => void;
  onBack: () => void;
}) {
  const confirm = useConfirm();
  const [batch, setBatch] = useState<IngestBatch | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!client) return;
    try {
      setBatch(await fetchIngestBatch(client, batchId));
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  }, [client, batchId]);

  useEffect(() => {
    void load();
  }, [load]);

  const running = batch ? isIngestRunning(batch.status) : false;
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => void load(), BATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [running, load]);
  // The tree learns about what was filed once filing ends.
  useEffect(() => {
    if (batch && !running) onChanged();
    // Only on the running → finished edge, not on every reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  const act = useCallback(
    async (key: string, op: () => Promise<IngestBatch | null>) => {
      setBusy(key);
      try {
        const next = await op();
        if (next) setBatch(next);
        setError(null);
        onChanged();
      } catch (e) {
        setError(errText(e));
      } finally {
        setBusy(null);
      }
    },
    [onChanged],
  );

  const grouped = useMemo(() => (batch ? groupIngestItems(batch) : null), [batch]);
  const choices = useMemo(() => folderChoices(folders), [folders]);

  const header = (
    <AppHeader
      onBack={onBack}
      title={t("上传整理")}
      sub={batch ? batch.workspaceName : undefined}
    />
  );
  if (!batch || !grouped || !client) {
    return (
      <div className={styles.page}>
        {header}
        <div className={styles.center}>
          {error ? <div className={styles.error}>{error}</div> : <Spinner size={18} />}
        </div>
      </div>
    );
  }

  const filed = batch.items.filter((i) => i.status === "filed");
  // Items past reading: what the bar counts while the host works through them.
  const total = batch.items.length;
  const done = batch.items.filter((i) => !["staged", "extracted"].includes(i.status)).length;
  const stats: string[] = [
    t("{0} 个文件", batch.items.length),
    t("归档 {0}", filed.filter((i) => i.folder).length),
  ];
  const unfiled = filed.filter((i) => !i.folder).length;
  if (unfiled) stats.push(t("未归档 {0}", unfiled));
  if (grouped.duplicates.length) stats.push(t("重复 {0}", grouped.duplicates.length));
  if (grouped.failed.length) stats.push(t("失败 {0}", grouped.failed.length));
  const undone = batch.items.filter((i) => i.status === "undone").length;
  if (undone) stats.push(t("已撤销 {0}", undone));
  if (batch.llmCalls > 0) stats.push(t("AI 花费 ${0}", batch.costUsd.toFixed(3)));

  const undoBatch = async () => {
    const ok = await confirm(
      t("撤销这次上传？已归档的 {0} 个文件会从资料里删除，AI 新建且已空的目录也会删掉。", filed.length),
    );
    if (ok) void act("batch", () => undoIngest(client, batch.id));
  };
  const dismiss = () =>
    void act("dismiss", async () => {
      await dismissIngest(client, batch.id);
      onBack();
      return null;
    });

  const newFolders = batch.newFolders ?? [];
  const drafted = batch.schemaDrafted ? draftedFolders(batch) : [];

  return (
    <div className={styles.page} data-batch-view={batch.id}>
      {header}
      <div className={styles.scroll}>
        <div className={styles.head}>
          <div className={styles.headLine}>
            <span className={styles.chip} data-status={batch.status}>
              {running && <Spinner size={10} />}
              {batchStatusLabel(batch.status)}
              {running && total > 0 ? ` ${done}/${total}` : ""}
            </span>
            <span className={styles.stats}>{stats.join(" · ")}</span>
          </div>
          {running && (
            <span className={styles.progress} aria-hidden="true">
              <span
                className={styles.progressFill}
                style={{ width: `${Math.max(total > 0 ? Math.round((done / total) * 100) : 0, 4)}%` }}
              />
            </span>
          )}
          {skipped && skipped.length > 0 && (
            <div className={styles.warn}>
              <TriangleAlert size={13} strokeWidth={1.6} />
              <span>
                {t("{0} 个文件没传上：", skipped.length)}
                {skipped
                  .map((s) => `${s.name}（${s.reason === "too-large" ? t("超过 512 MB") : s.reason}）`)
                  .join("、")}
              </span>
            </div>
          )}
          {drafted.length > 0 && (
            <div className={styles.notice}>
              <Sparkles size={13} strokeWidth={1.6} />
              <span>
                {t("这个项目原来没有目录结构，AI 按内容新建了：{0}。", drafted.join("、"))}
              </span>
            </div>
          )}
          {newFolders.length > 0 && (
            <div className={styles.notice} data-new-folders>
              <Sparkles size={13} strokeWidth={1.6} />
              <div className={styles.noticeBody}>
                <span>
                  {t("已有目录都不合适，AI 新建了 {0} 个目录（在目录树里带标记，可保留、改名或删除）", newFolders.length)}
                </span>
                {newFolders.map((n) => (
                  <span key={n.path} className={styles.newFolder}>
                    <b>{n.path}</b>
                    {` · ${t("{0} 个文件", newFolderItems(batch, n.path))}`}
                    {n.reason ? ` · ${n.reason}` : ""}
                  </span>
                ))}
              </div>
            </div>
          )}
          {batch.error && <div className={styles.error}>{batch.error}</div>}
          {error && <div className={styles.error}>{error}</div>}
          {!running && (
            <div className={styles.actions}>
              {filed.length > 0 && batch.status !== "undone" && (
                <button type="button" className={styles.button} onClick={() => void undoBatch()} disabled={busy !== null}>
                  {busy === "batch" ? <Spinner size={12} /> : <Undo2 size={13} />}
                  {t("撤销整批")}
                </button>
              )}
              <button type="button" className={styles.quiet} onClick={dismiss} disabled={busy !== null}>
                {t("清除记录")}
              </button>
            </div>
          )}
        </div>

        {grouped.pending.length > 0 && (
          <section className={styles.group}>
            <h4 className={styles.groupHead}>
              <Spinner size={11} />
              {t("处理中")}
              <span className={styles.groupCount}>{grouped.pending.length}</span>
            </h4>
            {grouped.pending.map((it) => (
              <div key={it.id} className={styles.line}>
                {it.relPath}
              </div>
            ))}
          </section>
        )}

        {grouped.groups.map((g) => (
          <section key={g.folder || "\u0000"} className={styles.group}>
            <h4 className={styles.groupHead}>
              {g.folder ? <FolderOpen size={13} strokeWidth={1.5} /> : <Inbox size={13} strokeWidth={1.5} />}
              <span className={styles.groupName}>{g.folder || t("未归档（没有合适的目录）")}</span>
              <span className={styles.groupCount}>{g.items.length}</span>
            </h4>
            {g.items.map((it) => (
              <ItemCard
                key={it.id}
                item={it}
                choices={choices}
                busy={busy === it.id}
                onMove={(f) => void act(it.id, () => moveIngestItem(client, batch.id, it.id, f))}
                onUndo={() => void act(it.id, () => undoIngest(client, batch.id, it.id))}
                onOpen={() => it.status === "filed" && it.artifactId && onOpenArtifact(it.artifactId)}
              />
            ))}
          </section>
        ))}

        {grouped.duplicates.length > 0 && (
          <section className={styles.group}>
            <h4 className={styles.groupHead}>
              <Copy size={13} strokeWidth={1.5} />
              {t("重复的文件（未导入）")}
              <span className={styles.groupCount}>{grouped.duplicates.length}</span>
            </h4>
            {grouped.duplicates.map((it) => (
              <div key={it.id} className={styles.line}>
                <span>{it.relPath}</span>
                <span className={styles.lineNote}>{t("与「{0}」相同", it.duplicateOf?.title ?? "")}</span>
              </div>
            ))}
          </section>
        )}

        {(grouped.failed.length > 0 || batch.rejected.length > 0) && (
          <section className={styles.group}>
            <h4 className={styles.groupHead}>
              <Ban size={13} strokeWidth={1.5} />
              {t("未导入")}
              <span className={styles.groupCount}>{grouped.failed.length + batch.rejected.length}</span>
            </h4>
            {grouped.failed.map((it) => (
              <div key={it.id} className={styles.line}>
                <span>{it.relPath}</span>
                <span className={styles.lineNote}>{it.error}</span>
              </div>
            ))}
            {batch.rejected.map((r) => (
              <div key={r.relPath} className={styles.line}>
                <span>{r.relPath}</span>
                <span className={styles.lineNote}>{r.reason}</span>
              </div>
            ))}
          </section>
        )}
      </div>
    </div>
  );
}

function ItemCard({
  item,
  choices,
  busy,
  onMove,
  onUndo,
  onOpen,
}: {
  item: IngestItem;
  choices: Array<{ path: string; label: string }>;
  busy: boolean;
  onMove: (folder: string) => void;
  onUndo: () => void;
  onOpen: () => void;
}) {
  const undone = item.status === "undone";
  const filed = item.status === "filed";
  return (
    <div className={styles.item} data-undone={undone} data-item={item.id}>
      <button type="button" className={styles.itemTop} onClick={onOpen} disabled={!filed}>
        <span className={styles.itemTitle}>{item.title || item.name}</span>
        {item.textMethod === "vision" && (
          <span className={styles.badge}>
            <Sparkles size={10} strokeWidth={1.8} />
            {t("AI 看图识别")}
          </span>
        )}
        {undone && <span className={styles.badgeDim}>{t("已撤销")}</span>}
        {busy && <Spinner size={11} />}
      </button>
      <div className={styles.itemPath}>{item.relPath}</div>
      {item.summary && <div className={styles.itemSummary}>{item.summary}</div>}
      {item.error && <div className={styles.itemNote}>{item.error}</div>}
      {(item.tags.length > 0 || filed) && (
        <div className={styles.itemFoot}>
          {item.tags.map((tag) => (
            <span key={tag} className={styles.tag}>
              {tag}
            </span>
          ))}
          <span className={styles.spacer} />
          {filed && (
            <>
              <label className={styles.footBtn}>
                <FolderInput size={13} strokeWidth={1.6} />
                {t("移到…")}
                <select
                  className={styles.footSelect}
                  value={item.folder ?? ""}
                  disabled={busy}
                  onChange={(e) => onMove(e.target.value)}
                >
                  <option value="">{t("未归档")}</option>
                  {choices.map((c) => (
                    <option key={c.path} value={c.path}>
                      {c.label}
                    </option>
                  ))}
                </select>
              </label>
              <button type="button" className={styles.footBtn} onClick={onUndo} disabled={busy}>
                <Undo2 size={13} />
                {t("撤销")}
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
