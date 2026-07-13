export const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
export const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
// smooth ease-in-out
export const ease = (t: number) => {
  const c = clamp01(t);
  return c * c * (3 - 2 * c);
};
// ease-out cubic — good for entrances
export const easeOut = (t: number) => 1 - Math.pow(1 - clamp01(t), 3);
