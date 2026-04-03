import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useOverlayStore, useSessionsStore } from "../store";
import { getItem } from "../storage";
import type { SessionInfo } from "../types";
import { RobotFrame } from "./RobotFrame";
import { useMood } from "./useMood";
import styles from "./MascotEyes.module.css";

// ── Dynamic quip generation ─────────────────────────────────────────────────

const QUIP_REGEN_INTERVAL = 30 * 60 * 1000; // 30 minutes
const MIN_TITLES_FOR_GENERATION = 1;

// ── Mood types ───────────────────────────────────────────────────────────────

export type MascotMood =
  | "excited" | "focused" | "anxious"
  | "satisfied" | "bored" | "lonely" | "sleepy"
  | "attentive" | "proud" | "frustrated" | "embarrassed";

// Mood derivation is now in useMood.ts

// ── Quip keys ────────────────────────────────────────────────────────────────

const QUIP_COUNT = 15;
const CLICK_QUIP_COUNT = 30;

function makeKeys(prefix: string, count: number): string[] {
  return Array.from({ length: count }, (_, i) => `mascot.${prefix}_${i + 1}`);
}

const QUIP_KEYS: Record<MascotMood, string[]> = {
  excited:     makeKeys("excited", QUIP_COUNT),
  focused:     makeKeys("focused", QUIP_COUNT),
  anxious:     makeKeys("anxious", QUIP_COUNT),
  satisfied:   makeKeys("satisfied", QUIP_COUNT),
  bored:       makeKeys("bored", QUIP_COUNT),
  lonely:      makeKeys("lonely", QUIP_COUNT),
  sleepy:      makeKeys("sleepy", QUIP_COUNT),
  attentive:   makeKeys("attentive", QUIP_COUNT),
  proud:       makeKeys("proud", QUIP_COUNT),
  frustrated:  makeKeys("frustrated", QUIP_COUNT),
  embarrassed: makeKeys("embarrassed", QUIP_COUNT),
};

// ── Click response quips ─────────────────────────────────────────────────────

const CLICK_QUIP_KEYS: Record<MascotMood, string[]> = {
  excited:     makeKeys("click_excited", CLICK_QUIP_COUNT),
  focused:     makeKeys("click_focused", CLICK_QUIP_COUNT),
  anxious:     makeKeys("click_anxious", CLICK_QUIP_COUNT),
  satisfied:   makeKeys("click_satisfied", CLICK_QUIP_COUNT),
  bored:       makeKeys("click_bored", CLICK_QUIP_COUNT),
  lonely:      makeKeys("click_lonely", CLICK_QUIP_COUNT),
  sleepy:      makeKeys("click_sleepy", CLICK_QUIP_COUNT),
  attentive:   makeKeys("click_attentive", CLICK_QUIP_COUNT),
  proud:       makeKeys("click_proud", CLICK_QUIP_COUNT),
  frustrated:  makeKeys("click_frustrated", CLICK_QUIP_COUNT),
  embarrassed: makeKeys("click_embarrassed", CLICK_QUIP_COUNT),
};

// ── Eye shape: just solid circle + eyelid positions ──────────────────────────

interface EyeShape {
  rx: number;
  ry: number;
  lidTop: number;       // 0–1 top eyelid closure
  lidBot: number;       // 0–1 bottom eyelid closure
  rightLidTop?: number;
  rightRy?: number;
  gazeX?: number;       // directional gaze bias
  gazeY?: number;
  eyeShape?: "star" | "cross" | "diamond" | "spiral" | "heart";
}

const EYE_VARIANTS: Record<MascotMood, EyeShape[]> = {
  excited: [
    { rx: 20, ry: 20, lidTop: 0.05, lidBot: 0 },            // wide open, energized
    { rx: 20, ry: 18, lidTop: 0.08, lidBot: 0 },
    { rx: 20, ry: 20, lidTop: 0.05, lidBot: 0, rightLidTop: 0.85 },  // wink
    { rx: 20, ry: 19, lidTop: 0.06, lidBot: 0, gazeY: 0.4 },
  ],
  focused: [
    { rx: 16, ry: 16, lidTop: 0.15, lidBot: 0.08 },         // slightly narrowed, concentrating
    { rx: 16, ry: 15, lidTop: 0.18, lidBot: 0.10 },
    { rx: 16, ry: 16, lidTop: 0.15, lidBot: 0.08, rightLidTop: 0.25 },
    { rx: 16, ry: 15, lidTop: 0.20, lidBot: 0.10, gazeY: 0.5 },  // looking down at work
  ],
  anxious: [
    { rx: 19, ry: 19, lidTop: 0.04, lidBot: 0 },
    { rx: 19, ry: 20, lidTop: 0.04, lidBot: 0 },
    { rx: 19, ry: 19, lidTop: 0.04, lidBot: 0, rightLidTop: 0.15 },
    { rx: 19, ry: 20, lidTop: 0.04, lidBot: 0, gazeY: -0.5 },
  ],
  satisfied: [
    { rx: 18, ry: 16, lidTop: 0.30, lidBot: 0.25 },   // happy squint
    { rx: 18, ry: 16, lidTop: 0.40, lidBot: 0.32 },   // crescent ^_^
    { rx: 18, ry: 16, lidTop: 0.28, lidBot: 0.22, rightLidTop: 0.85 },  // wink
    { rx: 18, ry: 16, lidTop: 0.45, lidBot: 0.38 },   // super happy ^^
  ],
  bored: [
    { rx: 18, ry: 18, lidTop: 0.38, lidBot: 0, gazeY: -0.6 },     // eye roll up
    { rx: 18, ry: 16, lidTop: 0.45, lidBot: 0, gazeY: -0.8 },     // heavy eye roll
    { rx: 18, ry: 18, lidTop: 0.32, lidBot: 0, gazeX: 0.7 },      // looking away
    { rx: 18, ry: 18, lidTop: 0.35, lidBot: 0, gazeY: -0.5, rightLidTop: 0.55 },
  ],
  lonely: [
    { rx: 19, ry: 20, lidTop: 0, lidBot: 0 },          // big puppy
    { rx: 20, ry: 21, lidTop: 0, lidBot: 0 },           // watery big
    { rx: 19, ry: 20, lidTop: 0.04, lidBot: 0, gazeY: 0.4 },  // looking down
    { rx: 19, ry: 20, lidTop: 0, lidBot: 0, gazeX: 0.6 },     // looking away
  ],
  sleepy: [
    { rx: 18, ry: 10, lidTop: 0.62, lidBot: 0 },
    { rx: 18, ry: 8,  lidTop: 0.72, lidBot: 0 },       // nearly shut
    { rx: 18, ry: 10, lidTop: 0.58, lidBot: 0, rightLidTop: 0.80 },
    { rx: 18, ry: 6,  lidTop: 0.78, lidBot: 0 },       // barely a slit
  ],
  attentive: [
    { rx: 20, ry: 20, lidTop: 0.02, lidBot: 0 },       // wide open, curious
    { rx: 20, ry: 21, lidTop: 0, lidBot: 0, eyeShape: "diamond" },  // diamond sparkle eyes
    { rx: 20, ry: 20, lidTop: 0.02, lidBot: 0, gazeY: -0.3 }, // looking up
    { rx: 20, ry: 20, lidTop: 0, lidBot: 0, eyeShape: "diamond" },
  ],
  proud: [
    { rx: 18, ry: 18, lidTop: 0, lidBot: 0, eyeShape: "star" },       // ★ star eyes!
    { rx: 18, ry: 14, lidTop: 0.40, lidBot: 0.35 },                   // smug ^^
    { rx: 18, ry: 14, lidTop: 0.32, lidBot: 0.28, rightLidTop: 0.85 }, // proud wink
    { rx: 18, ry: 18, lidTop: 0, lidBot: 0, eyeShape: "star", gazeY: 0.3 }, // ★
  ],
  frustrated: [
    { rx: 17, ry: 14, lidTop: 0.30, lidBot: 0 },       // glaring
    { rx: 17, ry: 17, lidTop: 0, lidBot: 0, eyeShape: "cross" },  // X eyes!
    { rx: 17, ry: 17, lidTop: 0, lidBot: 0, eyeShape: "cross", gazeY: -0.4 },
    { rx: 17, ry: 13, lidTop: 0.32, lidBot: 0, rightLidTop: 0.45 },
  ],
  embarrassed: [
    { rx: 18, ry: 18, lidTop: 0, lidBot: 0, eyeShape: "spiral" },  // @_@ spiral eyes
    { rx: 16, ry: 13, lidTop: 0.30, lidBot: 0.20, gazeX: -0.8 },  // looking away left
    { rx: 18, ry: 18, lidTop: 0, lidBot: 0, eyeShape: "spiral" },  // @_@
    { rx: 16, ry: 13, lidTop: 0.25, lidBot: 0.15, gazeX: 0.8, gazeY: 0.5 }, // averted down-right
  ],
};

// ── Special actions ──────────────────────────────────────────────────────────

type SpecialAction =
  | "none" | "yawn" | "lookAround" | "nod" | "bounce"
  | "dozeOff" | "sigh" | "wiggle" | "stretch" | "spin" | "fistPump"
  | "eureka" | "typing" | "steam" | "meltdown" | "startle"
  | "sunglasses" | "victoryPose" | "phoneLook" | "doodle" | "wave"
  | "huddle" | "sleepBubble" | "headSnap";

const MOOD_ACTIONS: Record<MascotMood, SpecialAction[]> = {
  excited:     ["bounce", "bounce", "spin", "fistPump"],
  focused:     ["typing", "typing", "eureka", "nod"],
  anxious:     ["lookAround", "lookAround", "startle", "startle"],
  satisfied:   ["nod", "wiggle", "stretch", "sunglasses", "victoryPose"],
  bored:       ["yawn", "lookAround", "phoneLook", "doodle"],
  lonely:      ["sigh", "sigh", "wave", "huddle"],
  sleepy:      ["dozeOff", "dozeOff", "sleepBubble", "headSnap"],
  attentive:   ["lookAround", "bounce", "wave", "nod"],
  proud:       ["victoryPose", "fistPump", "sunglasses", "spin"],
  frustrated:  ["steam", "steam", "meltdown", "startle"],
  embarrassed: ["lookAround", "sigh", "huddle", "nod"],
};

const ACTION_DURATIONS: Partial<Record<SpecialAction, number>> = {
  yawn: 2500, dozeOff: 3000, spin: 1200, sleepBubble: 3500, headSnap: 2000,
  steam: 2500, meltdown: 2000, doodle: 2000, huddle: 2500, wave: 2000, eureka: 1800,
};

// ── Gaze wander config ───────────────────────────────────────────────────────

const GAZE_WANDER: Record<MascotMood, { range: number; interval: [number, number]; dirBlend: number }> = {
  excited:     { range: 3,   interval: [1000, 2000], dirBlend: 0.2 },
  focused:     { range: 2,   interval: [2000, 4000], dirBlend: 0.15 },
  anxious:     { range: 5,   interval: [400, 1200],  dirBlend: 0.4 },
  satisfied:   { range: 2,   interval: [2500, 5000], dirBlend: 0.2 },
  bored:       { range: 4,   interval: [1500, 3500], dirBlend: 0.3 },
  lonely:      { range: 2.5, interval: [2000, 4000], dirBlend: 0.2 },
  sleepy:      { range: 1,   interval: [3000, 6000], dirBlend: 0.1 },
  attentive:   { range: 3,   interval: [800, 1500],  dirBlend: 0.35 },  // alert, looking around
  proud:       { range: 1.5, interval: [2500, 5000], dirBlend: 0.15 },  // calm confidence
  frustrated:  { range: 2,   interval: [600, 1200],  dirBlend: 0.3 },   // twitchy
  embarrassed: { range: 3,   interval: [1500, 3000], dirBlend: 0.25 },  // avoidant
};

// ── Geometry ─────────────────────────────────────────────────────────────────

const EYE_LEFT_CX = 65;
const EYE_RIGHT_CX = 135;
const EYE_CY = 40;
const EYE_COLOR = "var(--color-accent)";   // brand accent color

// ── Component ────────────────────────────────────────────────────────────────

export function MascotEyes({ embedded, onQuip }: { embedded?: boolean; onQuip?: (text: string | null) => void } = {}) {
  const { t, i18n } = useTranslation();
  const { sessions } = useSessionsStore();
  const mood = useMood(sessions);
  const [expanded, setExpanded] = useState(true);

  const [isBlinking, setIsBlinking] = useState(false);
  const [isDoubleBlink, setIsDoubleBlink] = useState(false);
  const [gazeOffset, setGazeOffset] = useState({ x: 0, y: 0 });
  const [sizePulse, setSizePulse] = useState(0);
  const [eyeVariant, setEyeVariant] = useState(0);
  const [quipIndex, setQuipIndex] = useState(0);
  const [specialAction, setSpecialAction] = useState<SpecialAction>("none");
  const [bodyBobPhase, setBodyBobPhase] = useState(0);
  const [moodJiggle, setMoodJiggle] = useState(false);
  const [clickReaction, setClickReaction] = useState(false);
  const [clickQuipKey, setClickQuipKey] = useState("");
  const [generatedBusyQuips, setGeneratedBusyQuips] = useState<string[]>([]);
  const [generatedIdleQuips, setGeneratedIdleQuips] = useState<string[]>([]);

  const blinkTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const gazeTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const pulseTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const quipTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const eyeVarTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const actionTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const actionEndTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const bobTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const clickTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const clickCooldown = useRef(false);

  // ── Blink ──────────────────────────────────────────────────────────────────
  useEffect(() => {
    function scheduleBlink() {
      const delay = 2000 + Math.random() * 4000;
      blinkTimer.current = setTimeout(() => {
        setIsBlinking(true);
        if (Math.random() < 0.25) {
          setIsDoubleBlink(true);
          setTimeout(() => setIsBlinking(false), 120);
          setTimeout(() => setIsBlinking(true), 250);
          setTimeout(() => { setIsBlinking(false); setIsDoubleBlink(false); }, 380);
        } else {
          setTimeout(() => setIsBlinking(false), 150);
        }
        scheduleBlink();
      }, delay);
    }
    scheduleBlink();
    return () => clearTimeout(blinkTimer.current);
  }, []);

  // ── Gaze wander (moves entire eye position) ───────────────────────────────
  useEffect(() => {
    const cfg = GAZE_WANDER[mood];
    function scheduleMove() {
      const delay = cfg.interval[0] + Math.random() * (cfg.interval[1] - cfg.interval[0]);
      gazeTimer.current = setTimeout(() => {
        setGazeOffset({
          x: (Math.random() - 0.5) * cfg.range * 2,
          y: (Math.random() - 0.5) * cfg.range,
        });
        scheduleMove();
      }, delay);
    }
    scheduleMove();
    return () => clearTimeout(gazeTimer.current);
  }, [mood]);

  // ── Size pulse (subtle breathing) ──────────────────────────────────────────
  useEffect(() => {
    function schedulePulse() {
      const delay = 3000 + Math.random() * 5000;
      pulseTimer.current = setTimeout(() => {
        const d = mood === "lonely" ? Math.random() * 2 :
                  (Math.random() - 0.5) * 1.5;
        setSizePulse(d);
        schedulePulse();
      }, delay);
    }
    schedulePulse();
    return () => clearTimeout(pulseTimer.current);
  }, [mood]);

  // ── Eye variant rotation ───────────────────────────────────────────────────
  useEffect(() => {
    setEyeVariant(0);
    function scheduleVariant() {
      const delay = 4000 + Math.random() * 6000;
      eyeVarTimer.current = setTimeout(() => {
        setEyeVariant((v) => (v + 1) % EYE_VARIANTS[mood].length);
        scheduleVariant();
      }, delay);
    }
    scheduleVariant();
    return () => clearTimeout(eyeVarTimer.current);
  }, [mood]);

  // ── Quip rotation ──────────────────────────────────────────────────────────
  useEffect(() => {
    setQuipIndex(Math.floor(Math.random() * QUIP_COUNT));
    function rotateQuip() {
      quipTimer.current = setTimeout(() => {
        setQuipIndex((i) => (i + 1) % QUIP_COUNT);
        rotateQuip();
      }, 8000 + Math.random() * 4000);
    }
    rotateQuip();
    return () => clearTimeout(quipTimer.current);
  }, [mood]);

  // ── Periodic special actions ───────────────────────────────────────────────
  useEffect(() => {
    setSpecialAction("none");
    function scheduleAction() {
      const delay = 10000 + Math.random() * 10000;
      actionTimer.current = setTimeout(() => {
        const actions = MOOD_ACTIONS[mood];
        const action = actions[Math.floor(Math.random() * actions.length)];
        setSpecialAction(action);
        const duration = ACTION_DURATIONS[action] ?? 1500;
        actionEndTimer.current = setTimeout(() => setSpecialAction("none"), duration);
        scheduleAction();
      }, delay);
    }
    scheduleAction();
    return () => { clearTimeout(actionTimer.current); clearTimeout(actionEndTimer.current); };
  }, [mood]);

  // ── Body bob ───────────────────────────────────────────────────────────────
  useEffect(() => {
    let frame = 0;
    function tick() {
      frame++;
      setBodyBobPhase(Math.sin(frame * 0.04) * 1.5);
      bobTimer.current = setTimeout(tick, 50);
    }
    tick();
    return () => clearTimeout(bobTimer.current);
  }, []);

  // ── Mood jiggle ────────────────────────────────────────────────────────────
  useEffect(() => {
    setMoodJiggle(true);
    const t1 = setTimeout(() => setMoodJiggle(false), 400);
    return () => clearTimeout(t1);
  }, [mood]);

  // ── Dynamic quip generation ────────────────────────────────────────────────
  const fetchQuips = useCallback(async (currentSessions: SessionInfo[]) => {
    if (getItem("personalized-mascot") === "false") return;
    const busyStatuses = ["thinking", "executing", "streaming", "processing", "active", "delegating"];
    // Recent non-subagent sessions sorted by creation time (newest first)
    const recentMain = [...currentSessions]
      .filter((s) => !s.isSubagent && s.aiTitle)
      .sort((a, b) => b.createdAtMs - a.createdAtMs)
      .slice(0, 10);

    const busyTitles = recentMain
      .filter((s) => busyStatuses.includes(s.status))
      .map((s) => s.aiTitle!);
    const doneTitles = recentMain
      .filter((s) => !busyStatuses.includes(s.status))
      .map((s) => s.aiTitle!);

    if (busyTitles.length + doneTitles.length < MIN_TITLES_FOR_GENERATION) return;

    try {
      const result = await invoke<{ busy: string[]; idle: string[] }>("generate_mascot_quips", {
        busyTitles,
        doneTitles,
        locale: i18n.language,
      });
      if (result.busy.length > 0) setGeneratedBusyQuips(result.busy);
      if (result.idle.length > 0) setGeneratedIdleQuips(result.idle);
    } catch {
      // CLI not available or failed — silently ignore
    }
  }, [i18n.language]);

  // Periodic regeneration + on session count change
  const lastSessionCount = useRef(0);
  const lastGenTime = useRef(0);

  useEffect(() => {
    const busyStatuses = ["thinking", "executing", "streaming", "processing", "active", "delegating"];
    const hasActiveSessions = sessions.some((s) => busyStatuses.includes(s.status));
    if (!hasActiveSessions) return;

    const now = Date.now();
    const sessionCount = sessions.length;
    const countChanged = Math.abs(sessionCount - lastSessionCount.current) >= 2;
    const timeExpired = now - lastGenTime.current > QUIP_REGEN_INTERVAL;

    if (countChanged || timeExpired) {
      lastSessionCount.current = sessionCount;
      lastGenTime.current = now;
      fetchQuips(sessions);
    }

    // Also set up a periodic timer (only while active sessions exist)
    const interval = setInterval(() => {
      if (document.visibilityState === "hidden") return;
      lastGenTime.current = Date.now();
      fetchQuips(sessions);
    }, QUIP_REGEN_INTERVAL);

    return () => clearInterval(interval);
  }, [sessions.length, fetchQuips, sessions]);

  // ── Click handler ──────────────────────────────────────────────────────────
  const handleMascotClick = () => {
    if (clickCooldown.current || clickReaction) return;

    const keys = CLICK_QUIP_KEYS[mood];
    const key = keys[Math.floor(Math.random() * keys.length)];

    setClickReaction(true);
    setClickQuipKey(key);
    setMoodJiggle(true);
    setTimeout(() => setMoodJiggle(false), 400);
    clickCooldown.current = true;

    clickTimer.current = setTimeout(() => {
      setClickReaction(false);
      setClickQuipKey("");
    }, 3500);

    setTimeout(() => { clickCooldown.current = false; }, 5000);
  };

  // Clean up click timer
  useEffect(() => () => clearTimeout(clickTimer.current), []);

  // ── Derived values ─────────────────────────────────────────────────────────

  const variants = EYE_VARIANTS[mood];
  const baseShape = variants[eyeVariant % variants.length];
  // Override eye shape to heart during click reaction
  const shape = clickReaction ? { ...baseShape, eyeShape: "heart" as const } : baseShape;
  // Quip text: mix i18n keys with generated quips (busy or idle group based on mood).
  const busyMoods: MascotMood[] = ["excited", "focused", "anxious", "attentive", "frustrated"];
  const generatedQuips = busyMoods.includes(mood) ? generatedBusyQuips : generatedIdleQuips;

  const quipText = useMemo(() => {
    if (clickReaction && clickQuipKey) {
      return t(clickQuipKey);
    }
    // Normal quip rotation: alternate between i18n and generated
    const keys = QUIP_KEYS[mood];
    const i18nText = t(keys[quipIndex % keys.length]);
    if (generatedQuips.length > 0 && quipIndex % 2 === 1) {
      return generatedQuips[Math.floor(quipIndex / 2) % generatedQuips.length];
    }
    return i18nText;
  }, [clickReaction, clickQuipKey, mood, quipIndex, generatedQuips, t]);

  const isYawning = specialAction === "yawn";
  const isDozing = specialAction === "dozeOff";
  const isBouncing = specialAction === "bounce";
  const isLookingAround = specialAction === "lookAround";
  const isSighing = specialAction === "sigh";
  const isEureka = specialAction === "eureka";

  const yawnLidExtra = isYawning ? 0.35 : 0;
  const dozeLidExtra = isDozing ? 0.55 : 0;
  const eurekaLidReduction = isEureka ? -0.12 : 0;

  const wanderCfg = GAZE_WANDER[mood];
  const effectiveGaze = isLookingAround
    ? { x: Math.sin(Date.now() * 0.01) * 6, y: 0 }
    : shape.gazeX !== undefined || shape.gazeY !== undefined
    ? {
        x: (shape.gazeX ?? 0) * 5 + gazeOffset.x * wanderCfg.dirBlend,
        y: (shape.gazeY ?? 0) * 5 + gazeOffset.y * wanderCfg.dirBlend,
      }
    : gazeOffset;

  const bodyTranslateY = bodyBobPhase
    + (isBouncing ? Math.sin(Date.now() * 0.02) * 3 : 0)
    + (isSighing ? 2 : 0);

  // ── Special eye shape SVG paths ──────────────────────────────────────────────

  const renderSpecialEyeShape = (cx: number, cy: number, size: number, type: "star" | "cross" | "diamond" | "spiral" | "heart") => {
    switch (type) {
      case "star": {
        // 5-pointed star
        const pts: string[] = [];
        for (let i = 0; i < 10; i++) {
          const r = i % 2 === 0 ? size : size * 0.45;
          const angle = (Math.PI / 2) + (i * Math.PI / 5);
          pts.push(`${cx + r * Math.cos(angle)},${cy - r * Math.sin(angle)}`);
        }
        return (
          <g className={styles.eyeShape_star}>
            <polygon points={pts.join(" ")} fill={EYE_COLOR} />
            <polygon points={pts.join(" ")} fill="url(#grad-special)" opacity={0.4} />
          </g>
        );
      }
      case "cross": {
        // Bold X shape
        const w = size * 0.3;
        return (
          <g className={styles.eyeShape_cross}>
            <line x1={cx - size} y1={cy - size} x2={cx + size} y2={cy + size}
              stroke={EYE_COLOR} strokeWidth={w} strokeLinecap="round" />
            <line x1={cx + size} y1={cy - size} x2={cx - size} y2={cy + size}
              stroke={EYE_COLOR} strokeWidth={w} strokeLinecap="round" />
          </g>
        );
      }
      case "diamond": {
        // 4-pointed diamond/sparkle
        const d = `M ${cx},${cy - size} L ${cx + size * 0.5},${cy} L ${cx},${cy + size} L ${cx - size * 0.5},${cy} Z`;
        return (
          <g className={styles.eyeShape_diamond}>
            <path d={d} fill={EYE_COLOR} />
            <path d={d} fill="url(#grad-special)" opacity={0.3} />
            {/* Small sparkle lines */}
            <line x1={cx - size * 0.7} y1={cy} x2={cx - size * 0.4} y2={cy}
              stroke={EYE_COLOR} strokeWidth={1} opacity={0.6} />
            <line x1={cx + size * 0.4} y1={cy} x2={cx + size * 0.7} y2={cy}
              stroke={EYE_COLOR} strokeWidth={1} opacity={0.6} />
          </g>
        );
      }
      case "spiral": {
        // Archimedean spiral
        const pts: string[] = [];
        const turns = 2.5;
        const steps = 60;
        for (let i = 0; i <= steps; i++) {
          const t = (i / steps) * turns * Math.PI * 2;
          const r = (i / steps) * size;
          pts.push(`${cx + r * Math.cos(t)},${cy + r * Math.sin(t)}`);
        }
        return (
          <g className={styles.eyeShape_spiral}>
            <polyline points={pts.join(" ")} fill="none"
              stroke={EYE_COLOR} strokeWidth={2.5} strokeLinecap="round" />
          </g>
        );
      }
      case "heart": {
        // Classic heart: round lobes at top, rounded point at bottom
        const sc = size / 16;
        // Bottom tip uses a small arc for rounded corner instead of sharp point
        const br = 2 * sc; // bottom rounding radius
        const d = `M ${cx},${cy + (14 - br) * sc}
          C ${cx - 2 * sc},${cy + 10 * sc} ${cx - 16 * sc},${cy + 4 * sc} ${cx - 16 * sc},${cy - 4 * sc}
          C ${cx - 16 * sc},${cy - 12 * sc} ${cx - 8 * sc},${cy - 14 * sc} ${cx},${cy - 6 * sc}
          C ${cx + 8 * sc},${cy - 14 * sc} ${cx + 16 * sc},${cy - 12 * sc} ${cx + 16 * sc},${cy - 4 * sc}
          C ${cx + 16 * sc},${cy + 4 * sc} ${cx + 2 * sc},${cy + 10 * sc} ${cx},${cy + (14 - br) * sc} Z`;
        return (
          <g className={styles.eyeShape_heart}>
            <path d={d} fill={EYE_COLOR} strokeLinejoin="round" stroke={EYE_COLOR} strokeWidth={br * 2} />
            <path d={d} fill="url(#grad-special)" opacity={0.35} />
          </g>
        );
      }
    }
  };

  // ── Render single eye (LOOI-style: solid filled circle) ─────────────────────

  const renderEye = (baseCx: number, isRight: boolean) => {
    const clipId = isRight ? "clip-eye-r" : "clip-eye-l";
    const gradId = isRight ? "grad-eye-r" : "grad-eye-l";
    const { rx, ry, lidTop, lidBot, rightLidTop, rightRy } = shape;
    const effectiveRy = isRight && rightRy ? rightRy : ry;
    const baseLidTop = isRight && rightLidTop !== undefined ? rightLidTop : lidTop;

    const blinkLid = isBlinking ? 0.95
      : Math.max(0, Math.min(baseLidTop + yawnLidExtra + dozeLidExtra + eurekaLidReduction, 0.95));
    // When the right eye uses rightLidTop (wink), clamp lidBot so the
    // top + bottom lids don't exceed 0.95 — otherwise the eye disappears.
    const effectiveLidBot = isRight && rightLidTop !== undefined
      ? Math.min(lidBot, Math.max(0, 0.95 - blinkLid))
      : lidBot;

    // Eye position includes gaze offset
    const cx = baseCx + effectiveGaze.x;
    const cy = EYE_CY + effectiveGaze.y;

    // Subtle size pulse
    const pRx = rx + sizePulse * 0.3;
    const pRy = effectiveRy + sizePulse * 0.3;

    // Special eye shapes: skip the ellipse+eyelid system entirely
    if (shape.eyeShape && !isBlinking) {
      const eyeSize = Math.min(pRx, pRy) * 0.9;
      return (
        <g className={moodJiggle ? styles.eyeJiggle : ""}>
          {renderSpecialEyeShape(cx, cy, eyeSize, shape.eyeShape)}
        </g>
      );
    }

    const eyeTop = cy - pRy;
    const lidTopHeight = pRy * 2 * blinkLid;
    const lidBotHeight = pRy * 2 * effectiveLidBot;

    return (
      <g className={moodJiggle ? styles.eyeJiggle : ""}>
        {/* Watery glow for lonely */}
        {mood === "lonely" && (
          <ellipse cx={cx} cy={cy} rx={pRx + 4} ry={pRy + 4}
            fill="rgba(100,180,255,0.06)" className={styles.waterShimmer} />
        )}

        <defs>
          <clipPath id={clipId}>
            <ellipse cx={cx} cy={cy} rx={pRx + 0.5} ry={pRy + 0.5} />
          </clipPath>
          {/* LOOI-style inner shadow: top-to-bottom gradient darkening the lower portion */}
          <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="rgba(0,0,0,0)" />
            <stop offset="55%" stopColor="rgba(0,0,0,0)" />
            <stop offset="100%" stopColor="rgba(60,20,120,0.45)" />
          </linearGradient>
        </defs>

        {/* Eye content — clipped to eye ellipse */}
        <g clipPath={`url(#${clipId})`}>
          {/* Solid accent eye fill */}
          <ellipse cx={cx} cy={cy} rx={pRx} ry={pRy} fill={EYE_COLOR} className={styles.eyeFill} />

          {/* Inner shadow overlay — purple gradient at bottom of eye (3D sphere look) */}
          <ellipse cx={cx} cy={cy} rx={pRx} ry={pRy} fill={`url(#${gradId})`} />

          {/* Top eyelid — slides down from above, background-colored */}
          <rect
            x={cx - pRx - 2} y={eyeTop - 2}
            width={pRx * 2 + 4}
            height={Math.max(0, lidTopHeight + 2)}
            fill="var(--mascot-bg)" className={styles.eyelid}
          />
          {/* Bottom eyelid — rises from below */}
          {effectiveLidBot > 0 && (
            <rect
              x={cx - pRx - 2}
              y={cy + pRy - lidBotHeight}
              width={pRx * 2 + 4}
              height={lidBotHeight + 2}
              fill="var(--mascot-bg)" className={styles.eyelid}
            />
          )}
        </g>
      </g>
    );
  };

  // ── Mouth (LOOI-style small line/curve between eyes) ────────────────────────
  const MOUTH_CX = (EYE_LEFT_CX + EYE_RIGHT_CX) / 2;
  const MOUTH_CY = EYE_CY + shape.ry + 8;

  const renderMouth = () => {
    switch (mood) {
      case "satisfied":
        // wide happy wavy ~~
        return (
          <path
            d={`M ${MOUTH_CX - 10},${MOUTH_CY} q 5,-5 10,0 q 5,5 10,0`}
            fill="none" stroke="var(--color-accent)" strokeWidth={2}
            strokeLinecap="round" opacity={0.75} className={styles.mouth}
          />
        );
      case "excited":
        // big open grin
        return (
          <path
            d={`M ${MOUTH_CX - 12},${MOUTH_CY - 2} q 12,14 24,0`}
            fill="var(--color-accent)" fillOpacity={0.2} stroke="var(--color-accent)" strokeWidth={2.2}
            strokeLinecap="round" opacity={0.85} className={styles.mouth}
          />
        );
      case "focused":
        // asymmetric thinking pout
        return (
          <path
            d={`M ${MOUTH_CX - 6},${MOUTH_CY + 1} q 4,-3 8,-1 q 2,1 4,2`}
            fill="none" stroke="var(--color-accent)" strokeWidth={1.8}
            strokeLinecap="round" opacity={0.6} className={styles.mouth}
          />
        );
      case "anxious":
        // worried squiggle
        return (
          <path
            d={`M ${MOUTH_CX - 8},${MOUTH_CY} q 4,-3 8,0 q 4,3 8,0`}
            fill="none" stroke="var(--color-accent)" strokeWidth={1.8}
            strokeLinecap="round" opacity={0.6} className={styles.mouth}
          />
        );
      case "bored":
        // flat line
        return (
          <line
            x1={MOUTH_CX - 8} y1={MOUTH_CY} x2={MOUTH_CX + 8} y2={MOUTH_CY}
            stroke="var(--color-accent)" strokeWidth={2}
            strokeLinecap="round" opacity={0.55} className={styles.mouth}
          />
        );
      case "lonely":
        // deeper frown
        return (
          <path
            d={`M ${MOUTH_CX - 8},${MOUTH_CY + 2} q 8,-7 16,0`}
            fill="none" stroke="var(--color-accent)" strokeWidth={2}
            strokeLinecap="round" opacity={0.6} className={styles.mouth}
          />
        );
      case "sleepy":
        // open yawn
        return (
          <ellipse
            cx={MOUTH_CX} cy={MOUTH_CY}
            rx={5} ry={4}
            fill="var(--color-accent)" fillOpacity={0.25} stroke="var(--color-accent)" strokeWidth={1.5}
            opacity={0.5} className={styles.mouth}
          />
        );
      case "attentive":
        // open "o" mouth — surprised / curious
        return (
          <ellipse
            cx={MOUTH_CX} cy={MOUTH_CY}
            rx={5} ry={4.5}
            fill="none" stroke="var(--color-accent)" strokeWidth={1.8}
            opacity={0.65} className={styles.mouth}
          />
        );
      case "proud":
        // wide confident grin
        return (
          <path
            d={`M ${MOUTH_CX - 11},${MOUTH_CY - 1} q 11,10 22,0`}
            fill="var(--color-accent)" fillOpacity={0.1} stroke="var(--color-accent)" strokeWidth={2}
            strokeLinecap="round" opacity={0.75} className={styles.mouth}
          />
        );
      case "frustrated":
        // tight scowl
        return (
          <path
            d={`M ${MOUTH_CX - 8},${MOUTH_CY + 3} q 8,-7 16,0`}
            fill="none" stroke="var(--color-accent)" strokeWidth={2.2}
            strokeLinecap="round" opacity={0.7} className={styles.mouth}
          />
        );
      case "embarrassed":
        // lopsided cringe
        return (
          <path
            d={`M ${MOUTH_CX - 8},${MOUTH_CY + 1} q 5,-5 9,-1 q 3,4 7,2`}
            fill="none" stroke="var(--color-accent)" strokeWidth={1.8}
            strokeLinecap="round" opacity={0.6} className={styles.mouth}
          />
        );
      default:
        return null;
    }
  };

  // ── Emoji decoration system ──────────────────────────────────────────────────

  interface EmojiItem {
    emoji: string;
    x: number;
    y: number;
    size: number;
    anim: string;  // CSS class name from styles
  }

  const MOOD_EMOJIS: Record<MascotMood, EmojiItem[]> = {
    excited: [],
    focused: [],
    anxious: [],
    satisfied: [],
    bored: [],
    lonely: [],
    sleepy: [],
    attentive: [],
    proud: [],
    frustrated: [],
    embarrassed: [],
  };

  const renderEmojis = () => {
    const items = (MOOD_EMOJIS as Record<string, EmojiItem[]>)[mood] ?? [];
    const elements: React.ReactNode[] = [];

    for (let i = 0; i < items.length; i++) {
      const { emoji, x, y, size, anim } = items[i];
      elements.push(
        <text
          key={`emoji-${i}`}
          x={x} y={y}
          fontSize={size}
          className={styles[anim] ?? ""}
          textAnchor="middle"
          dominantBaseline="central"
        >{emoji}</text>
      );
    }

    // Keep sunglasses SVG overlay for satisfied mood
    if (mood === "satisfied" && specialAction === "sunglasses") {
      elements.push(
        <g key="shades" className={styles.sunglassesDrop}>
          <rect x={EYE_LEFT_CX - 16} y={EYE_CY - 10} width={28} height={16} rx={4} fill="rgba(30,30,40,0.85)" />
          <rect x={EYE_RIGHT_CX - 12} y={EYE_CY - 10} width={28} height={16} rx={4} fill="rgba(30,30,40,0.85)" />
          <line x1={EYE_LEFT_CX + 12} y1={EYE_CY} x2={EYE_RIGHT_CX - 12} y2={EYE_CY}
            stroke="rgba(30,30,40,0.85)" strokeWidth={2.5} />
        </g>
      );
    }

    // Keep sleep bubbles for sleepy+sleepBubble action
    if (mood === "sleepy" && specialAction === "sleepBubble") {
      elements.push(
        <circle key="b1" cx={112} cy={52} r={2} fill="none" stroke="rgba(200,200,220,0.5)" strokeWidth={0.8} className={styles.sleepBubbleSmall} />,
        <circle key="b2" cx={118} cy={44} r={4} fill="none" stroke="rgba(200,200,220,0.4)" strokeWidth={0.8} className={styles.sleepBubbleMed} />,
        <circle key="b3" cx={126} cy={32} r={7} fill="none" stroke="rgba(200,200,220,0.35)" strokeWidth={1} className={styles.sleepBubbleBig} />,
      );
    }

    // Keep startle exclamation
    if (mood === "anxious" && specialAction === "startle") {
      elements.push(
        <text key="excl" x={150} y={14} fontSize="14" fill="#fbbf24" fontWeight="bold" className={styles.startleMark}>!</text>,
      );
    }

    // Keep embarrassed blush circles (these look good as subtle SVG)
    if (mood === "embarrassed") {
      elements.push(
        <g key="blush" opacity={0.35}>
          <ellipse cx={EYE_LEFT_CX - 8} cy={EYE_CY + 14} rx={8} ry={4} fill="#f87171" />
          <ellipse cx={EYE_RIGHT_CX + 8} cy={EYE_CY + 14} rx={8} ry={4} fill="#f87171" />
        </g>
      );
    }

    // Keep frustrated steam for steam action
    if (mood === "frustrated" && specialAction === "steam") {
      elements.push(
        <g key="steam-circles" opacity={0.5}>
          <circle cx={50} cy={14} r={3} fill="none" stroke="rgba(200,200,200,0.6)" strokeWidth={1} className={styles.steamLeft1} />
          <circle cx={46} cy={8} r={4} fill="none" stroke="rgba(200,200,200,0.5)" strokeWidth={1} className={styles.steamLeft2} />
          <circle cx={150} cy={14} r={3} fill="none" stroke="rgba(200,200,200,0.6)" strokeWidth={1} className={styles.steamRight1} />
          <circle cx={154} cy={8} r={4} fill="none" stroke="rgba(200,200,200,0.5)" strokeWidth={1} className={styles.steamRight2} />
        </g>
      );
    }

    // Click reaction: emoji burst
    if (clickReaction) {
      const burstEmojis = ["💕", "✨", "💖", "⭐", "💗"];
      const burstPositions = [
        { x: 60, y: 20 }, { x: 140, y: 15 }, { x: 100, y: 5 },
        { x: 50, y: 8 }, { x: 150, y: 25 },
      ];
      burstEmojis.forEach((emoji, i) => {
        elements.push(
          <text
            key={`burst-${i}`}
            x={burstPositions[i].x}
            y={burstPositions[i].y}
            fontSize={11}
            className={styles[`clickBurstEmoji${i + 1}`]}
            textAnchor="middle"
            dominantBaseline="central"
          >{emoji}</text>
        );
      });
    }

    return elements.length > 0 ? <>{elements}</> : null;
  };

  // ── Render ─────────────────────────────────────────────────────────────────

  const mascotClasses = [
    styles.mascot,
    styles[mood],
    specialAction !== "none" && !clickReaction ? styles[`action_${specialAction}`] : "",
    clickReaction ? styles.clickReaction : "",
    isDoubleBlink ? styles.doubleBlink : "",
  ].filter(Boolean).join(" ");

  const quipClasses = [
    styles.quip,
    clickReaction ? styles.clickQuip : "",
  ].filter(Boolean).join(" ");

  // Notify parent of quip changes (used by overlay to render bubble externally)
  useEffect(() => {
    onQuip?.(quipText || null);
  }, [quipText, onQuip]);

  // Embedded mode: just the SVG, no toggle/quip wrapper
  if (embedded) {
    return (
      <div className={mascotClasses} onClick={handleMascotClick} style={{ background: "transparent" }}>
        <svg viewBox="0 -14 200 114" className={styles.svg}>
          <defs>
            <linearGradient id="grad-special" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="rgba(255,255,255,0.3)" />
              <stop offset="100%" stopColor="rgba(0,0,0,0.2)" />
            </linearGradient>
          </defs>
          <rect x="0" y="-14" width="200" height="128" fill="var(--mascot-bg)" className={styles.bg} />
          <g style={{ transform: `translateY(${bodyTranslateY}px)` }} className={styles.bodyGroup}>
            {renderEye(EYE_LEFT_CX, false)}
            {renderEye(EYE_RIGHT_CX, true)}
            {renderMouth()}
          </g>
          {renderEmojis()}
        </svg>
      </div>
    );
  }

  const overlayEnabled = useOverlayStore((s) => s.enabled);

  // When overlay is active, show a compact "find assistant" placeholder instead
  if (overlayEnabled) {
    return (
      <div className={styles.container}>
        <div className={styles.overlayPlaceholder}>
          <button
            className={styles.findBtn}
            onClick={() => invoke("center_overlay").catch(() => {})}
          >
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="7" cy="7" r="5" />
              <path d="M14 14l-3.5-3.5" />
            </svg>
            <span>{t("overlay.find")}</span>
          </button>
          <button
            className={styles.recallBtn}
            onClick={() => useOverlayStore.getState().setEnabled(false)}
            title={t("overlay.recall")}
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 9v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h4" />
              <path d="M9 7L2 14" />
              <path d="M2 10v4h4" />
            </svg>
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.toggle}>
        <button className={styles.toggle_btn} onClick={() => setExpanded((v) => !v)}>
          <span className={styles.toggle_label}>{t("mascot.panel_title")}</span>
          <span className={styles.toggle_icon}>{expanded ? "▲" : "▼"}</span>
        </button>
        <button
          className={styles.popout_btn}
          onClick={(e) => {
            e.stopPropagation();
            useOverlayStore.getState().setEnabled(true);
          }}
          title={t("overlay.popout")}
        >
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M10 2h4v4" />
            <path d="M14 2L8 8" />
            <path d="M12 9v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h4" />
          </svg>
        </button>
      </div>
      {expanded && (
        <>
          {quipText && (
            <div className={styles.quipBubble} key={`${mood}-${quipIndex}-${clickReaction}`}>
              <div className={quipClasses}>{quipText}</div>
            </div>
          )}
          <RobotFrame onClick={handleMascotClick}>
            <div className={mascotClasses}>
              <svg viewBox="0 -14 200 114" className={styles.svg}>
                <defs>
                  <linearGradient id="grad-special" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor="rgba(255,255,255,0.3)" />
                    <stop offset="100%" stopColor="rgba(0,0,0,0.2)" />
                  </linearGradient>
                </defs>
                <rect x="0" y="-14" width="200" height="128" fill="var(--mascot-bg)" className={styles.bg} />
                <g style={{ transform: `translateY(${bodyTranslateY}px)` }} className={styles.bodyGroup}>
                  {renderEye(EYE_LEFT_CX, false)}
                  {renderEye(EYE_RIGHT_CX, true)}
                  {renderMouth()}
                </g>
                {renderEmojis()}
              </svg>
            </div>
          </RobotFrame>
        </>
      )}
    </div>
  );
}
