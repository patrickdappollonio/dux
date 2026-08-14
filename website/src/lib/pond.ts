// The maths behind the homepage hero's pond ripples: the cursor is a duck
// gliding on a pond, and every ring it leaves lights the grid intersections it
// crosses. The drawing lives in `src/components/PondRipples.astro`; everything
// here is pure so the tuning can be tested without a canvas.
//
// Every constant below was tuned by feel across three iterations. They are
// named and commented one per line so a future taste tweak is a one-line edit
// rather than an archaeology expedition.

/** How fast a ring's radius grows, in CSS pixels per second. */
export const SPEED = 120;

/** Half-width of the lit band around a ring's radius, in pixels. */
export const BAND = 32;

/** How long a ring lives before it has faded to nothing, in seconds. */
export const LIFE = 2.4;

/** Minimum gap between two cursor-glide drips, in milliseconds. */
export const DRIP_MS = 90;

/** Amplitude of a ring dripped by the gliding cursor. */
export const DRIP_AMP = 0.5;

/** Amplitude of the bigger splash a click or tap drops. */
export const SPLASH_AMP = 1.2;

/** Grid pitch, matching `.grid-bg`'s 44px background-size. */
export const CELL = 44;

/** Grid phase, matching `.grid-bg`'s -1px background-position. */
export const GRID_OFFSET = -1;

/** Below this, a point is too dim to be worth a paint call. */
export const MIN_GLOW = 0.02;

/** A single expanding ring: origin, birth time in seconds, and strength. */
export interface Ring {
  x: number;
  y: number;
  t: number;
  amp: number;
}

/** Age of a ring in seconds at wall-clock time `now` (also in seconds). */
export function ringAge(ring: Ring, now: number): number {
  return now - ring.t;
}

/** A ring is done once it has outlived `LIFE`; the caller then drops it. */
export function isAlive(ring: Ring, now: number): boolean {
  return ringAge(ring, now) < LIFE;
}

/**
 * How much one ring lights a point `dist` pixels from its origin. The band is a
 * linear tent centred on the ring's current radius, squared so the edges fall
 * off softly, then scaled by the ring's linear fade and its amplitude.
 */
export function ringGlow(ring: Ring, dist: number, now: number): number {
  const age = ringAge(ring, now);
  if (age < 0 || age >= LIFE) return 0;
  const radius = age * SPEED;
  const band = Math.max(0, 1 - Math.abs(dist - radius) / BAND);
  if (band <= 0) return 0;
  const fade = 1 - age / LIFE;
  return band * band * fade * ring.amp;
}

/**
 * Total glow at a grid intersection: every live ring adds its contribution, and
 * the sum is clamped so overlapping splashes brighten without blowing out.
 */
export function pointGlow(rings: readonly Ring[], x: number, y: number, now: number): number {
  let glow = 0;
  for (const ring of rings) {
    const dx = x - ring.x;
    const dy = y - ring.y;
    glow += ringGlow(ring, Math.hypot(dx, dy), now);
  }
  return Math.min(1, glow);
}

/** Halo alpha and radius for a lit point, painted under the core. */
export function haloPaint(glow: number): { alpha: number; radius: number } {
  return { alpha: glow * 0.2, radius: 4 + glow * 3.5 };
}

/** Core alpha and radius for a lit point, painted over the halo. */
export function corePaint(glow: number): { alpha: number; radius: number } {
  return { alpha: glow * 0.85, radius: 1.3 + glow * 1.9 };
}
