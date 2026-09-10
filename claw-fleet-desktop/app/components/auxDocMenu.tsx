import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AppWindow,
  BookOpen,
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  ExternalLink,
  FolderOpen,
  Package,
  PanelRightClose,
  RotateCw,
  Trash2,
  X,
  XCircle,
} from "lucide-react";
import type { TFunction } from "i18next";

import { canRevealPath } from "../canReveal";
import type { AuxDoc } from "../detailAux";
import type { AuxAction } from "./AuxDocBar";
import type { ExplorerFileContent } from "./ExplorerPane";
import type { ContextMenuItem } from "./ContextMenu";

/**
 * Every action an auxiliary-rail card offers, built once per card.
 *
 * The card's toolbar (`AuxDocBar`) and the card's right-click menu read the
 * *same* build: `actions` is the first few as icon buttons, `menu` is all of
 * them. Keeping them one array is the fix for the pair of symptoms this module
 * exists for — a toolbar that offered one button while four backend commands
 * to act on the thing sat unused, and a right-click that fell through to the
 * app-wide Settings/About/Quit menu because no rail card had ever registered
 * an `onContextMenu`. Two entry points, one list, so neither can drift.
 *
 * Nothing here is new capability: `export_artifact`, `reveal_artifact`,
 * `open_artifact_external`, `delete_artifact`, `reveal_path`, the wiki's
 * `copyDocRef` / `exportDoc` and the 产出 / 知识库 page navigations all already
 * existed. They were simply unreachable from the rail.
 */

export interface AuxMenuBuild {
  actions: AuxAction[];
  menu: ContextMenuItem[];
}

/**
 * The card-management half of every menu, owned by the rail rather than by the
 * reader inside the card.
 *
 * Every card has these regardless of kind, and they are what makes a right-click
 * on a *collapsed* chip worth having: expanding, dismissing this one, and
 * clearing the stack are exactly the things you want when the rail has eight
 * chips in it and no toolbar is on screen at all.
 */
export interface AuxCardTail {
  isExpanded: boolean;
  onToggle: () => void;
  onClose: () => void;
  onCloseOthers: () => void;
  onCloseAll: () => void;
  /** Put the whole rail away — the header switch's action, offered from the
   *  card because that is where the pointer already is.
   *
   *  It lives on the *cards* rather than on a menu of the rail's own for a
   *  layout reason worth stating: `.rail` is `pointer-events: none` with
   *  `.rail > * { pointer-events: auto }`, so the transparent gaps between
   *  cards hand every click to the transcript underneath. A right-click on the
   *  rail's "background" therefore never reaches the rail at all — an
   *  `onContextMenu` on the `<aside>` looks correct, passes a jsdom test (no
   *  hit-testing there) and is dead in the app. */
  onHideRail: () => void;
  /** Cards other than this one — hides "close others" when there are none. */
  otherCount: number;
}

/** Errors are reported by the caller's own surface (a banner, a toast) when it
 *  has one; a menu item that has already closed cannot show its own failure, so
 *  the fallback is the console rather than a silent swallow. */
type Fail = (message: string) => void;

const ICON = { size: 13, strokeWidth: 1.7 } as const;

function copy(text: string, fail: Fail, t: TFunction) {
  writeText(text).catch((e) => {
    console.error("clipboard write failed:", e);
    fail(t("detail.copy_failed", "复制失败"));
  });
}

/** Reveal reads differently per platform, and the affordance must be absent —
 *  not merely inert — where no host shell can answer it (see `canRevealPath`). */
function revealLabel(t: TFunction): string {
  const windows = document.documentElement.getAttribute("data-platform") === "windows";
  return windows ? t("paths.reveal_in_explorer") : t("paths.reveal_in_finder");
}

function expandItem(tail: AuxCardTail, t: TFunction): ContextMenuItem {
  return {
    id: "toggle",
    label: tail.isExpanded
      ? t("detail.aux_collapse_card", "收起此卡")
      : t("detail.aux_expand_card", "展开此卡"),
    icon: tail.isExpanded ? <ChevronDown {...ICON} /> : <ChevronRight {...ICON} />,
    onSelect: tail.onToggle,
  };
}

function tailItems(tail: AuxCardTail, t: TFunction): ContextMenuItem[] {
  const items: ContextMenuItem[] = [
    {
      id: "close",
      label: t("detail.aux_close_card", "关闭此卡"),
      icon: <X {...ICON} />,
      dividerBefore: true,
      onSelect: tail.onClose,
    },
  ];
  if (tail.otherCount > 0) {
    items.push(
      {
        id: "close-others",
        label: t("detail.aux_close_others", "关闭其他 {{count}} 张", { count: tail.otherCount }),
        icon: <XCircle {...ICON} />,
        onSelect: tail.onCloseOthers,
      },
      {
        id: "close-all",
        label: t("detail.aux_close_all", "全部关闭"),
        icon: <XCircle {...ICON} />,
        onSelect: tail.onCloseAll,
      },
    );
  }
  items.push({
    id: "hide-rail",
    label: t("detail.rail_hide", "收起辅助栏"),
    icon: <PanelRightClose {...ICON} />,
    dividerBefore: true,
    onSelect: tail.onHideRail,
  });
  return items;
}

// ── collapsed chip ───────────────────────────────────────────────────────────

/** What a chip's 复制 copies, and what it is called, by kind. A wiki chip copies
 *  the `[[slug]]` form because that is the doc's address everywhere else. */
function chipCopy(doc: AuxDoc, t: TFunction): { label: string; text: string } {
  switch (doc.kind) {
    case "file":
      return { label: t("detail.aux_copy_path", "复制路径"), text: doc.ref };
    case "wiki":
      return { label: t("wiki.copy_ref_short", "复制引用"), text: `[[${doc.ref}]]` };
    case "web":
      return { label: t("detail.aux_copy_url", "复制链接"), text: doc.ref };
    case "artifact":
      return { label: t("detail.aux_copy_artifact_id", "复制产出 ID"), text: doc.ref };
  }
}

/**
 * A collapsed chip's right-click menu.
 *
 * Deliberately *not* the expanded card's menu with items greyed out. A chip has
 * no loaded document behind it, so 导出 (which needs the deliverable's filename
 * and the doc's kind) and 复制文件内容 (which needs the read) genuinely cannot be
 * offered — and a menu whose items depend on whether a card happens to be open
 * would be the drift this module exists to prevent. What a chip *can* do is
 * everything that needs only its ref: expand, copy the address, hand it to the
 * page that owns it, and manage the stack.
 */
export function buildChipMenu({
  doc,
  tail,
  t,
  fail,
  onOpenPage,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  t: TFunction;
  fail: Fail;
  /** The 仓库 / 知识库 / 产出 / 浏览器 destination for this kind. */
  onOpenPage?: () => void;
}): ContextMenuItem[] {
  const copyable = chipCopy(doc, t);
  const items: ContextMenuItem[] = [expandItem(tail, t)];
  if (onOpenPage) {
    items.push({
      id: "open-page",
      label:
        doc.kind === "file"
          ? t("detail.aux_open_in_files", "在仓库页打开")
          : doc.kind === "wiki"
            ? t("tabs.wiki_open_page", "在知识库中打开")
            : doc.kind === "artifact"
              ? t("detail.ingest.open_artifact", "在产出页打开")
              : t("tabs.web_open_browser", "在浏览器中打开"),
      icon: <ExternalLink {...ICON} />,
      dividerBefore: true,
      onSelect: onOpenPage,
    });
  }
  items.push({
    id: "copy",
    label: copyable.label,
    icon: <Copy {...ICON} />,
    sub: copyable.text,
    dividerBefore: onOpenPage == null,
    onSelect: () => copy(copyable.text, fail, t),
  });
  if (doc.kind === "file" && canRevealPath()) {
    items.push({
      id: "reveal",
      label: revealLabel(t),
      icon: <FolderOpen {...ICON} />,
      onSelect: () => {
        invoke("reveal_path", { path: doc.ref }).catch(() =>
          fail(t("paths.not_found", "找不到该文件")),
        );
      },
    });
  }
  if (doc.kind === "artifact" && canRevealPath()) {
    items.push({
      id: "reveal",
      label: t("artifacts.reveal", "在访达中显示"),
      icon: <FolderOpen {...ICON} />,
      onSelect: () => {
        invoke("reveal_artifact", { id: doc.ref }).catch((e) =>
          fail(t("artifacts.reveal_failed", "显示失败：{{error}}", { error: String(e) })),
        );
      },
    });
  }
  items.push(...tailItems(tail, t));
  return items;
}

// ── file ─────────────────────────────────────────────────────────────────────

export function buildFileMenu({
  doc,
  tail,
  t,
  fail,
  content,
  onOpenInFiles,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  t: TFunction;
  fail: Fail;
  /** The loaded read, when the reader has one — only its text arm adds an item. */
  content?: ExplorerFileContent | null;
  /** Hand the path to the 仓库 page, which owns the tree and the git status.
   *  Absent on a collapsed chip's menu, where no workspace is in hand. */
  onOpenInFiles?: () => void;
}): AuxMenuBuild {
  const reveal: AuxAction = {
    id: "reveal",
    label: revealLabel(t),
    icon: <FolderOpen {...ICON} />,
    onSelect: () => {
      invoke("reveal_path", { path: doc.ref }).catch(() =>
        fail(t("paths.not_found", "找不到该文件")),
      );
    },
  };
  const copyPath: AuxAction = {
    id: "copy-path",
    label: t("detail.aux_copy_path", "复制路径"),
    icon: <Copy {...ICON} />,
    onSelect: () => copy(doc.ref, fail, t),
  };
  const openPage: AuxAction | null = onOpenInFiles
    ? {
        id: "open-files",
        label: t("detail.aux_open_in_files", "在仓库页打开"),
        icon: <ExternalLink {...ICON} />,
        onSelect: onOpenInFiles,
      }
    : null;

  const actions: AuxAction[] = [copyPath];
  if (canRevealPath()) actions.push(reveal);
  if (openPage) actions.push(openPage);

  const menu: ContextMenuItem[] = [
    expandItem(tail, t),
    ...actions.map((a) => ({ id: a.id, label: a.label, icon: a.icon, onSelect: a.onSelect })),
  ];
  // Only the text arm has anything to put on a clipboard; an image or a binary
  // would copy the word "undefined".
  if (content?.kind === "text") {
    menu.push({
      id: "copy-content",
      label: t("detail.aux_copy_content", "复制文件内容"),
      icon: <Copy {...ICON} />,
      onSelect: () => copy(content.content, fail, t),
    });
  }
  menu[1] = { ...menu[1], dividerBefore: true };
  menu.push(...tailItems(tail, t));
  return { actions, menu };
}

// ── artifact ─────────────────────────────────────────────────────────────────

export function buildArtifactMenu({
  doc,
  tail,
  t,
  fail,
  title,
  exporting,
  onExport,
  onOpenPage,
  onDelete,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  t: TFunction;
  fail: Fail;
  /** The deliverable's own title when it is loaded; the card label otherwise. */
  title: string;
  exporting?: boolean;
  /** Owns the save panel (and the browser build's download fallback). */
  onExport: () => void;
  onOpenPage: () => void;
  /** Confirms, deletes, then dismisses the card — the card outliving the
   *  deliverable it reads is the one state this must not leave behind. */
  onDelete: () => void;
}): AuxMenuBuild {
  const openPage: AuxAction = {
    id: "open-page",
    label: t("detail.ingest.open_artifact", "在产出页打开"),
    icon: <Package {...ICON} />,
    onSelect: onOpenPage,
  };
  const exportIt: AuxAction = {
    id: "export",
    label: exporting ? t("artifacts.exporting", "导出中…") : t("artifacts.export_short", "导出"),
    icon: <Download {...ICON} />,
    busy: exporting,
    onSelect: onExport,
  };
  const reveal: AuxAction = {
    id: "reveal",
    label: t("artifacts.reveal", "在访达中显示"),
    icon: <FolderOpen {...ICON} />,
    onSelect: () => {
      invoke("reveal_artifact", { id: doc.ref }).catch((e) =>
        fail(t("artifacts.reveal_failed", "显示失败：{{error}}", { error: String(e) })),
      );
    },
  };
  const openWith: AuxAction = {
    id: "open-with",
    label: t("artifacts.open_with", "用系统应用打开"),
    icon: <AppWindow {...ICON} />,
    onSelect: () => {
      invoke("open_artifact_external", { id: doc.ref }).catch((e) =>
        fail(t("artifacts.open_failed", "打开失败：{{error}}", { error: String(e) })),
      );
    },
  };

  const actions: AuxAction[] = [exportIt, openPage];
  if (canRevealPath()) actions.splice(1, 0, reveal);

  const menu: ContextMenuItem[] = [
    expandItem(tail, t),
    { id: openPage.id, label: openPage.label, icon: openPage.icon, dividerBefore: true, onSelect: openPage.onSelect },
    { id: exportIt.id, label: t("artifacts.export_short", "导出") + "…", icon: exportIt.icon, onSelect: exportIt.onSelect },
  ];
  if (canRevealPath()) {
    menu.push(
      { id: reveal.id, label: reveal.label, icon: reveal.icon, onSelect: reveal.onSelect },
      { id: openWith.id, label: openWith.label, icon: openWith.icon, onSelect: openWith.onSelect },
    );
  }
  menu.push(
    {
      id: "copy-title",
      label: t("detail.aux_copy_title", "复制标题"),
      icon: <Copy {...ICON} />,
      sub: title,
      dividerBefore: true,
      onSelect: () => copy(title, fail, t),
    },
    {
      id: "copy-id",
      label: t("detail.aux_copy_artifact_id", "复制产出 ID"),
      icon: <Copy {...ICON} />,
      sub: doc.ref,
      onSelect: () => copy(doc.ref, fail, t),
    },
    ...tailItems(tail, t),
    {
      id: "delete",
      label: t("detail.aux_delete_artifact", "删除这份产出"),
      icon: <Trash2 {...ICON} />,
      danger: true,
      onSelect: onDelete,
    },
  );
  return { actions, menu };
}

// ── wiki ─────────────────────────────────────────────────────────────────────

export function buildWikiMenu({
  doc,
  tail,
  t,
  fail,
  onOpenPage,
  onExport,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  t: TFunction;
  fail: Fail;
  onOpenPage: () => void;
  /** Absent until the doc itself is loaded: the export needs its kind (which
   *  decides md / html / zip) and the version being read. */
  onExport?: () => void;
}): AuxMenuBuild {
  const openPage: AuxAction = {
    id: "open-page",
    label: t("tabs.wiki_open_page", "在知识库中打开"),
    icon: <BookOpen {...ICON} />,
    onSelect: onOpenPage,
  };
  const copyRef: AuxAction = {
    id: "copy-ref",
    label: t("wiki.copy_ref_short", "复制引用"),
    icon: <Copy {...ICON} />,
    // The `[[slug]]` form, not the slug: that is the doc's stable address and
    // what `fleet wiki cat` and the composer's @-mention both resolve.
    onSelect: () => copy(`[[${doc.ref}]]`, fail, t),
  };
  const exportIt: AuxAction | null = onExport
    ? {
        id: "export",
        label: t("wiki.export_short", "导出"),
        icon: <Download {...ICON} />,
        onSelect: onExport,
      }
    : null;

  const actions: AuxAction[] = [copyRef];
  if (exportIt) actions.push(exportIt);
  actions.push(openPage);

  return {
    actions,
    menu: [
      expandItem(tail, t),
      { id: openPage.id, label: openPage.label, icon: openPage.icon, dividerBefore: true, onSelect: openPage.onSelect },
      ...(exportIt
        ? [{ id: exportIt.id, label: `${exportIt.label}…`, icon: exportIt.icon, onSelect: exportIt.onSelect }]
        : []),
      {
        id: copyRef.id,
        label: copyRef.label,
        icon: copyRef.icon,
        sub: `[[${doc.ref}]]`,
        dividerBefore: true,
        onSelect: copyRef.onSelect,
      },
      {
        id: "copy-slug",
        label: t("detail.aux_copy_slug", "复制 slug"),
        icon: <Copy {...ICON} />,
        sub: doc.ref,
        onSelect: () => copy(doc.ref, fail, t),
      },
      ...tailItems(tail, t),
    ],
  };
}

// ── web ──────────────────────────────────────────────────────────────────────

export function buildWebMenu({
  doc,
  tail,
  t,
  fail,
  onReload,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  t: TFunction;
  fail: Fail;
  /** Re-probes and forces a fresh frame load. Absent on a collapsed chip,
   *  which has no frame to reload. */
  onReload?: () => void;
}): AuxMenuBuild {
  const openBrowser: AuxAction = {
    id: "open-browser",
    label: t("tabs.web_open_browser", "在浏览器中打开"),
    icon: <ExternalLink {...ICON} />,
    onSelect: () => {
      openUrl(doc.ref).catch((e) => {
        console.error("openUrl failed:", doc.ref, e);
        fail(t("tabs.web_open_failed", "无法打开该链接"));
      });
    },
  };
  const copyUrl: AuxAction = {
    id: "copy-url",
    label: t("detail.aux_copy_url", "复制链接"),
    icon: <Copy {...ICON} />,
    onSelect: () => copy(doc.ref, fail, t),
  };
  const reload: AuxAction | null = onReload
    ? {
        id: "reload",
        label: t("tabs.web_reload", "重新加载"),
        icon: <RotateCw {...ICON} />,
        onSelect: onReload,
      }
    : null;

  const actions: AuxAction[] = [copyUrl];
  if (reload) actions.push(reload);
  actions.push(openBrowser);

  return {
    actions,
    menu: [
      expandItem(tail, t),
      ...(reload
        ? [{ id: reload.id, label: reload.label, icon: reload.icon, dividerBefore: true, onSelect: reload.onSelect }]
        : []),
      {
        id: openBrowser.id,
        label: openBrowser.label,
        icon: openBrowser.icon,
        dividerBefore: reload == null,
        onSelect: openBrowser.onSelect,
      },
      {
        id: copyUrl.id,
        label: copyUrl.label,
        icon: copyUrl.icon,
        sub: doc.ref,
        dividerBefore: true,
        onSelect: copyUrl.onSelect,
      },
      ...tailItems(tail, t),
    ],
  };
}
