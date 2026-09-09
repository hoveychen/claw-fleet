import { useTranslation } from "react-i18next";
import type { RawMessage } from "../../types";
import { TextBlock } from "./TextBlock";
import { useBandOpen } from "./useBandOpen";
import styles from "./FleetEventBlock.module.css";

type FleetEvent = NonNullable<RawMessage["fleetEvent"]>;

const ICON: Record<FleetEvent["kind"], string> = {
  watch: "◷",
  handoff: "↗",
  loop: "↻",
  schedule: "◴",
};

export function FleetEventBlock({ event, text }: { event: FleetEvent; text: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useBandOpen(false, false);
  const title = t(`fleet.event.${event.kind}`, {
    defaultValue: {
      watch: "Fleet watch",
      handoff: "Fleet handoff",
      loop: "Fleet loop",
      schedule: "Fleet schedule",
    }[event.kind],
  });
  const status = t(`fleet.event.status.${event.status}`, {
    defaultValue: {
      fired: "已触发",
      timeout: "已超时",
      successor: "接力已启动",
      manual: "手动运行",
    }[event.status],
  });

  return (
    <div className={styles.root} data-testid="fleet-event-card" data-kind={event.kind}>
      <button className={styles.header} type="button" onClick={() => setOpen((v) => !v)}>
        <span className={styles.icon} aria-hidden>{ICON[event.kind]}</span>
        <span className={styles.title}>{title}</span>
        <span className={styles.status} data-status={event.status}>{status}</span>
        {event.id && <code className={styles.id}>{event.id}</code>}
        <span className={styles.arrow} aria-hidden>{open ? "▾" : "▸"}</span>
      </button>
      {open && <div className={styles.body}><TextBlock text={text} /></div>}
    </div>
  );
}
