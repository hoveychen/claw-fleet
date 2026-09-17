// Device switcher in the header title position.
//
// Previously this slot was the constant "Fleet" — a word that said nothing after
// pairing three machines. The scoped device (knowledge base, usage, where new
// sessions default) could only be switched from a list at the bottom of the
// "More" page — the control most-used and best-hidden.
//
// With one device it degenerates to plain text (that device's name): no second
// device to switch to, so a dropdown with one option is noise. When the name is
// unknown (same-origin mode, mock, old desktop not yet reporting hostname) fall
// back to "Fleet".

import { useEffect } from "react";
import { Check, ChevronDown, Laptop, Monitor, Server, Settings2 } from "lucide-react";
import type { PairedDevice } from "../devices";
import { t } from "../i18n";
import styles from "./DeviceSwitcher.module.css";

/** A device's connectivity status now; only the two bits the switcher displays. */
export interface DeviceStatus {
  /** Whether the link to relay/host is live. */
  connected: boolean;
  /** Whether that machine's desktop agent is online. */
  agentOnline: boolean;
}

/** Platform key → icon. Unrecognized platforms get a generic monitor, not no
 *  icon — a row missing one makes the list look broken. */
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
  /** Is the drawer open. State lives in App so actions like "switch to more page"
   *  can close it. */
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSwitch: (id: string) => void;
  /** "Manage devices" — jump to the complete list on the "More" page (rename,
   *  mute, remove all live there). */
  onManage: () => void;
}) {
  const active = devices.find((d) => d.id === activeId) ?? devices[0];
  const title = active?.label?.trim() || "Fleet";

  // Android back button / browser back on overlays: without catching it, exits
  // the whole page. Here we just handle Esc (external keyboard, desktop browser
  // debugging).
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
          {/* Drawer swallows clicks, or selecting a device would bubble to the
              backdrop and close it first. */}
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
