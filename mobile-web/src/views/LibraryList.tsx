// One list of 资料 rows — wiki documents and artifacts side by side. Shared by
// the library tab and a task's 资料 segment. Opening is read-only either way:
// a document goes to the App's WikiDocView stack (via `onOpenDoc`), an
// artifact opens the existing ArtifactDetail overlay, owned here.

import { useCallback, useEffect, useMemo, useState } from "react";
import { BookOpen, ChevronRight, FileText } from "lucide-react";
import { dateLocale, t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, WikiDoc } from "../types";
import { formatBytes, listArtifacts } from "../artifacts";
import { listWikiDocs } from "../wiki";
import { mergeLibrary, type LibraryItem } from "../library";
import { ArtifactDetail, KIND_ICON } from "./ArtifactsView";
import { Presence } from "../Presence";
import styles from "./LibraryList.module.css";

export interface LibraryState {
  items: LibraryItem[] | null;
  /** First failure of either store. The other store's items still show. */
  error: string | null;
  refreshing: boolean;
  refresh: () => Promise<void>;
}

/** Load both stores of one device and merge them. */
export function useLibrary(client: FleetTransport | null): LibraryState {
  const [docs, setDocs] = useState<WikiDoc[] | null>(null);
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = useCallback(async () => {
    if (!client) return;
    setRefreshing(true);
    setError(null);
    const msg = (e: unknown) => (e instanceof Error ? e.message : String(e));
    const [d, a] = await Promise.allSettled([listWikiDocs(client), listArtifacts(client)]);
    if (d.status === "fulfilled") setDocs(d.value);
    else setDocs((prev) => prev ?? []);
    if (a.status === "fulfilled") setArtifacts(a.value);
    else setArtifacts((prev) => prev ?? []);
    const failed = [d, a].find((r): r is PromiseRejectedResult => r.status === "rejected");
    if (failed) setError(msg(failed.reason));
    setRefreshing(false);
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const items = useMemo(
    () => (docs && artifacts ? mergeLibrary(docs, artifacts) : null),
    [docs, artifacts],
  );
  return { items, error, refreshing, refresh };
}

interface Props {
  items: LibraryItem[];
  client: FleetTransport | null;
  onOpenDoc: (doc: WikiDoc) => void;
  /** Full-text search snippets by wiki slug, shown in place of the meta line. */
  snippetBySlug?: Map<string, string>;
  /** Hide the project tag (a task's own 资料 are all one project). */
  hideProject?: boolean;
}

export function LibraryList({ items, client, onOpenDoc, snippetBySlug, hideProject }: Props) {
  const [openId, setOpenId] = useState<string | null>(null);
  const open = useMemo(
    () =>
      items.find((it): it is Extract<LibraryItem, { type: "artifact" }> =>
        it.type === "artifact" && it.artifact.id === openId,
      )?.artifact ?? null,
    [items, openId],
  );

  return (
    <>
      <div className={styles.list}>
        {items.map((it) => (
          <LibraryRow
            key={it.key}
            item={it}
            snippet={it.type === "doc" ? snippetBySlug?.get(it.doc.slug) : undefined}
            hideProject={hideProject}
            onOpen={() => (it.type === "doc" ? onOpenDoc(it.doc) : setOpenId(it.artifact.id))}
          />
        ))}
      </div>
      <Presence when={!!open}>
        {open && <ArtifactDetail artifact={open} client={client} onBack={() => setOpenId(null)} />}
      </Presence>
    </>
  );
}

function LibraryRow({
  item,
  snippet,
  hideProject,
  onOpen,
}: {
  item: LibraryItem;
  snippet?: string;
  hideProject?: boolean;
  onOpen: () => void;
}) {
  const Icon = item.type === "doc" ? BookOpen : (KIND_ICON[item.artifact.kind] ?? FileText);
  const meta =
    item.type === "doc"
      ? [t("文档"), fmtDate(item.updatedMs)]
      : [t("产出"), formatBytes(item.artifact.sizeBytes), fmtDate(item.updatedMs)];
  return (
    <button type="button" className={styles.row} onClick={onOpen}>
      <span className={styles.icon} data-type={item.type}>
        <Icon size={18} strokeWidth={1.5} />
      </span>
      <span className={styles.body}>
        <span className={styles.line1}>
          <span className={styles.title}>{item.title}</span>
          {!hideProject && item.workspaceName && (
            <span className={styles.project}>{item.workspaceName}</span>
          )}
        </span>
        {snippet ? (
          <span className={styles.snippet}>{snippet}</span>
        ) : (
          <span className={styles.line2}>{meta.filter(Boolean).join(" · ")}</span>
        )}
      </span>
      <ChevronRight size={16} className={styles.chevron} />
    </button>
  );
}

function fmtDate(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleDateString(dateLocale(), { month: "2-digit", day: "2-digit" });
}
