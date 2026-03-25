/**
 * OctopusMascot — 全身章鱼 mascot，Live2D 风格分层动画。
 *
 * 渲染层次（从后到前）:
 *   1. BackTentacles  — 背后4条触手 (upper-left, left, upper-right, right)
 *   2. Body           — 橙色身体圆球
 *   3. Belly          — 腹部浅色贴片
 *   4. Eyes           — 双眼（含眼皮、瞳孔、高光）
 *   5. Blush          — 腮红
 *   6. Mouth          — 嘴巴 + 舌头
 *   7. Hat            — 船长帽（帽顶 + 金星 + 帽檐）
 *   8. FrontTentacles — 前方4条触手 (lower-left, bottom-left, bottom-right, lower-right)
 *
 * 每个层均可通过独立 transform 驱动动画，实现类 Live2D 效果。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../../store";
import type { SessionInfo } from "../../types";
import type { MascotMood } from "../MascotEyes";
import styles from "./OctopusMascot.module.css";

// ── Math helpers ─────────────────────────────────────────────────────────────

type Vec2 = [number, number];

function v2sub(a: Vec2, b: Vec2): Vec2 { return [a[0] - b[0], a[1] - b[1]]; }
function v2add(a: Vec2, b: Vec2): Vec2 { return [a[0] + b[0], a[1] + b[1]]; }
function v2scale(v: Vec2, s: number): Vec2 { return [v[0] * s, v[1] * s]; }
function v2norm(v: Vec2): Vec2 {
  const l = Math.sqrt(v[0] * v[0] + v[1] * v[1]);
  return l > 1e-9 ? [v[0] / l, v[1] / l] : [0, 0];
}
function v2perp(v: Vec2): Vec2 { return [-v[1], v[0]]; }
function f1(n: number): string { return n.toFixed(1); }

/** Evaluate cubic bezier at t ∈ [0,1] */
function bezierAt(t: number, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2): Vec2 {
  const mt = 1 - t;
  return [
    mt * mt * mt * p0[0] + 3 * mt * mt * t * p1[0] + 3 * mt * t * t * p2[0] + t * t * t * p3[0],
    mt * mt * mt * p0[1] + 3 * mt * mt * t * p1[1] + 3 * mt * t * t * p2[1] + t * t * t * p3[1],
  ];
}

/**
 * Build a filled tapered tentacle path from a cubic bezier spine.
 * Outer and inner edges are computed by offsetting the spine perpendicular
 * to the local tangent, tapering from baseW at the start to tipW at the end.
 */
function taperTentaclePath(
  start: Vec2, cp1: Vec2, cp2: Vec2, end: Vec2,
  baseW: number, tipW: number,
  flip = false,
): string {
  const tanStart = v2norm(v2sub(cp1, start));
  const tanEnd = v2norm(v2sub(end, cp2));
  const perpS = flip ? v2scale(v2perp(tanStart), -1) : v2perp(tanStart);
  const perpE = flip ? v2scale(v2perp(tanEnd), -1) : v2perp(tanEnd);

  const hw0 = baseW / 2;
  const hw1 = (baseW * 0.65) / 2;
  const hw2 = (tipW * 1.3) / 2;
  const hw3 = tipW / 2;

  const os  = v2add(start, v2scale(perpS,  hw0));
  const oc1 = v2add(cp1,   v2scale(perpS,  hw1));
  const oc2 = v2add(cp2,   v2scale(perpE,  hw2));
  const oe  = v2add(end,   v2scale(perpE,  hw3));
  const is_ = v2add(start, v2scale(perpS, -hw0));
  const ic1 = v2add(cp1,   v2scale(perpS, -hw1));
  const ic2 = v2add(cp2,   v2scale(perpE, -hw2));
  const ie  = v2add(end,   v2scale(perpE, -hw3));

  return [
    `M ${f1(os[0])},${f1(os[1])}`,
    `C ${f1(oc1[0])},${f1(oc1[1])} ${f1(oc2[0])},${f1(oc2[1])} ${f1(oe[0])},${f1(oe[1])}`,
    `A ${f1(hw3)},${f1(hw3)} 0 0 1 ${f1(ie[0])},${f1(ie[1])}`,
    `C ${f1(ic2[0])},${f1(ic2[1])} ${f1(ic1[0])},${f1(ic1[1])} ${f1(is_[0])},${f1(is_[1])}`,
    `Z`,
  ].join(" ");
}

// ── Geometry constants ────────────────────────────────────────────────────────

/** SVG viewBox: body center (100,52) r=47, extended for tentacles */
const VIEWBOX = "-5 -20 210 240";
const BODY_CX = 100;
// Same dome path as MascotEyes — full circle via two semicircle arcs
const DOME_PATH = "M 53,52 A 47 47 0 1 1 147,52 A 47 47 0 0 1 53,52";
const EYE_LEFT_CX  = 81;
const EYE_RIGHT_CX = 119;
const EYE_CY = 50;

// ── Tentacle definitions ──────────────────────────────────────────────────────

interface TentacleDef {
  id: string;
  layer: "back" | "front";
  start: Vec2;
  cp1: Vec2;
  cp2: Vec2;
  end: Vec2;
  curlCx: number;
  curlCy: number;
  curlR: number;
  flip: boolean;      // Flip inner/outer edge for correct shape orientation
  waveDir: Vec2;      // Normalized direction for wave oscillation (perpendicular to spine)
  phaseOffset: number;
  suckerTs: number[]; // t ∈ [0,1] positions along bezier spine for sucker spots
}

const TENTACLES: TentacleDef[] = [
  // ── Back tentacles (rendered before body) ────────────────────────────────
  {
    id: "ul", layer: "back",
    start: [67, 19], cp1: [48, 3], cp2: [26, -10], end: [16, -8],
    curlCx: 13, curlCy: -1, curlR: 6,
    flip: false, waveDir: [0.5, -0.87], phaseOffset: 0,
    suckerTs: [0.28, 0.52, 0.74],
  },
  {
    id: "l", layer: "back",
    start: [53, 52], cp1: [33, 44], cp2: [10, 42], end: [-2, 50],
    curlCx: -3, curlCy: 57, curlR: 6,
    flip: false, waveDir: [0, 1], phaseOffset: Math.PI * 0.4,
    suckerTs: [0.28, 0.52, 0.74],
  },
  {
    id: "ur", layer: "back",
    start: [133, 19], cp1: [152, 3], cp2: [174, -10], end: [184, -8],
    curlCx: 187, curlCy: -1, curlR: 6,
    flip: true, waveDir: [-0.5, -0.87], phaseOffset: Math.PI,
    suckerTs: [0.28, 0.52, 0.74],
  },
  {
    id: "r", layer: "back",
    start: [147, 52], cp1: [167, 44], cp2: [190, 42], end: [202, 50],
    curlCx: 203, curlCy: 57, curlR: 6,
    flip: true, waveDir: [0, -1], phaseOffset: Math.PI * 0.4,
    suckerTs: [0.28, 0.52, 0.74],
  },
  // ── Front tentacles (rendered after body) ────────────────────────────────
  {
    id: "ll", layer: "front",
    start: [67, 85], cp1: [46, 108], cp2: [24, 132], end: [13, 148],
    curlCx: 9, curlCy: 155, curlR: 6,
    flip: false, waveDir: [-0.87, 0.5], phaseOffset: Math.PI * 0.2,
    suckerTs: [0.28, 0.52, 0.74],
  },
  {
    id: "bl", layer: "front",
    start: [92, 98], cp1: [82, 124], cp2: [70, 148], end: [65, 164],
    curlCx: 59, curlCy: 169, curlR: 6,
    flip: false, waveDir: [-1, 0], phaseOffset: Math.PI * 0.6,
    suckerTs: [0.28, 0.52, 0.74],
  },
  {
    id: "br", layer: "front",
    start: [108, 98], cp1: [118, 124], cp2: [130, 148], end: [135, 164],
    curlCx: 141, curlCy: 169, curlR: 6,
    flip: true, waveDir: [1, 0], phaseOffset: Math.PI * 0.6,
    suckerTs: [0.28, 0.52, 0.74],
  },
  {
    id: "lr", layer: "front",
    start: [133, 85], cp1: [154, 108], cp2: [176, 132], end: [187, 148],
    curlCx: 191, curlCy: 155, curlR: 6,
    flip: true, waveDir: [0.87, 0.5], phaseOffset: Math.PI * 0.2,
    suckerTs: [0.28, 0.52, 0.74],
  },
];

// ── Mood system ───────────────────────────────────────────────────────────────
// (mirrors MascotEyes mood derivation)

function promoteSessions(sessions: SessionInfo[]): SessionInfo[] {
  const activeSubagentParentIds = new Set(
    sessions
      .filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId &&
          ["thinking", "executing", "streaming", "processing", "waitingInput", "active"].includes(s.status),
      )
      .map((s) => s.parentSessionId!),
  );
  return sessions.map((s) =>
    !s.isSubagent &&
    ["idle", "active", "waitingInput", "processing"].includes(s.status) &&
    activeSubagentParentIds.has(s.id)
      ? { ...s, status: "delegating" as const }
      : s,
  );
}

function deriveMood(sessions: SessionInfo[]): MascotMood {
  if (sessions.length === 0) return "lonely";
  const promoted = promoteSessions(sessions);
  const busyStatuses = ["thinking", "executing", "streaming", "processing", "active", "delegating"];
  const busy    = promoted.filter((s) => busyStatuses.includes(s.status));
  const waiting = promoted.filter((s) => s.status === "waitingInput");
  const idle    = promoted.filter((s) => s.status === "idle");
  const totalSpeed = promoted.reduce((sum, s) => sum + s.tokenSpeed, 0);
  if (busy.length === 0 && waiting.length === 0) return idle.length > 3 ? "sleepy" : "bored";
  if (totalSpeed > 80 || busy.length >= 4) return "excited";
  if (busy.length >= 1) return "focused";
  if (waiting.length > 0 && busy.length === 0) return "anxious";
  return "satisfied";
}

// ── Wave animation parameters per mood ───────────────────────────────────────

const WAVE_AMPLITUDE: Record<MascotMood, number> = {
  excited:     9,
  focused:     3,
  anxious:     8,
  satisfied:   4,
  bored:       2,
  lonely:      3,
  sleepy:      1,
  attentive:   6,
  proud:       5,
  frustrated:  7,
  embarrassed: 4,
};

const WAVE_SPEED: Record<MascotMood, number> = {
  excited:     0.07,
  focused:     0.035,
  anxious:     0.09,
  satisfied:   0.045,
  bored:       0.022,
  lonely:      0.028,
  sleepy:      0.014,
  attentive:   0.06,
  proud:       0.05,
  frustrated:  0.08,
  embarrassed: 0.04,
};

// ── Body color per mood ───────────────────────────────────────────────────────

const BODY_COLORS: Record<MascotMood, { main: string; light: string; blush: string; spot: string }> = {
  excited:   { main: "#f27d2c", light: "#fde0b4", blush: "rgba(255,107,107,0.8)", spot: "#f9c07a" },
  focused:   { main: "#e8722a", light: "#f5d09a", blush: "rgba(255,107,107,0.7)", spot: "#f5c07a" },

  anxious:   { main: "#d87828", light: "#e5c08a", blush: "rgba(255,107,107,0.7)", spot: "#f0b870" },
  satisfied: { main: "#f27d2c", light: "#fde0b4", blush: "rgba(255,107,107,0.8)", spot: "#f9c07a" },
  bored:     { main: "#c07038", light: "#d0a878", blush: "rgba(255,107,107,0.5)", spot: "#d8a070" },
  lonely:    { main: "#c86830", light: "#d8a068", blush: "rgba(255,107,107,0.5)", spot: "#d89060" },
  sleepy:      { main: "#b06830", light: "#c09860", blush: "rgba(255,107,107,0.4)", spot: "#c88858" },
  attentive:   { main: "#e08030", light: "#f5d8a0", blush: "rgba(100,180,255,0.6)", spot: "#f0c080" },
  proud:       { main: "#f08028", light: "#ffe0b0", blush: "rgba(255,200,60,0.7)",  spot: "#ffd070" },
  frustrated:  { main: "#c85030", light: "#e0a080", blush: "rgba(255,80,80,0.8)",   spot: "#d87060" },
  embarrassed: { main: "#d87038", light: "#e8b890", blush: "rgba(255,130,130,0.8)", spot: "#e0a078" },
};

// ── Eye shape variants per mood ───────────────────────────────────────────────

interface EyeShape {
  rx: number; ry: number; pr: number;
  lidTop: number; lidBot: number;
  eyeColor: string; pupilColor: string;
  glowColor?: string;
  rightLidTop?: number; rightRy?: number;
  pupilDirX?: number; pupilDirY?: number;
}

const EYE_SHAPE_VARIANTS: Record<MascotMood, EyeShape[]> = {
  excited:   [
    { rx: 8.0, ry: 7.2, pr: 7.2, lidTop: 0.15, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.2,  lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 8.0, pr: 7.2, lidTop: 0.15, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.05 },
    { rx: 8.0, ry: 7.2, pr: 7.2, lidTop: 0.12, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
  ],
  focused:   [
    { rx: 8.0, ry: 8.0, pr: 7.2, lidTop: 0.08, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 7.2, pr: 7.2, lidTop: 0.15, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 8.0, pr: 7.2, lidTop: 0.08, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.25 },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.2,  lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
  ],
  anxious:   [
    { rx: 9.6, ry: 9.6,  pr: 8.0, lidTop: 0.05, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 9.6, ry: 11.2, pr: 8.0, lidTop: 0.05, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 9.6, ry: 9.6,  pr: 8.0, lidTop: 0.05, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.18 },
    { rx: 9.6, ry: 9.6,  pr: 6.4, lidTop: 0.05, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: -0.5 },
  ],
  satisfied: [
    { rx: 8.0, ry: 7.2, pr: 7.2, lidTop: 0.05, lidBot: 0.3,  eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.12, lidBot: 0.4,  eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 8.0, pr: 7.2, lidTop: 0.05, lidBot: 0.25, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.85 },
    { rx: 8.0, ry: 5.6, pr: 5.6, lidTop: 0.15, lidBot: 0.45, eyeColor: "#1a1a2e", pupilColor: "transparent" },
  ],
  bored:     [
    { rx: 8.0, ry: 8.0, pr: 7.2, lidTop: 0.35, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.42, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 8.0, pr: 6.4, lidTop: 0.3,  lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: -0.8 },
    { rx: 8.0, ry: 7.2, pr: 5.6, lidTop: 0.38, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
  ],
  lonely:    [
    { rx: 9.6,  ry: 10.4, pr: 8.8, lidTop: 0,    lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 10.4, ry: 11.2, pr: 9.6, lidTop: 0,    lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 9.6,  ry: 10.4, pr: 8.8, lidTop: 0.05, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: 0.5 },
    { rx: 9.6,  ry: 10.4, pr: 8.8, lidTop: 0,    lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirX: 0.8 },
  ],
  sleepy:    [
    { rx: 8.0, ry: 5.6, pr: 5.6, lidTop: 0.5,  lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 4.8, pr: 4.8, lidTop: 0.6,  lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 5.6, pr: 5.6, lidTop: 0.45, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.7 },
    { rx: 8.0, ry: 4.0, pr: 4.0, lidTop: 0.65, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
  ],
  attentive: [
    { rx: 10.0, ry: 10.0, pr: 8.8, lidTop: 0.02, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 10.0, ry: 10.4, pr: 9.2, lidTop: 0,    lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 10.0, ry: 10.0, pr: 8.8, lidTop: 0.02, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: -0.3 },
    { rx: 10.0, ry: 10.0, pr: 8.8, lidTop: 0,    lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.1 },
  ],
  proud: [
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.3,  lidBot: 0.28, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 5.6, pr: 5.6, lidTop: 0.38, lidBot: 0.35, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.28, lidBot: 0.25, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.85 },
    { rx: 8.0, ry: 5.6, pr: 5.6, lidTop: 0.42, lidBot: 0.38, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: 0.3 },
  ],
  frustrated: [
    { rx: 7.2, ry: 6.4, pr: 6.4, lidTop: 0.28, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 7.2, ry: 5.6, pr: 5.6, lidTop: 0.35, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 7.2, ry: 6.4, pr: 6.4, lidTop: 0.25, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: -0.4 },
    { rx: 7.2, ry: 5.6, pr: 5.6, lidTop: 0.32, lidBot: 0, eyeColor: "#1a1a2e", pupilColor: "transparent", rightLidTop: 0.45 },
  ],
  embarrassed: [
    { rx: 8.0, ry: 7.2, pr: 7.2, lidTop: 0.22, lidBot: 0.12, eyeColor: "#1a1a2e", pupilColor: "transparent" },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.28, lidBot: 0.18, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirY: 0.5 },
    { rx: 8.0, ry: 7.2, pr: 7.2, lidTop: 0.25, lidBot: 0.15, eyeColor: "#1a1a2e", pupilColor: "transparent", pupilDirX: 0.6 },
    { rx: 8.0, ry: 6.4, pr: 6.4, lidTop: 0.3,  lidBot: 0.2,  eyeColor: "#1a1a2e", pupilColor: "transparent" },
  ],
};

// ── Mouth variants per mood ───────────────────────────────────────────────────

const MOUTH_VARIANTS: Record<MascotMood, { d: string; fill?: string }[]> = {
  excited:   [
    { d: "M 94,56 Q 100,54 106,52" },
    { d: "M 93,55 Q 100,59 107,53" },
    { d: "M 93,56 Q 97,54 100,55 Q 103,54 107,52" },
    { d: "M 92,55 Q 100,60 108,53" },
  ],
  focused:   [
    { d: "M 96,56 L 104,56" },
    { d: "M 95,56 Q 100,55 105,56" },
    { d: "M 96,57 Q 100,55 104,57" },
    { d: "M 95,55 Q 100,56 105,55 L 107,55" },
  ],
  anxious:   [
    { d: "M 94,56 Q 97,54 100,56 Q 103,54 106,56" },
    { d: "M 95,56 Q 100,53 105,56" },
    { d: "M 94,55 Q 97,57 100,55 Q 103,57 106,55" },
    { d: "M 95,55 Q 100,58 105,55" },
  ],
  satisfied: [
    { d: "M 95.5,54 Q 100,60.7 104.5,54" },
    { d: "M 94,54 Q 100,62 106,54", fill: "#5a2a10" },
    { d: "M 95,55 Q 100,58 105,55" },
    { d: "M 95,55 Q 97,58 100,56 Q 103,58 105,55" },
  ],
  bored:     [
    { d: "M 96,56 L 104,56" },
    { d: "M 95,56 Q 100,58 105,56" },
    { d: "M 96,56 Q 100,57 104,56 L 105,58" },
    { d: "M 96,55 Q 100,60 104,55", fill: "#5a2a10" },
  ],
  lonely:    [
    { d: "M 94,58 Q 100,54 106,58" },
    { d: "M 95,57 Q 100,54 105,57" },
    { d: "M 94,57 Q 97,55 100,57 Q 103,55 106,57" },
    { d: "M 98,56 Q 100,59 102,56" },
  ],
  sleepy:      [
    { d: "M 96,56 Q 100,58 104,56" },
    { d: "M 96,56 L 104,56" },
    { d: "M 95,55 Q 100,57 105,55" },
    { d: "M 96,56 Q 100,58 104,56 L 105,60" },
  ],
  attentive:   [
    { d: "M 97,56 Q 100,59 103,56" },   // small "o"
    { d: "M 96,56 Q 100,60 104,56" },
    { d: "M 97,55 Q 100,58 103,55" },
    { d: "M 96,55 Q 100,59 104,55" },
  ],
  proud:       [
    { d: "M 93,55 Q 100,62 107,55" },           // big grin
    { d: "M 94,54 Q 100,61 106,54", fill: "#5a2a10" },
    { d: "M 93,56 Q 100,60 107,56" },
    { d: "M 94,55 Q 100,62 106,55", fill: "#5a2a10" },
  ],
  frustrated:  [
    { d: "M 94,58 Q 100,54 106,58" },   // frown
    { d: "M 95,57 Q 100,54 105,57" },
    { d: "M 94,57 Q 97,55 100,57 Q 103,55 106,57" },
    { d: "M 95,58 Q 100,54 105,58" },
  ],
  embarrassed: [
    { d: "M 94,56 Q 97,54 100,56 Q 103,54 106,56" },  // wobbly
    { d: "M 96,56 Q 100,58 104,56" },
    { d: "M 95,55 Q 98,57 100,55 Q 102,57 105,55" },
    { d: "M 95,56 Q 100,58 105,56" },
  ],
};

// ── Pupil wander config ───────────────────────────────────────────────────────

interface PupilWanderConfig { range: number; interval: [number, number]; dirBlend: number; }

const PUPIL_WANDER: Record<MascotMood, PupilWanderConfig> = {
  excited:     { range: 2.5, interval: [1000, 2000], dirBlend: 0.2 },
  focused:     { range: 1.5, interval: [2000, 4000], dirBlend: 0.15 },
  anxious:     { range: 7,   interval: [400,  1200], dirBlend: 0.4 },
  satisfied:   { range: 2,   interval: [2500, 5000], dirBlend: 0.2 },
  bored:       { range: 5,   interval: [1500, 3500], dirBlend: 0.3 },
  lonely:      { range: 2,   interval: [2000, 4000], dirBlend: 0.2 },
  sleepy:      { range: 1,   interval: [3000, 6000], dirBlend: 0.1 },
  attentive:   { range: 3,   interval: [800,  1500], dirBlend: 0.35 },
  proud:       { range: 1.5, interval: [2500, 5000], dirBlend: 0.15 },
  frustrated:  { range: 2,   interval: [600,  1200], dirBlend: 0.3 },
  embarrassed: { range: 3,   interval: [1500, 3000], dirBlend: 0.25 },
};

// ── Component ─────────────────────────────────────────────────────────────────

export interface OctopusMascotProps {
  /** Override CSS class for the outer container */
  className?: string;
  /** Show / hide the quip text below the mascot */
  showQuip?: boolean;
  /** Force a specific mood (overrides session-derived mood) */
  forceMood?: MascotMood;
}

export function OctopusMascot({ className, showQuip = true, forceMood }: OctopusMascotProps) {
  const { t } = useTranslation();
  const { sessions } = useSessionsStore();
  const derivedMood = useMemo(() => deriveMood(sessions), [sessions]);
  const mood = forceMood ?? derivedMood;

  // ── Per-frame wave phase (drives tentacle animation) ──────────────────────
  const [wavePhase, setWavePhase] = useState(0);
  const rafRef = useRef<number>(0);
  const phaseRef = useRef(0);

  useEffect(() => {
    const speed = WAVE_SPEED[mood];
    const tick = () => {
      phaseRef.current += speed;
      setWavePhase(phaseRef.current);
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafRef.current);
  }, [mood]);

  // ── Body bob (gentle float) ────────────────────────────────────────────────
  const [bodyBobPhase, setBodyBobPhase] = useState(0);
  const bobTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  const bobFrameRef = useRef(0);

  useEffect(() => {
    function tick() {
      bobFrameRef.current++;
      setBodyBobPhase(Math.sin(bobFrameRef.current * 0.04) * 1.5);
      bobTimerRef.current = setTimeout(tick, 50);
    }
    tick();
    return () => clearTimeout(bobTimerRef.current);
  }, []);

  // ── Blink ──────────────────────────────────────────────────────────────────
  const [isBlinking, setIsBlinking] = useState(false);
  const blinkTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    function scheduleBlink() {
      const delay = 2000 + Math.random() * 4000;
      blinkTimerRef.current = setTimeout(() => {
        setIsBlinking(true);
        const doDouble = Math.random() < 0.22;
        if (doDouble) {
          setTimeout(() => setIsBlinking(false), 110);
          setTimeout(() => setIsBlinking(true),  240);
          setTimeout(() => setIsBlinking(false), 360);
        } else {
          setTimeout(() => setIsBlinking(false), 140);
        }
        scheduleBlink();
      }, delay);
    }
    scheduleBlink();
    return () => clearTimeout(blinkTimerRef.current);
  }, []);

  // ── Pupil wander ───────────────────────────────────────────────────────────
  const [pupilOffset, setPupilOffset] = useState({ x: 0, y: 0 });
  const pupilTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    const cfg = PUPIL_WANDER[mood];
    function scheduleMove() {
      const delay = cfg.interval[0] + Math.random() * (cfg.interval[1] - cfg.interval[0]);
      pupilTimerRef.current = setTimeout(() => {
        setPupilOffset({ x: (Math.random() - 0.5) * cfg.range * 2, y: (Math.random() - 0.5) * cfg.range });
        scheduleMove();
      }, delay);
    }
    scheduleMove();
    return () => clearTimeout(pupilTimerRef.current);
  }, [mood]);

  // ── Eye variant cycling ────────────────────────────────────────────────────
  const [eyeVariant, setEyeVariant] = useState(0);
  const eyeVarTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    setEyeVariant(0);
    function schedule() {
      eyeVarTimerRef.current = setTimeout(() => {
        setEyeVariant((v) => (v + 1) % EYE_SHAPE_VARIANTS[mood].length);
        schedule();
      }, 4000 + Math.random() * 6000);
    }
    schedule();
    return () => clearTimeout(eyeVarTimerRef.current);
  }, [mood]);

  // ── Mouth variant cycling ──────────────────────────────────────────────────
  const [mouthVariant, setMouthVariant] = useState(0);
  const mouthTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    setMouthVariant(0);
    function schedule() {
      mouthTimerRef.current = setTimeout(() => {
        setMouthVariant((v) => (v + 1) % MOUTH_VARIANTS[mood].length);
        schedule();
      }, 3000 + Math.random() * 5000);
    }
    schedule();
    return () => clearTimeout(mouthTimerRef.current);
  }, [mood]);

  // ── Quip index ─────────────────────────────────────────────────────────────
  const [quipIndex, setQuipIndex] = useState(0);
  const quipTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    setQuipIndex(Math.floor(Math.random() * 3));
    function schedule() {
      quipTimerRef.current = setTimeout(() => {
        setQuipIndex((i) => (i + 1) % 3);
        schedule();
      }, 8000 + Math.random() * 4000);
    }
    schedule();
    return () => clearTimeout(quipTimerRef.current);
  }, [mood]);

  // ── Mood change jiggle ─────────────────────────────────────────────────────
  const [moodJiggle, setMoodJiggle] = useState(false);

  useEffect(() => {
    setMoodJiggle(true);
    const t = setTimeout(() => setMoodJiggle(false), 400);
    return () => clearTimeout(t);
  }, [mood]);

  // ── Derived render values ──────────────────────────────────────────────────
  const body    = BODY_COLORS[mood];
  const shape   = EYE_SHAPE_VARIANTS[mood][eyeVariant % EYE_SHAPE_VARIANTS[mood].length];
  const mouth   = MOUTH_VARIANTS[mood][mouthVariant % MOUTH_VARIANTS[mood].length];
  const quipKey = `mascot.${["excited","focused","anxious","satisfied","bored","lonely","sleepy"].includes(mood) ? mood : "bored"}_${quipIndex + 1}`;
  const amplitude = WAVE_AMPLITUDE[mood];

  const wanderCfg = PUPIL_WANDER[mood];
  const effectivePupilOffset =
    shape.pupilDirX !== undefined || shape.pupilDirY !== undefined
      ? {
          x: (shape.pupilDirX ?? 0) * 4 + pupilOffset.x * wanderCfg.dirBlend,
          y: (shape.pupilDirY ?? 0) * 4 + pupilOffset.y * wanderCfg.dirBlend,
        }
      : pupilOffset;

  // ── Tentacle rendering ─────────────────────────────────────────────────────
  const renderTentacle = useCallback(
    (def: TentacleDef) => {
      const wave = Math.sin(wavePhase + def.phaseOffset);
      const wx   = def.waveDir[0] * amplitude * wave;
      const wy   = def.waveDir[1] * amplitude * wave;

      const wCp1: Vec2 = [def.cp1[0] + wx,       def.cp1[1] + wy];
      const wCp2: Vec2 = [def.cp2[0] + wx * 0.55, def.cp2[1] + wy * 0.55];

      const bodyPath = taperTentaclePath(def.start, wCp1, wCp2, def.end, 14, 5, def.flip);
      const curlCx   = def.curlCx + wx * 0.3;
      const curlCy   = def.curlCy + wy * 0.3;
      const suckers  = def.suckerTs.map((tv) => bezierAt(tv, def.start, wCp1, wCp2, def.end));

      const mainColor = body.main;
      const hiColor   = body.spot;
      const spotColor = body.light;

      return (
        <g key={def.id}>
          {/* Shadow strip along inner edge */}
          <path d={bodyPath} fill="rgba(0,0,0,0.12)" transform="translate(1.5,1.5)" />
          {/* Main body fill */}
          <path d={bodyPath} fill={mainColor} stroke="#1a2035" strokeWidth={0.8} />
          {/* Highlight strip */}
          <path
            d={`M ${f1(def.start[0])},${f1(def.start[1])} C ${f1(wCp1[0])},${f1(wCp1[1])} ${f1(wCp2[0])},${f1(wCp2[1])} ${f1(def.end[0])},${f1(def.end[1])}`}
            fill="none" stroke={hiColor} strokeWidth={3.5} strokeLinecap="round" opacity={0.3}
          />
          {/* Tip curl */}
          <circle cx={f1(curlCx)} cy={f1(curlCy)} r={def.curlR}
            fill="none" stroke={mainColor} strokeWidth={4.5} opacity={0.9} />
          <circle cx={f1(curlCx)} cy={f1(curlCy)} r={def.curlR}
            fill="none" stroke="#1a2035" strokeWidth={0.7} opacity={0.5} />
          {/* Sucker spots */}
          {suckers.map((pos, i) => (
            <circle key={i}
              cx={f1(pos[0])} cy={f1(pos[1])}
              r={3.2 - i * 0.35}
              fill={spotColor} stroke="rgba(0,0,0,0.15)" strokeWidth={0.4}
            />
          ))}
        </g>
      );
    },
    [wavePhase, amplitude, body],
  );

  // ── Eye rendering ──────────────────────────────────────────────────────────
  const renderEye = useCallback(
    (cx: number, isRight: boolean) => {
      const { rx, ry, pr, lidTop, lidBot, eyeColor, pupilColor, glowColor, rightLidTop, rightRy } = shape;
      const effectivePr = Math.max(3, pr);
      const effectiveRy = isRight && rightRy ? rightRy : ry;
      const baseLidTop  = isRight && rightLidTop !== undefined ? rightLidTop : lidTop;
      const blinkLid    = isBlinking ? 0.95 : Math.max(0, Math.min(baseLidTop, 0.95));

      const eyeTop  = EYE_CY - effectiveRy;
      const eyeBot  = EYE_CY + effectiveRy;
      const lidTopY = eyeTop + effectiveRy * 2 * blinkLid;
      const lidBotY = eyeBot - effectiveRy * 2 * lidBot;

      const px = cx + effectivePupilOffset.x;
      const py = EYE_CY + effectivePupilOffset.y;
      const jiggleClass = moodJiggle ? styles.eyeJiggle : "";

      return (
        <g className={jiggleClass}>
          {glowColor && (
            <ellipse cx={cx} cy={EYE_CY} rx={rx + 4} ry={effectiveRy + 4} fill={glowColor}
              className={styles.softGlow} />
          )}
          {mood === "lonely" && (
            <ellipse cx={cx} cy={EYE_CY} rx={rx + 2} ry={effectiveRy + 2}
              fill="rgba(100,180,255,0.08)" className={styles.waterShimmer} />
          )}
          <ellipse cx={cx} cy={EYE_CY} rx={rx} ry={effectiveRy}
            fill={eyeColor} stroke="#2a1a0a" strokeWidth={1} className={styles.eyeWhite} />
          <circle cx={px} cy={py} r={effectivePr} fill={pupilColor} className={styles.pupil} />
          {/* Primary highlight */}
          <circle cx={px - rx * 0.325} cy={py - effectiveRy * 0.475}
            r={rx * 0.3375} fill="#ffffff" className={styles.pupil} />
          {/* Secondary highlight */}
          <circle cx={px + rx * 0.475} cy={py + effectiveRy * 0.475}
            r={rx * 0.16875} fill="#ffffff" className={styles.pupil} />
          {/* Tear highlight (lonely) */}
          {mood === "lonely" && !isBlinking && (
            <ellipse cx={cx + rx * 0.3} cy={EYE_CY - effectiveRy * 0.4}
              rx={2.5} ry={1.5} fill="rgba(255,255,255,0.6)" className={styles.tearHighlight} />
          )}
          {/* Top eyelid */}
          <rect x={cx - rx - 1} y={eyeTop - 2} width={rx * 2 + 2}
            height={Math.max(0, lidTopY - eyeTop + 2)} fill={body.main} className={styles.eyelid} />
          {/* Bottom eyelid */}
          {lidBot > 0 && (
            <rect x={cx - rx - 1} y={lidBotY} width={rx * 2 + 2}
              height={Math.max(0, eyeBot - lidBotY + 2)} fill={body.main} className={styles.eyelid} />
          )}
        </g>
      );
    },
    [shape, body, effectivePupilOffset, isBlinking, mood, moodJiggle],
  );

  // ── Mood-specific decoration rendering ────────────────────────────────────
  const renderDecorations = useCallback(() => {
    const els: React.ReactNode[] = [];
    switch (mood) {
      case "sleepy":
        els.push(
          <text key="zzz" x={158} y={18} className={styles.zzz} fill="rgba(200,200,220,0.8)" fontSize="13" fontWeight="bold">
            z<tspan dx="3" dy="-5" fontSize="10">z</tspan><tspan dx="2" dy="-4" fontSize="8">z</tspan>
          </text>,
        );
        break;
      case "anxious":
        els.push(
          <ellipse key="sw1" cx={46}  cy={18} rx={2.5} ry={4.5} fill="#7dd3fc" className={styles.sweatDrop} />,
          <ellipse key="sw2" cx={158} cy={22} rx={2}   ry={3.5} fill="#7dd3fc" className={styles.sweatDrop2} />,
        );
        break;
      case "lonely":
        els.push(
          <ellipse key="t1" cx={EYE_LEFT_CX}  cy={46} rx={1.2} ry={3} fill="rgba(100,180,255,0.5)" className={styles.tear} />,
          <ellipse key="t2" cx={EYE_RIGHT_CX} cy={46} rx={1.2} ry={3} fill="rgba(100,180,255,0.5)" className={styles.tear2} />,
        );
        break;
      case "excited":
        els.push(
          <line key="aura1" x1={45}  y1={20} x2={40}  y2={12} stroke="rgba(255,180,80,0.4)" strokeWidth={1} className={styles.sparkle1} />,
          <line key="aura2" x1={155} y1={20} x2={160} y2={12} stroke="rgba(255,180,80,0.4)" strokeWidth={1} className={styles.sparkle2} />,
          <line key="aura3" x1={100} y1={10} x2={100} y2={2}  stroke="rgba(255,180,80,0.3)" strokeWidth={1} className={styles.sparkle3} />,
        );
        break;
      case "satisfied":
        els.push(
          <text key="note1" x={158} y={16} fontSize="18" className={styles.sparkle1}>♪</text>,
          <text key="note2" x={36}  y={20} fontSize="13" className={styles.sparkle3}>♪</text>,
          <text key="heart1" x={62}  y={42} fontSize="10" className={styles.floatHeart1}>♥</text>,
          <text key="heart2" x={134} y={40} fontSize="8"  className={styles.floatHeart2}>♥</text>,
        );
        break;
      default:
        break;
    }
    return els.length > 0 ? <>{els}</> : null;
  }, [mood]);

  // ── Full SVG assembly ──────────────────────────────────────────────────────
  const backTentacles  = useMemo(() => TENTACLES.filter((d) => d.layer === "back"),  []);
  const frontTentacles = useMemo(() => TENTACLES.filter((d) => d.layer === "front"), []);

  const bodyTranslateY = bodyBobPhase;

  return (
    <div className={[styles.container, className ?? ""].join(" ").trim()}>
      {showQuip && t(quipKey, { defaultValue: "" }) && (
        <div className={styles.quipBubble} key={quipKey}>
          <div className={[styles.quip, styles[mood]].join(" ")}>
            {t(quipKey, { defaultValue: "" })}
          </div>
        </div>
      )}
      <svg
        viewBox={VIEWBOX}
        className={styles.svg}
        xmlns="http://www.w3.org/2000/svg"
      >
        <defs>
          <clipPath id="octo-full-dome-clip">
            <path d={DOME_PATH} />
          </clipPath>
        </defs>

        {/* ── Layer 1: Back tentacles ─────────────────────────────────────── */}
        <g className={styles.backTentacles}>
          {backTentacles.map(renderTentacle)}
        </g>

        {/* ── Everything that bobs ────────────────────────────────────────── */}
        <g style={{ transform: `translateY(${bodyTranslateY}px)` }} className={[styles.bodyGroup, styles[mood]].join(" ")}>

          {/* ── Layer 2: Body dome ──────────────────────────────────────────── */}
          <path d={DOME_PATH} fill={body.main} stroke="#1a2035" strokeWidth={2.5}
            strokeLinejoin="round" className={styles.head} />

          {/* ── Layer 3: Belly patch ────────────────────────────────────────── */}
          <path d="M 66,83.5 A 36 29 0 0 0 134,83.5 A 47 47 0 0 1 66,83.5 Z"
            fill={body.light} stroke="#1a2035" strokeWidth={2} />

          {/* Bottom shadow inside dome */}
          <g clipPath="url(#octo-full-dome-clip)">
            <ellipse cx={BODY_CX} cy={76} rx={44} ry={12} fill="rgba(0,0,0,0.1)" />
          </g>

          {/* Body highlight blobs */}
          <ellipse cx={76} cy={12} rx={9} ry={5} fill="rgba(255,255,255,0.25)" transform="rotate(-20, 76, 12)" />
          <ellipse cx={87} cy={20} rx={3.5} ry={2.5} fill="rgba(255,255,255,0.2)" transform="rotate(-10, 87, 20)" />

          {/* ── Layer 4: Eyes ────────────────────────────────────────────────── */}
          <g clipPath="url(#octo-full-dome-clip)">
            {renderEye(EYE_LEFT_CX, false)}
            {renderEye(EYE_RIGHT_CX, true)}
          </g>

          {/* ── Layer 5: Blush ───────────────────────────────────────────────── */}
          <ellipse cx={68}  cy={61.5} rx={6.5} ry={3.5} fill={body.blush}
            className={[styles.blush, mood === "excited" ? styles.blushPulse : ""].join(" ")} />
          <ellipse cx={132} cy={61.5} rx={6.5} ry={3.5} fill={body.blush}
            className={[styles.blush, mood === "excited" ? styles.blushPulse : ""].join(" ")} />

          {/* ── Layer 6: Mouth ───────────────────────────────────────────────── */}
          <g transform="translate(0, 3)">
            <path d={mouth.d} stroke="#1a2035" strokeWidth={2.5}
              fill={mouth.fill ?? "none"} strokeLinecap="round" strokeLinejoin="round"
              className={styles.mouth} />
          </g>

          {/* ── Layer 7: Hat ─────────────────────────────────────────────────── */}
          <g className={styles.hat}>
            {/* Crown */}
            <path d="M 71,23 C 60,1 140,1 129,23 Z" fill="#1a233a" stroke="#1a2035" strokeWidth={2} />
            {/* Brim highlight stripe */}
            <path d="M 68,21 Q 100,33 132,21 L 133,24 Q 100,37 67,24 Z" fill="#fbbf24" stroke="#1a2035" strokeWidth={1.5} />
            {/* Brim base */}
            <path d="M 66,23 Q 100,37 134,23 L 136,26 Q 100,41 64,26 Z" fill="#111524" stroke="#1a2035" strokeWidth={2} strokeLinejoin="round" />
            {/* Crown highlight */}
            <path d="M 73,24 Q 91,31 100,32 L 95,28 Q 82,28 75,23 Z" fill="#ffffff" opacity={0.25} />
            {/* Star */}
            <polygon points="100,10 101.5,13.5 105,13.5 102,15.5 103,19 100,17 97,19 98,15.5 95,13.5 98.5,13.5"
              fill="#fbbf24" className={styles.hatStar} />
          </g>

        </g>{/* end bodyGroup */}

        {/* ── Layer 8: Front tentacles ────────────────────────────────────── */}
        <g className={styles.frontTentacles}>
          {frontTentacles.map(renderTentacle)}
        </g>

        {/* ── Decorations (outside body bob group) ────────────────────────── */}
        {renderDecorations()}

      </svg>
    </div>
  );
}
