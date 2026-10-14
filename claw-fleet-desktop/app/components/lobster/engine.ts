/**
 * Render loop for the Lobster Fleet page: a low-resolution canvas scaled up by
 * an integer factor so every art pixel lands on whole device pixels.
 *
 * The technique (logical canvas + integer device-pixel scale + adaptive frame
 * rate) follows rezored/cubicle-crew (MIT); the art itself is drawn here.
 */

/** Logical canvas size and the integer factor it is blown up by. */
export interface PixelScale {
  /** Device pixels per art pixel. */
  scale: number;
  /** Logical (art-pixel) width and height of the canvas. */
  w: number;
  h: number;
}

/** Art pixels the scene wants vertically; the scale is chosen so the canvas
 *  is at least this tall, which keeps a lobster readable on any window. */
const TARGET_LOGICAL_H = 220;
const MIN_SCALE = 2;
const MAX_SCALE = 8;

/** Pick the largest integer scale that still leaves ~TARGET_LOGICAL_H art
 *  pixels of height, then size the logical canvas to fill the device area. */
export function pickScale(devW: number, devH: number): PixelScale {
  const raw = Math.floor(devH / TARGET_LOGICAL_H);
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, raw));
  return {
    scale,
    w: Math.max(1, Math.floor(devW / scale)),
    h: Math.max(1, Math.floor(devH / scale)),
  };
}

/** What the engine drives each frame. `t` is milliseconds since start. */
export interface Renderer {
  draw(ctx: CanvasRenderingContext2D, t: number, w: number, h: number): void;
  /** True while something on screen moves fast enough to need the full rate
   *  (a lobster at work); otherwise the loop drops to the idle rate. */
  busy(): boolean;
}

const BUSY_FPS = 30;
const IDLE_FPS = 8;

export class LobsterEngine {
  private readonly canvas: HTMLCanvasElement;
  private readonly ctx: CanvasRenderingContext2D;
  private renderer: Renderer | null = null;
  private raf = 0;
  private lastDraw = -Infinity;
  private readonly t0 = performance.now();
  private dirty = true;
  px: PixelScale = { scale: MIN_SCALE, w: 1, h: 1 };
  /** CSS pixels per art pixel; DOM overlays multiply art coordinates by it. */
  cssPerPx = MIN_SCALE;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2d canvas context unavailable");
    this.ctx = ctx;
  }

  setRenderer(r: Renderer) {
    this.renderer = r;
    this.dirty = true;
  }

  /** Force a draw on the next tick even at the idle rate (new data arrived). */
  invalidate() {
    this.dirty = true;
  }

  resize(cssW: number, cssH: number, dpr: number) {
    const px = pickScale(Math.round(cssW * dpr), Math.round(cssH * dpr));
    this.px = px;
    this.cssPerPx = px.scale / dpr;
    this.canvas.width = px.w;
    this.canvas.height = px.h;
    this.canvas.style.width = `${px.w * this.cssPerPx}px`;
    this.canvas.style.height = `${px.h * this.cssPerPx}px`;
    this.ctx.imageSmoothingEnabled = false;
    this.dirty = true;
  }

  start() {
    if (this.raf) return;
    const tick = (now: number) => {
      this.raf = requestAnimationFrame(tick);
      // A hidden page (minimised window, other tab) keeps nothing on screen
      // to update; skip the draw entirely rather than burn the CPU.
      if (document.hidden || !this.renderer) return;
      const fps = this.renderer.busy() ? BUSY_FPS : IDLE_FPS;
      if (!this.dirty && now - this.lastDraw < 1000 / fps) return;
      this.lastDraw = now;
      this.dirty = false;
      this.renderer.draw(this.ctx, now - this.t0, this.px.w, this.px.h);
    };
    this.raf = requestAnimationFrame(tick);
  }

  stop() {
    cancelAnimationFrame(this.raf);
    this.raf = 0;
  }
}
