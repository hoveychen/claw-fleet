import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { pendingDecisionState, type PendingDecisionState } from "../../pendingDecisionState";
import { useDecisionStore, useSessionsStore, useUIStore } from "../../store";
import { preferredSessionTitle, type SessionInfo } from "../../types";
import { PageShell } from "../PageShell";
import { LobsterEngine, type PixelScale } from "./engine";
import {
  bridgeTargetH,
  buildRoster,
  FLEET_TARGET_H,
  layoutBridge,
  layoutShips,
  MAX_SHRIMP,
  NAME_H,
  SHIP_CELL_W,
  SLOT_H,
  type BridgeLayout,
  type FleetLayout,
  type PodIn,
} from "./layout";
import { BUSY_POSES, lookFor, STATE_COLORS, type Look } from "./poses";
import { drawBridge, drawConsoleBank, drawSpace } from "./scene";
import { paintFor, SHIP_CANOPY_X, SHIP_H, SHIP_W, SHIP_WINDOWS, shipSprite } from "./ship";
import { frameAt, LOBSTER_H, LOBSTER_W, lobsterSprite, shrimpSprite } from "./sprites";
import styles from "./LobsterFleetView.module.css";

/** Stable per-id animation offset so a crowd does not move in lockstep. */
function phaseOf(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return ((Math.abs(h) % 1000) / 1000) * 4;
}

/** The loudest alert among a ship's crew: a live card beats a parked one. */
function shipAlert(pod: PodIn, looks: Map<string, Look>): PendingDecisionState {
  let alert: PendingDecisionState = "none";
  for (const c of pod.crew) {
    const a = looks.get(c.session.id)?.alert ?? "none";
    if (a === "pending") return "pending";
    if (a === "parked") alert = "parked";
  }
  return alert;
}

type Scene =
  | { kind: "fleet"; layout: FleetLayout; looks: Map<string, Look> }
  | { kind: "bridge"; pod: PodIn; layout: BridgeLayout; looks: Map<string, Look> };

/**
 * "Lobster Fleet", in space: the overview shows one ship per workspace, its
 * portholes lit in its sessions' state colours; boarding a ship shows the
 * bridge, where each session is a pixel lobster at a console, posed by what it
 * is doing, with its live sub-agents as shrimp at its feet. The canvas is
 * driven by {@link LobsterEngine}; text (names, badges) is DOM over it so CJK
 * stays crisp, positioned with the engine's CSS-per-art-pixel.
 */
export function LobsterFleetView() {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<LobsterEngine | null>(null);
  const [px, setPx] = useState<{ scale: PixelScale; cssPerPx: number } | null>(null);
  const [host, setHost] = useState<{ w: number; h: number; dpr: number } | null>(null);
  const [boarded, setBoarded] = useState<string | null>(null);
  const sessions = useSessionsStore((s) => s.sessions);
  const decisions = useDecisionStore((s) => s.decisions);
  const requestOpenTask = useUIStore((s) => s.requestOpenTask);

  const { roster, looks } = useMemo(() => {
    const cards = new Map<string, PendingDecisionState>();
    for (const s of sessions) {
      const st = pendingDecisionState(decisions, s.id);
      if (st !== "none") cards.set(s.id, st);
    }
    const roster = buildRoster(sessions, Date.now(), (id) => cards.has(id));
    const looks = new Map<string, Look>();
    for (const pod of roster) {
      for (const c of pod.crew) {
        looks.set(c.session.id, lookFor(c.session, cards.get(c.session.id) ?? "none"));
        for (const sub of c.subagents) looks.set(sub.id, lookFor(sub));
      }
    }
    return { roster, looks };
  }, [sessions, decisions]);

  // A ship whose crew have all gone home drops back to the overview.
  const pod = boarded ? roster.find((p) => p.key === boarded) : undefined;
  // Zoom: a small crew is drawn large on its bridge.
  const targetH = useMemo(
    () => (pod && host ? bridgeTargetH(pod.crew, Math.round(host.w * host.dpr), Math.round(host.h * host.dpr)) : FLEET_TARGET_H),
    [pod, host],
  );

  useEffect(() => {
    const engine = engineRef.current;
    if (!engine || !host) return;
    engine.resize(host.w, host.h, host.dpr, targetH);
    setPx({ scale: engine.px, cssPerPx: engine.cssPerPx });
  }, [host, targetH]);

  const scene = useMemo<Scene | null>(() => {
    if (!px) return null;
    const { w, h } = px.scale;
    if (pod) return { kind: "bridge", pod, layout: layoutBridge(pod.crew, w, h), looks };
    return { kind: "fleet", layout: layoutShips(roster, w, h), looks };
  }, [px, roster, looks, pod]);

  const sceneRef = useRef<Scene | null>(null);
  sceneRef.current = scene;

  useEffect(() => {
    const hostEl = hostRef.current;
    const canvas = canvasRef.current;
    if (!hostEl || !canvas) return;
    const engine = new LobsterEngine(canvas);
    engineRef.current = engine;
    const busyLook = (sc: Scene, id: string) => {
      const look = sc.looks.get(id);
      return !!look && BUSY_POSES.has(look.pose);
    };
    engine.setRenderer({
      draw(ctx, now, w, h) {
        const sc = sceneRef.current;
        if (!sc || sc.kind === "fleet") {
          drawSpace(ctx, now, w, h);
          if (!sc) return;
          for (const ship of sc.layout.ships) {
            const crew = ship.pod.crew;
            const windows = crew.slice(0, SHIP_WINDOWS).map((c) => sc.looks.get(c.session.id)?.color ?? STATE_COLORS.idle);
            const thrust = crew.some((c) => busyLook(sc, c.session.id));
            const bob = Math.round(Math.sin(now / 900 + phaseOf(ship.pod.key)) * 1.5);
            const frame = Math.floor(now / 120 + phaseOf(ship.pod.key));
            ctx.drawImage(shipSprite({ windows, paint: paintFor(ship.pod.key), thrust, frame }), ship.x, ship.y + bob);
          }
          return;
        }
        const warp = sc.pod.crew.some((c) => busyLook(sc, c.session.id));
        drawBridge(ctx, now, w, h, sc.layout.viewportH, warp);
        for (const top of sc.layout.rowTops) {
          const row = sc.layout.seats.filter((s) => s.y - top >= 0 && s.y - top < SLOT_H);
          drawConsoleBank(
            ctx,
            now,
            row.map((s) => ({
              x: s.x + 1,
              w: LOBSTER_W - 2,
              color: sc.looks.get(s.member.session.id)?.color ?? STATE_COLORS.idle,
            })),
            top,
          );
        }
        for (const seat of sc.layout.seats) {
          const s = seat.member.session;
          const look = sc.looks.get(s.id);
          if (!look) continue;
          ctx.drawImage(
            lobsterSprite({ pose: look.pose, frame: frameAt(look.pose, now, phaseOf(s.id)), color: look.color, captain: true }),
            seat.x,
            seat.y,
          );
          seat.shrimp.forEach((pos, k) => {
            const sub = seat.member.subagents[k];
            const frame = Math.floor(now / 250 + phaseOf(sub.id)) % 2;
            ctx.drawImage(shrimpSprite({ frame, color: sc.looks.get(sub.id)?.color ?? STATE_COLORS.idle }), pos.x, pos.y);
          });
        }
      },
      busy() {
        const sc = sceneRef.current;
        if (!sc) return false;
        for (const look of sc.looks.values()) if (BUSY_POSES.has(look.pose)) return true;
        return false;
      },
    });
    // Sizing happens in the effect above, once the zoom for this size is known.
    const fit = () => setHost({ w: hostEl.clientWidth, h: hostEl.clientHeight, dpr: window.devicePixelRatio || 1 });
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(hostEl);
    engine.start();
    return () => {
      ro.disconnect();
      engine.stop();
      engineRef.current = null;
    };
  }, []);

  // New data: redraw now rather than waiting for the idle frame.
  useEffect(() => engineRef.current?.invalidate(), [scene]);

  // Escape leaves the bridge.
  useEffect(() => {
    if (!boarded) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setBoarded(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [boarded]);

  const k = px?.cssPerPx ?? 1;
  const at = (x: number, y: number) => ({ left: x * k, top: y * k });
  const titleOf = (s: SessionInfo) =>
    preferredSessionTitle(s) ?? s.slug ?? s.lastMessagePreview ?? s.id.slice(0, 8);
  const alertBadge = (alert: PendingDecisionState, style?: React.CSSProperties) =>
    alert === "none" ? null : (
      <span
        style={style}
        className={alert === "parked" ? `${styles.alert} ${styles.alertParked}` : styles.alert}
        title={alert === "parked" ? t("lobsters.parked") : t("lobsters.waiting")}
      >
        !
      </span>
    );

  return (
    <PageShell view="lobsters" title={t("view_lobsters", "龙虾舰队")}>
      <div ref={hostRef} className={styles.stage}>
        <canvas ref={canvasRef} className={styles.canvas} />

        {scene?.kind === "fleet" &&
          scene.layout.ships.map((ship) => (
            <button
              key={ship.pod.key}
              type="button"
              className={styles.ship}
              style={{ ...at(ship.x - (SHIP_CELL_W - SHIP_W) / 2, ship.y - 4), width: SHIP_CELL_W * k, height: (SHIP_H + 8) * k }}
              title={`${ship.pod.key}\n${t("lobsters.board")}`}
              onClick={() => setBoarded(ship.pod.key)}
            >
              {/* Over the canopy, so it cannot be read as the ship above's. */}
              {alertBadge(shipAlert(ship.pod, scene.looks), {
                left: ((SHIP_CELL_W - SHIP_W) / 2 + SHIP_CANOPY_X) * k,
                top: 4 * k - 12,
              })}
              <span className={styles.plate} style={{ top: (SHIP_H + 8) * k }}>
                <span className={styles.plateName}>{ship.pod.key}</span>
                <span className={styles.plateCrew}>{t("lobsters.crew", { n: ship.pod.crew.length })}</span>
              </span>
            </button>
          ))}
        {scene?.kind === "fleet" && scene.layout.hidden > 0 && (
          <div className={styles.more}>{t("lobsters.more_ships", { n: scene.layout.hidden })}</div>
        )}
        {scene?.kind === "fleet" && scene.layout.ships.length === 0 && (
          <div className={styles.empty}>{t("lobsters.empty")}</div>
        )}

        {scene?.kind === "bridge" && (
          <>
            <div className={styles.bridgeBar} style={{ ...at(8, 6) }}>
              <button type="button" className={styles.back} onClick={() => setBoarded(null)}>
                ← {t("lobsters.back")}
              </button>
              <span className={styles.bridgeName}>{scene.pod.key}</span>
              <span className={styles.plateCrew}>{t("lobsters.crew", { n: scene.pod.crew.length })}</span>
            </div>
            {scene.layout.seats.map((seat) => {
              const s = seat.member.session;
              const look = scene.looks.get(s.id);
              const title = titleOf(s);
              const subs = seat.member.subagents.length;
              return (
                <button
                  key={s.id}
                  type="button"
                  className={styles.seat}
                  style={{ ...at(seat.x, seat.y), width: LOBSTER_W * k, height: (SLOT_H - 1) * k }}
                  title={`${title}\n${t("lobsters.open_title")}`}
                  onClick={() => requestOpenTask(s.id)}
                >
                  {look && alertBadge(look.alert)}
                  <span className={styles.name} style={{ top: LOBSTER_H * k, height: NAME_H * k, lineHeight: `${NAME_H * k}px` }}>
                    {title}
                  </span>
                  {subs > MAX_SHRIMP && (
                    <span className={styles.crewBadge} title={t("lobsters.subagents", { n: subs })}>
                      ×{subs}
                    </span>
                  )}
                </button>
              );
            })}
            {scene.layout.more > 0 && (
              <div className={styles.more}>{t("lobsters.more_crew", { n: scene.layout.more })}</div>
            )}
          </>
        )}
      </div>
    </PageShell>
  );
}
