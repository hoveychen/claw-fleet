import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronUp, Folder, FolderGit2, X } from "lucide-react";
import { t } from "../i18n";
import type { RelayClient } from "../relay";
import type { BrowseDirResponse } from "../types";
import styles from "./DirPicker.module.css";

interface DirPickerProps {
  client: RelayClient | null;
  /** 起点目录；空字符串从桌面端的 home 开始。 */
  initialPath: string;
  onPick: (path: string) => void;
  onClose: () => void;
}

/** 在手机上挑一个桌面机器上的目录当 workspace。
 *
 *  取代原来那个裸输入框——手机用户看不见桌面上有什么目录，让他盲敲绝对路径
 *  本来就是个坏交互。桌面端的 `browse_dir` 每次只回一层子目录，「能不能往上翻」
 *  也由它判断（parent 为 null 即到达可浏览边界），所以这里不做任何路径拼接。 */
export function DirPicker({ client, initialPath, onPick, onClose }: DirPickerProps) {
  const [data, setData] = useState<BrowseDirResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(
    async (path?: string, fallbackToHome = false) => {
      if (!client) return;
      setLoading(true);
      setError(null);
      try {
        setData(await client.request<BrowseDirResponse>("browse_dir", path ? { path } : {}));
      } catch (e) {
        // 失败保留上一屏，用户还能往回退，不至于卡在空白页。
        setError(e instanceof Error ? e.message : t("读取目录失败"));
        // 但**起点**打不开时没有上一屏——手输框里可能是个早就删掉的路径，
        // 列表空空如也，连一行可点的都没有，用户只能关掉重来。回退到 home
        // 保证选择器永远走得动；错误照旧留在屏上解释为什么跳了。
        if (fallbackToHome && path) {
          try {
            setData(await client.request<BrowseDirResponse>("browse_dir", {}));
          } catch {
            // home 也读不到，没有更靠后的退路了。
          }
        }
      } finally {
        setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    void load(initialPath || undefined, true);
  }, [load, initialPath]);

  // 路径比屏幕长时，有意义的是尾部（当前在哪个目录），不是 `/Users` 那一截。
  const crumbRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = crumbRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [data?.path]);

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()}>
        <div className={styles.head}>
          <span className={styles.title}>{t("选择工作目录")}</span>
          <button className={styles.close} onClick={onClose} aria-label={t("关闭")}>
            <X size={18} />
          </button>
        </div>

        <div className={styles.crumb} ref={crumbRef}>
          {data?.path ?? initialPath ?? "…"}
        </div>

        {error && <div className={styles.error}>{error}</div>}

        <div className={styles.list}>
          {data?.parent && (
            <button className={styles.row} onClick={() => void load(data.parent!)}>
              <ChevronUp size={16} className={styles.icon} />
              <span className={styles.name}>{t("上一级")}</span>
            </button>
          )}
          {data?.entries.map((e) => (
            <button key={e.path} className={styles.row} onClick={() => void load(e.path)}>
              {e.isGitRepo ? (
                <FolderGit2 size={16} className={styles.iconRepo} />
              ) : (
                <Folder size={16} className={styles.icon} />
              )}
              <span className={styles.name}>{e.name}</span>
            </button>
          ))}
          {data && !loading && data.entries.length === 0 && (
            <div className={styles.empty}>{t("这里没有子目录")}</div>
          )}
          {data?.truncated && (
            <div className={styles.empty}>{t("子目录过多，仅显示前 500 个")}</div>
          )}
          {loading && <div className={styles.empty}>{t("读取中…")}</div>}
        </div>

        <button
          className={styles.confirm}
          disabled={!data}
          onClick={() => data && onPick(data.path)}
        >
          {t("用这个目录")}
        </button>
      </div>
    </div>
  );
}
