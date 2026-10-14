import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { pendingDecisionState, type PendingDecisionState } from "../../pendingDecisionState";
import { useDecisionStore, useSessionsStore, useUIStore } from "../../store";
import { preferredSessionTitle, type SessionInfo } from "../../types";
import { PageShell } from "../PageShell";
import { LobsterEngine, type PixelScale } from "./engine";
import { buildRoster, layoutFleet, MAX_SHRIMP, POD_LABEL_H, type FleetLayout } from "./layout";
import { BUSY_POSES, lookFor, type Look } from "./poses";
import { drawBubbles, drawCaustics, drawOcean, drawPod, drawSeaweed, SAND_H } from "./scene";
import { frameAt, LOBSTER_H, LOBSTER_W, lobsterSprite, shrimpSprite } from "./sprites";
import styles from "./LobsterFleetView.module.css";

/** Stable per-session animation offset so a crowd does not move in lockstep. */
function phaseOf(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return (Math.abs(h) % 1000) / 1000 * 4;
}

interface Scene {
  layout: FleetLayout;
  looks: Map<string, Look>;
}

/**
 * "Lobster Fleet": every session is a pixel lobster in its workspace's
 * submarine, posed by what it is doing; live sub-agents are shrimp at its feet.
 * The canvas is driven by {@link LobsterEngine}; text (names, badges) is DOM
 * over it so CJK stays crisp, positioned with the engine's CSS-per-art-pixel.
 */
export function LobsterFleetView() {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<LobsterEngine | null>(null);
  const [px, setPx] = useState<{ scale: PixelScale; cssPerPx: number } | null>(null);
  const sessions = useSessionsStore((s) => s.sessions);
  const decisions = useDecisionStore((s) => s.decisions);
  const requestOpenTask = useUIStore((s) => s.requestOpenTask);

  const scene = useMemo<Scene | null>(() => {
    if (!px) return null;
    const cards = new Map<string, PendingDecisionState>();
    for (const s of sessions) {
      const st = pendingDecisionState(decisions, s.id);
      if (st !== "none") cards.set(s.id, st);
    }
    const roster = buildRoster(sessions, Date.now(), (id) => cards.has(id));
    const layout = layoutFleet(roster, px.scale.w, px.scale.h - SAND_H + 4);
    const looks = new Map<string, Look>();
    for (const pod of layout.pods) {
      for (const seat of pod.seats) {
        const s = seat.member.session;
        looks.set(s.id, lookFor(s, cards.get(s.id) ?? "none"));
        for (const sub of seat.member.subagents) looks.set(sub.id, lookFor(sub));
      }
    }
    return { layout, looks };
  }, [px, sessions, decisions]);

  const sceneRef = useRef<Scene | null>(null);
  sceneRef.current = scene;

  useEffect(() => {
    const host = hostRef.current;
    const canvas = canvasRef.current;
    if (!host || !canvas) return;
    const engine = new LobsterEngine(canvas);
    engineRef.current = engine;
    engine.setRenderer({
      draw(ctx, now, w, h) {
        drawOcean(ctx, now, w, h);
        drawCaustics(ctx, now, w, h);
        drawSeaweed(ctx, now, w, h);
        const sc = sceneRef.current;
        if (!sc) return;
        const bubbleSources: { x: number; y: number }[] = [];
        for (const pod of sc.layout.pods) {
          drawPod(ctx, pod.x, pod.y, pod.w, pod.h, POD_LABEL_H);
          bubbleSources.push({ x: pod.x + pod.w - 7, y: pod.y - 6 });
          for (const seat of pod.seats) {
            const s = seat.member.session;
            const look = sc.looks.get(s.id);
            if (!look) continue;
            const phase = phaseOf(s.id);
            ctx.drawImage(
              lobsterSprite({ pose: look.pose, frame: frameAt(look.pose, now, phase), color: look.color, captain: true }),
              seat.x,
              seat.y,
            );
            seat.shrimp.forEach((pos, k) => {
              const sub = seat.member.subagents[k];
              const subLook = sc.looks.get(sub.id);
              const frame = Math.floor(now / 250 + phaseOf(sub.id)) % 2;
              ctx.drawImage(shrimpSprite({ frame, color: subLook?.color ?? "#6b6b80" }), pos.x, pos.y);
            });
          }
        }
        drawBubbles(ctx, now, bubbleSources);
      },
      busy() {
        const sc = sceneRef.current;
        if (!sc) return false;
        for (const look of sc.looks.values()) if (BUSY_POSES.has(look.pose)) return true;
        return false;
      },
    });
    const fit = () => {
      engine.resize(host.clientWidth, host.clientHeight, window.devicePixelRatio || 1);
      setPx({ scale: engine.px, cssPerPx: engine.cssPerPx });
    };
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

  // New data: redraw now rather than waiting for the idle frame.
  useEffect(() => engineRef.current?.invalidate(), [scene]);

  const k = px?.cssPerPx ?? 1;
  const at = (x: number, y: number) => ({ left: x * k, top: y * k });
  const titleOf = (s: SessionInfo) =>
    preferredSessionTitle(s) ?? s.slug ?? s.lastMessagePreview ?? s.id.slice(0, 8);
  const lobsterCount = scene?.layout.pods.reduce((n, p) => n + p.seats.length, 0) ?? 0;

  return (
    <PageShell view="lobsters" title={t("view_lobsters", "龙虾舰队")}>
      <div ref={hostRef} className={styles.stage}>
        <canvas ref={canvasRef} className={styles.canvas} />
        {scene?.layout.pods.map((pod) => (
          <div key={pod.key}>
            <div
              className={styles.podLabel}
              style={{ ...at(pod.x + 2, pod.y), width: (pod.w - 4) * k, height: (POD_LABEL_H - 1) * k }}
              title={pod.key}
            >
              <span className={styles.podName}>{pod.key}</span>
              {pod.more > 0 && <span className={styles.podMore}>+{pod.more}</span>}
            </div>
            {pod.seats.map((seat) => {
              const s = seat.member.session;
              const look = scene.looks.get(s.id);
              const title = titleOf(s);
              const subs = seat.member.subagents.length;
              return (
                <button
                  key={s.id}
                  type="button"
                  className={styles.seat}
                  style={{ ...at(seat.x, seat.y), width: LOBSTER_W * k, height: LOBSTER_H * k }}
                  title={`${title}\n${t("lobsters.open_title")}`}
                  onClick={() => requestOpenTask(s.id)}
                >
                  {look && look.alert !== "none" && (
                    <span
                      className={look.alert === "parked" ? `${styles.alert} ${styles.alertParked}` : styles.alert}
                      title={look.alert === "parked" ? t("lobsters.parked") : t("lobsters.waiting")}
                    >
                      !
                    </span>
                  )}
                  <span className={styles.name}>{title}</span>
                  {subs > MAX_SHRIMP && (
                    <span className={styles.crewBadge}>{t("lobsters.subagents", { n: subs })}</span>
                  )}
                </button>
              );
            })}
          </div>
        ))}
        {scene && scene.layout.hiddenPods > 0 && (
          <div className={styles.morePods}>{t("lobsters.more_pods", { n: scene.layout.hiddenPods })}</div>
        )}
        {scene && lobsterCount === 0 && <div className={styles.empty}>{t("lobsters.empty")}</div>}
      </div>
    </PageShell>
  );
}
