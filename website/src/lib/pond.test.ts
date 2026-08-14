import { describe, it, expect } from "vitest";
import {
  BAND,
  CELL,
  LIFE,
  SPEED,
  corePaint,
  haloPaint,
  isAlive,
  pointGlow,
  ringGlow,
  type Ring,
} from "./pond";

const ring = (over: Partial<Ring> = {}): Ring => ({ x: 0, y: 0, t: 0, amp: 1, ...over });

describe("isAlive", () => {
  it("keeps a ring for its whole life and drops it after", () => {
    expect(isAlive(ring(), 0)).toBe(true);
    expect(isAlive(ring(), LIFE - 0.01)).toBe(true);
    expect(isAlive(ring(), LIFE)).toBe(false);
  });
});

describe("ringGlow", () => {
  it("peaks exactly on the expanding wavefront", () => {
    const now = 1;
    const radius = now * SPEED;
    const onFront = ringGlow(ring(), radius, now);
    expect(onFront).toBeGreaterThan(ringGlow(ring(), radius - 10, now));
    expect(onFront).toBeGreaterThan(ringGlow(ring(), radius + 10, now));
  });

  it("lights nothing beyond the band's half-width", () => {
    const now = 1;
    const radius = now * SPEED;
    expect(ringGlow(ring(), radius + BAND, now)).toBe(0);
    expect(ringGlow(ring(), radius + BAND + 1, now)).toBe(0);
  });

  it("fades linearly with age at the wavefront", () => {
    const at = (now: number) => ringGlow(ring(), now * SPEED, now);
    expect(at(0.6)).toBeCloseTo(1 - 0.6 / LIFE, 10);
    expect(at(1.2)).toBeCloseTo(1 - 1.2 / LIFE, 10);
  });

  it("scales with amplitude, so a splash outshines a drip", () => {
    const now = 1;
    const radius = now * SPEED;
    expect(ringGlow(ring({ amp: 1.2 }), radius, now)).toBeCloseTo(
      2.4 * ringGlow(ring({ amp: 0.5 }), radius, now),
      10,
    );
  });

  it("gives nothing to a dead or unborn ring", () => {
    expect(ringGlow(ring({ t: 5 }), 0, 1)).toBe(0);
    expect(ringGlow(ring(), 0, LIFE + 1)).toBe(0);
  });
});

describe("pointGlow", () => {
  it("sums overlapping rings", () => {
    const now = 1;
    const a = ring({ x: 0, y: 0, amp: 0.3 });
    const b = ring({ x: 2 * SPEED, y: 0, amp: 0.3 });
    const point = SPEED; // on both wavefronts at once
    expect(pointGlow([a, b], point, 0, now)).toBeCloseTo(
      ringGlow(a, SPEED, now) + ringGlow(b, SPEED, now),
      10,
    );
  });

  it("clamps a pile-up to 1 so splashes never blow out", () => {
    const now = 1;
    const many = Array.from({ length: 8 }, () => ring({ amp: 1.2 }));
    expect(pointGlow(many, SPEED, 0, now)).toBe(1);
  });

  it("measures distance radially, not per axis", () => {
    const now = 1;
    const r = SPEED;
    const diagonal = pointGlow([ring()], r * Math.SQRT1_2, r * Math.SQRT1_2, now);
    expect(diagonal).toBeCloseTo(pointGlow([ring()], r, 0, now), 10);
  });

  it("is dark where no ring reaches", () => {
    expect(pointGlow([ring()], 10 * CELL, 10 * CELL, 0.1)).toBe(0);
  });
});

describe("paint passes", () => {
  it("keeps the halo wide and faint and the core tight and bright", () => {
    const halo = haloPaint(1);
    const core = corePaint(1);
    expect(halo.radius).toBeGreaterThan(core.radius);
    expect(halo.alpha).toBeLessThan(core.alpha);
  });

  it("stays inside a legal alpha range at full glow", () => {
    for (const glow of [0, 0.5, 1]) {
      expect(haloPaint(glow).alpha).toBeGreaterThanOrEqual(0);
      expect(corePaint(glow).alpha).toBeLessThanOrEqual(1);
    }
  });

  it("still paints a visible dot at the dimmest glow worth painting", () => {
    expect(corePaint(0.02).radius).toBeGreaterThan(1);
  });
});
