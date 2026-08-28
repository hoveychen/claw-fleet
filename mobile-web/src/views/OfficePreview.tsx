/**
 * 手机上的 Office 预览 —— docx / xlsx / pptx。
 *
 * 与桌面端同样的三个库、同样的取舍（见 claw-fleet-desktop 的 OfficePreview），
 * 但输入不同：手机的字节是从 relay 的 base64 帧里解出来的 Uint8Array，不是一个
 * 能直接给 <iframe> 的 URL，所以这里收 Blob。
 *
 * 三个库合计约 1.6 MB（pptx-preview 自带 echarts 占 1.25 MB），对手机网络来说
 * 这是必须推迟的量：整个模块被 lazy 引入，模块内部再对每个库各做一次动态
 * import——看一份 .docx 只会下 docx-preview 那 76 KB，绝不碰 pptx 那份。
 *
 * 排版按手机竖屏收窄：pptx 按容器实宽渲染，xlsx 只画表格并允许横向滚动。
 */
import { useEffect, useRef, useState } from "react";

import { t } from "../i18n";
import styles from "./OfficePreview.module.css";

/** 一份表格里渲染多少行。手机上再多也是滚不到的。 */
const MAX_SHEET_ROWS = 500;

/** 16:9，pptx-preview 要显式像素。 */
const SLIDE_RATIO = 9 / 16;

type CellValue = string | number | boolean | Date | null;
interface ParsedSheet {
  sheet: string;
  data: CellValue[][];
}

export default function OfficePreview({
  kind,
  blob,
}: {
  kind: "docx" | "xlsx" | "pptx";
  blob: Blob;
}) {
  if (kind === "xlsx") return <SheetPreview blob={blob} />;
  return <HostPreview kind={kind} blob={blob} />;
}

function HostPreview({ kind, blob }: { kind: "docx" | "pptx"; blob: Blob }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let alive = true;
    setLoading(true);
    setErr(null);
    host.replaceChildren();

    (async () => {
      if (kind === "docx") {
        const { renderAsync } = await import("docx-preview");
        if (!alive) return;
        await renderAsync(blob, host, undefined, {
          inWrapper: true,
          breakPages: true,
          ignoreLastRenderedPageBreak: false,
        });
        fixSymbolBullets(host);
      } else {
        const { init } = await import("pptx-preview");
        if (!alive) return;
        // 竖屏下容器很窄；360 是「还没量到宽度」时的兜底，不是目标值。
        const width = host.clientWidth || 360;
        const previewer = init(host, {
          width,
          height: Math.round(width * SLIDE_RATIO),
          // 必须是 slide：list 模式下每页都画在同一个偏移上，两页会叠在一起。
          mode: "slide",
        });
        await previewer.preview(await blob.arrayBuffer());
        // 它先画分页再渲染首页，所以刚打开会显示「0/2」。
        previewer.updatePagination();
      }
    })()
      .then(() => {
        if (alive) setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setLoading(false);
        setErr(e instanceof Error ? e.message : String(e));
      });

    return () => {
      alive = false;
    };
  }, [kind, blob]);

  return (
    <div className={styles.wrap}>
      {loading && <div className={styles.status}>{t("加载中…")}</div>}
      {err && <div className={styles.status}>{err}</div>}
      <div ref={hostRef} className={kind === "docx" ? styles.docxHost : styles.pptxHost} />
    </div>
  );
}

function SheetPreview({ blob }: { blob: Blob }) {
  const [sheets, setSheets] = useState<ParsedSheet[] | null>(null);
  const [active, setActive] = useState(0);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setSheets(null);
    setErr(null);
    setActive(0);
    (async () => {
      // `/browser` 而不是包根：read-excel-file 按环境分发,根本没有根导出。
      const readXlsxFile = (await import("read-excel-file/browser")).default;
      const parsed = (await readXlsxFile(blob)) as ParsedSheet[];
      if (alive) setSheets(parsed);
    })().catch((e) => {
      if (alive) setErr(e instanceof Error ? e.message : String(e));
    });
    return () => {
      alive = false;
    };
  }, [blob]);

  if (err) return <div className={styles.status}>{err}</div>;
  if (!sheets) return <div className={styles.status}>{t("加载中…")}</div>;

  const current = sheets[Math.min(active, sheets.length - 1)];
  const rows = (current?.data ?? []).slice(0, MAX_SHEET_ROWS);

  return (
    <div className={styles.wrap}>
      {sheets.length > 1 && (
        <div className={styles.sheetTabs}>
          {sheets.map((s, i) => (
            <button
              key={`${s.sheet}-${i}`}
              className={`${styles.sheetTab} ${i === active ? styles.sheetTabOn : ""}`}
              onClick={() => setActive(i)}
            >
              {s.sheet || `#${i + 1}`}
            </button>
          ))}
        </div>
      )}
      <div className={styles.sheetScroll}>
        <table className={styles.sheetTable}>
          <tbody>
            {rows.map((row, r) => (
              <tr key={r}>
                {row.map((cell, c) => (
                  <td key={c}>{formatCell(cell)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function formatCell(cell: CellValue): string {
  if (cell === null || cell === undefined) return "";
  if (cell instanceof Date) return cell.toLocaleDateString();
  return String(cell);
}

/**
 * Word 的列表符号是符号字体私有区里的一个码位（`Symbol` 里的「\u2022」是
 * U+F0B7），docx-preview 会照搬成 `content: "\uF0B7\\9 "; font-family: Symbol`。
 * 手机上没有这些字体，于是每个项目符号都渲染成豆腐块。渲染完把这些码位换成
 * 普通字符即可。
 *
 * 与桌面端 claw-fleet-desktop/app/officeRender.ts 的同名函数是同一份逻辑 ——
 * 两个包没有共享代码路径，改一处时另一处也要改。
 */
function fixSymbolBullets(host: HTMLElement): void {
  const PRIVATE_USE = /[\ue000-\uf8ff]/g;
  for (const styleEl of host.querySelectorAll("style")) {
    const sheet = styleEl.sheet;
    if (!sheet) continue;
    for (const rule of Array.from(sheet.cssRules)) {
      const style = (rule as CSSStyleRule).style as CSSStyleDeclaration | undefined;
      const content = style?.content;
      if (!style || !content) continue;
      const replaced = content.replace(PRIVATE_USE, "\u2022");
      if (replaced === content) continue;
      style.content = replaced;
      // 符号字体本身也要去掉：它正是「装不上」的那个字体。
      style.fontFamily = "inherit";
    }
  }
}
