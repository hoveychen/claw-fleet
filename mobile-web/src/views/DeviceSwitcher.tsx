// 头部标题位上的设备切换器。
//
// 在此之前那一格是常量「Fleet」——一个在配对了三台机器之后什么也没说的字。当前
// 作用域设备(知识库、用量、新会话默认落在哪一台)此前只能从「更多」页最底下那份
// 列表里切,而那正是用得最勤、藏得最深的一个开关。
//
// 一台在册时它退化成一行纯文字(那台的名字):没有第二台可切,一个永远只有一个
// 选项的下拉是噪音。名字不知道(同源形态、mock、老桌面端还没报上主机名)时才回到
// 「Fleet」。

import { useEffect } from "react";
import { Check, ChevronDown, Laptop, Monitor, Server, Settings2 } from "lucide-react";
import type { PairedDevice } from "../devices";
import { t } from "../i18n";
import styles from "./DeviceSwitcher.module.css";

/** 一台设备此刻的连通性,只取切换器要显示的那两位。 */
export interface DeviceStatus {
  /** 到中转/主机的链路通不通。 */
  connected: boolean;
  /** 那台桌面端在不在线。 */
  agentOnline: boolean;
}

/** 平台键 → 图标。认不出来的平台给一台通用显示器,而不是不给图标 —— 一行少一个
 *  图标会让列表看着像坏掉了。 */
function PlatformIcon({ platform, size = 18 }: { platform?: string; size?: number }) {
  if (platform === "macos") return <Laptop size={size} />;
  if (platform === "linux") return <Server size={size} />;
  return <Monitor size={size} />;
}

function statusText(status: DeviceStatus | undefined): string {
  if (!status?.connected) return t("未连接");
  return status.agentOnline ? t("在线") : t("桌面端离线");
}

function statusKind(status: DeviceStatus | undefined): "online" | "offline" | "down" {
  if (!status?.connected) return "down";
  return status.agentOnline ? "online" : "offline";
}

export function DeviceSwitcher({
  devices,
  activeId,
  statusOf,
  open,
  onOpenChange,
  onSwitch,
  onManage,
}: {
  devices: PairedDevice[];
  activeId: string;
  statusOf: (id: string) => DeviceStatus | undefined;
  /** 抽屉开着没有。状态放在 App 里,好让「切到更多页」这类动作能把它关掉。 */
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSwitch: (id: string) => void;
  /** 「管理设备」——跳到「更多」页那份完整列表(改名、静音、移除都在那儿)。 */
  onManage: () => void;
}) {
  const active = devices.find((d) => d.id === activeId) ?? devices[0];
  const title = active?.label?.trim() || "Fleet";

  // 安卓返回键 / 浏览器后退在这类浮层上的老问题:不拦一下就直接退出整个页面。
  // 这里只做最轻的一层 —— Esc 关掉(外接键盘、桌面浏览器调试时都会用到)。
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOpenChange(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  if (devices.length <= 1) {
    return <span className={styles.plainTitle}>{title}</span>;
  }

  return (
    <>
      <button
        className={styles.trigger}
        onClick={() => onOpenChange(!open)}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={t("切换设备（当前 {0}）", title)}
      >
        <span className={styles.triggerIcon}>
          <PlatformIcon platform={active?.platform} size={16} />
        </span>
        <span className={styles.triggerLabel}>{title}</span>
        <ChevronDown size={15} className={styles.chevron} data-open={open ? "true" : undefined} />
      </button>

      {open && (
        <div className={styles.backdrop} onClick={() => onOpenChange(false)}>
          {/* 抽屉本身吃掉点击,否则选一台的那一下会穿到背板上先把它关掉。 */}
          <div className={styles.sheet} role="listbox" onClick={(e) => e.stopPropagation()}>
            <div className={styles.sheetTitle}>{t("设备")}</div>
            {devices.map((d) => {
              const status = statusOf(d.id);
              return (
                <button
                  key={d.id}
                  className={styles.row}
                  role="option"
                  aria-selected={d.id === activeId}
                  onClick={() => {
                    if (d.id !== activeId) onSwitch(d.id);
                    onOpenChange(false);
                  }}
                >
                  <span className={styles.rowIcon}>
                    <PlatformIcon platform={d.platform} />
                  </span>
                  <span className={styles.rowText}>
                    <span className={styles.rowLabel}>{d.label || t("未命名设备")}</span>
                    <span className={styles.rowStatus} data-kind={statusKind(status)}>
                      <span className={styles.dot} />
                      {statusText(status)}
                    </span>
                  </span>
                  {d.id === activeId && <Check size={17} className={styles.check} />}
                </button>
              );
            })}
            <button
              className={styles.manage}
              onClick={() => {
                onOpenChange(false);
                onManage();
              }}
            >
              <Settings2 size={15} />
              {t("管理设备")}
            </button>
          </div>
        </div>
      )}
    </>
  );
}
