// The host's automatic-continuation switches (mobile_relay.rs `resume_settings` /
// `set_resume_settings`). Desktop equivalent: the "Automatic continuation" group in
// claw-fleet-desktop/app/components/SettingsPanel.tsx. These switches govern the
// host, not the phone, so they are read from and written to the host each time.
import { Fragment, useEffect, useState } from "react";
import { useI18n } from "../i18n";
import type { FleetTransport } from "../transport";
import type { ResumeTriggersConfig } from "../generated/types";
import { SkeletonCard } from "./loading";
import styles from "./MoreView.module.css";

/** Mirrors `auto_resume::AutoResumeConfig`; whole object round-trips on save. */
interface AutoResumeConfig {
  enabled: boolean;
  maxWaitHours: number;
  retryServerErrors: boolean;
  maxServerErrorRetries: number;
}

/** Mirrors `resume_triggers::ResumeSettings`. */
interface ResumeSettings {
  autoResume: AutoResumeConfig;
  planRevive: { enabled: boolean };
  triggers: ResumeTriggersConfig;
}

type Part = keyof ResumeSettings;

export function ResumeSettingsSection({ client }: { client: FleetTransport | null }) {
  const { t } = useI18n();
  // null = not fetched: offline, in flight, or a host too old to know the method.
  // The section stays hidden rather than showing switches that save nowhere.
  const [settings, setSettings] = useState<ResumeSettings | null>(null);
  // The first read is in flight: hold the section's place with a skeleton so it
  // does not pop in and shift the page. A failed read still hides it.
  const [loading, setLoading] = useState(client !== null);

  useEffect(() => {
    if (!client) return;
    let alive = true;
    setLoading(true);
    client
      .request<ResumeSettings>("resume_settings")
      .then((r) => {
        if (alive) setSettings(r);
      })
      .catch(() => {
        if (alive) setSettings(null);
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [client]);

  if (client && !settings && loading) {
    return (
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("自动续跑")}</div>
        {/* Seven switch rows plus the note. */}
        <SkeletonCard height={7 * 52 + 36} />
      </div>
    );
  }
  if (!client || !settings) return null;

  function flip<P extends Part>(part: P, patch: Partial<ResumeSettings[P]>) {
    if (!client || !settings) return;
    const next = { ...settings[part], ...patch };
    const prev = settings;
    setSettings({ ...settings, [part]: next });
    client
      .request<ResumeSettings>("set_resume_settings", { [part]: next })
      .then(setSettings)
      .catch(() => setSettings(prev));
  }

  const rows: Array<[string, boolean, (v: boolean) => void]> = [
    [t("限流后自动恢复"), settings.autoResume.enabled, (v) => flip("autoResume", { enabled: v })],
    [t("服务端报错后自动重试"), settings.autoResume.retryServerErrors, (v) => flip("autoResume", { retryServerErrors: v })],
    [t("自动唤醒无人负责的计划"), settings.planRevive.enabled, (v) => flip("planRevive", { enabled: v })],
    [t("结束任务后接着做下一个计划"), settings.triggers.finishContinue, (v) => flip("triggers", { finishContinue: v })],
    [t("计划没做完时拦住收工"), settings.triggers.planGate, (v) => flip("triggers", { planGate: v })],
    [t("接力时自动起下一棒"), settings.triggers.handoffSuccessor, (v) => flip("triggers", { handoffSuccessor: v })],
    [t("打断卡死的 Codex 回合"), settings.triggers.codexStallWatchdog, (v) => flip("triggers", { codexStallWatchdog: v })],
  ];

  return (
    <div className={styles.section}>
      <div className={styles.sectionLabel}>{t("自动续跑")}</div>
      <div className={styles.card}>
        {rows.map(([label, on, set], i) => (
          <Fragment key={label}>
            {i > 0 && <div className={styles.divider} />}
            <div className={`${styles.row} ${styles.switchRow}`}>
              <span className={styles.rowLabel}>{label}</span>
              <div className={styles.segment}>
                <button className={styles.segmentButton} data-active={!on} onClick={() => set(false)}>
                  {t("关")}
                </button>
                <button className={styles.segmentButton} data-active={on} onClick={() => set(true)}>
                  {t("开")}
                </button>
              </div>
            </div>
          </Fragment>
        ))}
        <div className={styles.rowNote}>{t("作用于桌面端主机上的所有会话")}</div>
      </div>
    </div>
  );
}
