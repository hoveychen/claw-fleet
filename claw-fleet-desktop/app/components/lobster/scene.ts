/**
 * The sea-floor backdrop: water gradient, light shafts, sand. Fixed art
 * colours on purpose — this is a painted scene, not themed chrome.
 */

export const WATER_TOP = "#1d5f86";
export const WATER_BOTTOM = "#0b2240";
export const SAND = "#c9a46a";
export const SAND_DARK = "#a9844f";

/** Height in art pixels of the sand strip along the bottom. */
export const SAND_H = 14;

export function drawOcean(ctx: CanvasRenderingContext2D, t: number, w: number, h: number) {
  const g = ctx.createLinearGradient(0, 0, 0, h);
  g.addColorStop(0, WATER_TOP);
  g.addColorStop(1, WATER_BOTTOM);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);

  // Slow light shafts from the surface, swaying a few pixels.
  ctx.save();
  ctx.globalAlpha = 0.07;
  ctx.fillStyle = "#ffffff";
  for (let i = 0; i < 5; i++) {
    const x0 = ((i + 0.5) * w) / 5 + Math.sin(t / 3000 + i) * 6;
    ctx.beginPath();
    ctx.moveTo(x0 - 6, 0);
    ctx.lineTo(x0 + 6, 0);
    ctx.lineTo(x0 + 26, h - SAND_H);
    ctx.lineTo(x0 + 2, h - SAND_H);
    ctx.closePath();
    ctx.fill();
  }
  ctx.restore();

  // Sand with a dithered top edge.
  const sy = h - SAND_H;
  ctx.fillStyle = SAND;
  ctx.fillRect(0, sy, w, SAND_H);
  ctx.fillStyle = SAND_DARK;
  for (let x = 0; x < w; x += 2) ctx.fillRect(x, sy + ((x / 2) % 2), 1, 1);
  for (let x = 3; x < w; x += 7) ctx.fillRect(x, sy + 5 + (x % 5), 1, 1);
}
