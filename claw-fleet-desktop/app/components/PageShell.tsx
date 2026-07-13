import type { ReactNode } from "react";
import { useUIStore } from "../store";
import type { ViewMode } from "../store";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { CollapsedSidebarRail } from "./CollapsedSidebarRail";
import { RAILS } from "./pageShellConfig";
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
 * The application shell every page wears: a full-width banner over an optional
 * secondary sidebar + the main container.
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
  const collapsed = useUIStore((s) => !!s.secondarySidebarCollapsed[view]);
  const setSecondarySidebar = useUIStore((s) => s.setSecondarySidebar);

  // Hooks cannot be conditional, so a rail-less page still calls this — with a
  // key it never writes, because the drag handle is never rendered.
  const { width, isDragging, onMouseDown } = useResizableWidth(
    rail?.storageKey ?? "__page_shell_no_rail__",
    {
      min: rail?.min ?? 0,
      max: rail?.max ?? 0,
      initial: rail?.initial ?? 0,
      side: rail?.side,
    },
  );

  const railEl =
    secondary === undefined ? null : collapsed ? (
      <CollapsedSidebarRail
        side={rail?.side}
        onExpand={() => setSecondarySidebar(view, false)}
      />
    ) : (
      <aside className={styles.rail} style={{ width }}>
        {secondary}
        <ResizeHandle side={rail?.side} active={isDragging} onMouseDown={onMouseDown} />
      </aside>
    );

  const main = <main className={styles.main}>{children}</main>;

  return (
    <div className={className ? `${styles.page} ${className}` : styles.page}>
      <header className={styles.banner} data-tauri-drag-region>
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
        {bannerCenter}
        {actions && <div className={styles.actions}>{actions}</div>}
      </header>

      {subBar && <div className={styles.sub_bar}>{subBar}</div>}

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
