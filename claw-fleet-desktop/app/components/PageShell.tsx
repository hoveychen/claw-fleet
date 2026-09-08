import type { ReactNode } from "react";
import { useUIStore } from "../store";
import type { ViewMode } from "../store";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { CollapsedSidebarRail } from "./CollapsedSidebarRail";
import { RAILS } from "./pageShellConfig";
import { SimpleNavigation } from "./SimpleNavigation";
import styles from "./PageShell.module.css";

interface SearchProps {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  /** Shows a spinner inside the box while a query is in flight. */
  busy?: boolean;
}

interface Props {
  /** Which page this is. Keys the rail config, the persisted width, and the
   *  collapsed flag — all three then agree by construction. */
  view: ViewMode;
  title: string;
  /** Rendered dim beside the title. `null`/`undefined` = nothing. */
  count?: number | null;
  search?: SearchProps;
  /** Free-form banner content between the search box and the actions. */
  bannerCenter?: ReactNode;
  /** Right-aligned action cluster. Kept clear of the Windows caption buttons by
   *  the banner's --win-caption-inset-x padding. */
  actions?: ReactNode;
  /** A second row under the banner (filter chips, tabs). Full page width. */
  subBar?: ReactNode;
  /** The secondary sidebar's content. Omit for a page with no rail — the shell
   *  then renders the main container alone. */
  secondary?: ReactNode;
  /** The main container. */
  children: ReactNode;
  /** Rendered inside the page root, directly after the body — for things that
   *  belong to the page rather than to its main column: context menus and
   *  confirm dialogs (Wiki), a footer section (Audit's rules tab). */
  afterBody?: ReactNode;
  /** Extra class on the page root. REQUIRED by any page whose CSS reads
   *  category-colour variables (--mem-* / --wiki-* / --risk-*): those are
   *  declared on a page-local class, and rgb(var(--mem-user)) resolves to an
   *  invalid value — silently, with no gate to catch it — if the element
   *  carrying the declaration is not an ancestor. */
  className?: string;
}

/** The page's scrollable body: rail + main. Exported so a page that renders its
 *  shell from more than one place (AuditView's tabs) can compose one itself. */
export function PageBody({ children }: { children: ReactNode }) {
  return <div className={styles.body}>{children}</div>;
}

/**
 * The application shell every page wears: three columns — the app sidebar (owned
 * by SessionList, outside this component), the page's list column, and the
 * detail column.
 *
 * The banner (title / count / search / bannerCenter / actions) and the sub-bar
 * belong to the LIST column, not to the page: everything in them — the search
 * box, the workspace <select>, the filter chips — filters or acts on the list,
 * never on what the detail column is showing. Hanging them across the full page
 * width instead produced one very long header over a two-column body, which is
 * the layout this replaces.
 *
 * Which column is the list depends on the rail's side: a left rail IS the list
 * (History, Memory, Wiki, …); with a right rail (Audit) the main container is
 * the list and the rail is the detail pane. A page with no rail is a single
 * column, so its banner spans it as before.
 *
 * Before this component the same structure was copy-pasted through seven
 * module.css files (MemoryView.module.css's own comment said "Shared by Memory /
 * Skills / Files / Plugins"), and each copy drifted — which is how the search
 * boxes ended up split 3:3 between a raised and a recessed surface.
 */
export function PageShell({
  view,
  title,
  count,
  search,
  bannerCenter,
  actions,
  subBar,
  secondary,
  children,
  afterBody,
  className,
}: Props) {
  const rail = RAILS[view];
  const simplifiedMode = useUIStore((s) => s.simplifiedMode);
  const collapsed = useUIStore((s) => !!s.secondarySidebarCollapsed[view]);
  const setSecondarySidebar = useUIStore((s) => s.setSecondarySidebar);

  // Hooks cannot be conditional, so a rail-less page still calls this — with a
  // key it never writes, because the drag handle is never rendered.
  const { width, isDragging, onMouseDown } = useResizableWidth(
    simplifiedMode ? "simplified-rail-width" : rail?.storageKey ?? "__page_shell_no_rail__",
    {
      min: simplifiedMode ? 240 : rail?.min ?? 0,
      max: simplifiedMode ? 520 : rail?.max ?? 0,
      initial: simplifiedMode ? 320 : rail?.initial ?? 0,
      side: rail?.side,
    },
  );

  // Where the banner lives. Inside the list column whenever there is one to put
  // it in; back across the page top when the page is a single column, or when
  // the rail is collapsed to its icon strip — a collapsed rail has no room for a
  // search box, and losing the page title + search on collapse would be a worse
  // trade than the wide header.
  const hasRail = secondary !== undefined;
  const effectiveCollapsed = simplifiedMode ? false : collapsed;
  const inColumn = hasRail && !effectiveCollapsed;

  const header = (
    <>
      <header
        className={inColumn ? `${styles.banner} ${styles.banner_column}` : styles.banner}
        data-tauri-drag-region
      >
        <div className={styles.title_row}>
          <h1 className={styles.title}>{title}</h1>
          {count != null && <span className={styles.count}>{count}</span>}
        </div>
        {search && (
          <div className={styles.search_wrap}>
            <input
              className={styles.search}
              type="text"
              value={search.value}
              onChange={(e) => search.onChange(e.target.value)}
              placeholder={search.placeholder}
            />
            {search.busy && <span className={styles.search_spinner} />}
          </div>
        )}
        {/* `display: contents` in the wide layout, so the page's bannerCenter
            nodes stay direct flex children of the banner exactly as before; the
            column layout turns it into a real full-width row of its own. */}
        {bannerCenter && <div className={styles.center}>{bannerCenter}</div>}
        {actions && <div className={styles.actions}>{actions}</div>}
      </header>

      {subBar && (
        <div className={inColumn ? `${styles.sub_bar} ${styles.sub_bar_column}` : styles.sub_bar}>
          {subBar}
        </div>
      )}
    </>
  );

  const railIsList = rail?.side !== "right";

  const railEl = !hasRail ? null : effectiveCollapsed ? (
    <CollapsedSidebarRail
      side={rail?.side}
      onExpand={() => setSecondarySidebar(view, false)}
    />
  ) : (
    <aside
      className={`${styles.rail}${railIsList ? "" : ` ${styles.col_last}`}`}
      style={{ width }}
    >
      {simplifiedMode && railIsList && <SimpleNavigation />}
      {inColumn && railIsList && header}
      {secondary}
      <ResizeHandle side={rail?.side} active={isDragging} onMouseDown={onMouseDown} />
    </aside>
  );

  const main = (
    <main className={`${styles.main}${hasRail && railIsList ? ` ${styles.col_last}` : ""}`}>
      {inColumn && !railIsList && header}
      {children}
    </main>
  );

  return (
    <div className={className ? `${styles.page} ${className}` : styles.page}>
      {!inColumn && header}

      <PageBody>
        {rail?.side === "right" ? (
          <>
            {main}
            {railEl}
          </>
        ) : (
          <>
            {railEl}
            {main}
          </>
        )}
      </PageBody>

      {afterBody}
    </div>
  );
}
