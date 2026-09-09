import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { t } from "../i18n";
import type { RawMessage } from "../types";
import styles from "./SessionDetailView.module.css";

type FleetEvent = NonNullable<RawMessage["fleetEvent"]>;

const ICON: Record<FleetEvent["kind"], string> = {
  watch: "◷",
  handoff: "↗",
  loop: "↻",
  schedule: "◴",
};

function kindLabel(kind: FleetEvent["kind"]): string {
  return {
    watch: "Fleet watch",
    handoff: "Fleet handoff",
    loop: "Fleet loop",
    schedule: "Fleet schedule",
  }[kind];
}

function statusLabel(status: FleetEvent["status"]): string {
  return {
    fired: t("已触发"),
    timeout: t("已超时"),
    successor: t("接力已启动"),
    manual: t("手动运行"),
  }[status];
}

export function FleetEventCard({ event, text }: { event: FleetEvent; text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={styles.fleetEventCard} data-testid="fleet-event-card" data-kind={event.kind}>
      <button className={styles.fleetEventHeader} type="button" onClick={() => setOpen((v) => !v)}>
        <span className={styles.fleetEventIcon} aria-hidden>{ICON[event.kind]}</span>
        <span className={styles.fleetEventTitle}>{kindLabel(event.kind)}</span>
        <span className={styles.fleetEventStatus} data-status={event.status}>{statusLabel(event.status)}</span>
        {event.id && <code className={styles.fleetEventId}>{event.id}</code>}
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
      </button>
      {open && <div className={styles.fleetEventBody}>{text}</div>}
    </div>
  );
}
