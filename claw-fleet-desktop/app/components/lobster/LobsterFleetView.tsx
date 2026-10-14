import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../../store";
import { PageShell } from "../PageShell";
import { LobsterEngine } from "./engine";
import { drawOcean } from "./scene";
import styles from "./LobsterFleetView.module.css";

/**
 * "Lobster Fleet": every session is a pixel lobster on the sea floor, posed by
 * what it is doing. The canvas is driven by {@link LobsterEngine}; text (names,
 * badges) is DOM over it so CJK stays crisp.
 */
export function LobsterFleetView() {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<LobsterEngine | null>(null);
  const sessions = useSessionsStore((s) => s.sessions);

  useEffect(() => {
    const host = hostRef.current;
    const canvas = canvasRef.current;
    if (!host || !canvas) return;
    const engine = new LobsterEngine(canvas);
    engineRef.current = engine;
    engine.setRenderer({ draw: drawOcean, busy: () => false });
    const fit = () => engine.resize(host.clientWidth, host.clientHeight, window.devicePixelRatio || 1);
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(host);
    engine.start();
    return () => {
      ro.disconnect();
      engine.stop();
      engineRef.current = null;
    };
  }, []);

  return (
    <PageShell view="lobsters" title={t("view_lobsters", "龙虾舰队")}>
      <div ref={hostRef} className={styles.stage}>
        <canvas ref={canvasRef} className={styles.canvas} />
        {sessions.length === 0 && <div className={styles.empty}>{t("lobsters.empty")}</div>}
      </div>
    </PageShell>
  );
}
