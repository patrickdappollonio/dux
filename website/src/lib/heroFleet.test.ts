import { describe, expect, it } from "vitest";

import {
  AMBIENCE_FIELDS,
  CHANGED_ROWS,
  FEED,
  LATTICE,
  MAX_STEP_SEC,
  PANES,
  PANE_PALETTE,
  RippleRing,
  SWAY_IDLE_MS,
  ambienceGlsl,
  approach,
  clampStep,
  clip,
  fallbackBackgroundImage,
  fallbackBackgroundPosition,
  fallbackBackgroundSize,
  glslFloat,
  grade,
  inkCss,
  rippleOrigin,
  swayOffset,
  swayTarget,
} from "./heroFleet";
import { fragmentShader } from "./heroScene";

const RECT = { left: 100, top: 50, width: 800, height: 400 };

/** Always non-negative, unlike `%`. */
function mod(n: number, m: number): number {
  return ((n % m) + m) % m;
}

describe("RippleRing", () => {
  it("hands out slots round robin and wraps", () => {
    const ring = new RippleRing(3, 0);
    expect([ring.claim(), ring.claim(), ring.claim(), ring.claim()]).toEqual([
      0, 1, 2, 0,
    ]);
  });

  it("rate limits a glide trail but never a press", () => {
    const ring = new RippleRing(8, 120);
    expect(ring.claimGlide(1000)).toBe(0);
    expect(ring.claimGlide(1050)).toBeNull();
    expect(ring.claimGlide(1119)).toBeNull();
    expect(ring.claimGlide(1120)).toBe(1);
    // A press is not part of the trail, so it splashes whatever the clock says.
    expect(ring.claim()).toBe(2);
    expect(ring.claim()).toBe(3);
  });

  it("does not burn a slot on a throttled glide", () => {
    const ring = new RippleRing(8, 120);
    ring.claimGlide(0);
    ring.claimGlide(10);
    ring.claimGlide(20);
    expect(ring.claimGlide(500)).toBe(1);
  });
});

describe("rippleOrigin", () => {
  it("measures from the bottom-left corner, which is where gl_FragCoord counts from", () => {
    expect(rippleOrigin(RECT, 100, 50)).toEqual({ x: 0, y: 400 });
    expect(rippleOrigin(RECT, 900, 450)).toEqual({ x: 800, y: 0 });
    expect(rippleOrigin(RECT, 500, 250)).toEqual({ x: 400, y: 200 });
  });

  it("refuses a pointer outside the frame", () => {
    expect(rippleOrigin(RECT, 99, 250)).toBeNull();
    expect(rippleOrigin(RECT, 901, 250)).toBeNull();
    expect(rippleOrigin(RECT, 500, 49)).toBeNull();
    expect(rippleOrigin(RECT, 500, 451)).toBeNull();
  });
});

describe("idle sway", () => {
  it("takes over only once no mouse has been seen for the idle window", () => {
    expect(swayTarget(10_000, 10_000 - SWAY_IDLE_MS)).toBe(0);
    expect(swayTarget(10_000, 10_000 - SWAY_IDLE_MS - 1)).toBe(1);
  });

  it("is armed from the start on a device that never reports a mouse", () => {
    expect(swayTarget(0, -1e9)).toBe(1);
  });

  it("drifts within the amplitudes the camera translation is tuned for", () => {
    for (let t = 0; t < 200; t += 0.37) {
      const { x, y } = swayOffset(t);
      expect(Math.abs(x)).toBeLessThanOrEqual(0.8);
      expect(Math.abs(y)).toBeLessThanOrEqual(0.55);
    }
  });
});

describe("clampStep", () => {
  it("passes an ordinary frame through", () => {
    expect(clampStep(1 / 60)).toBeCloseTo(1 / 60, 9);
  });

  it("caps a pause the scene slept through", () => {
    expect(clampStep(60)).toBe(MAX_STEP_SEC);
  });

  it("refuses a step that is not forward motion", () => {
    expect(clampStep(0)).toBe(0);
    expect(clampStep(-3)).toBe(0);
    expect(clampStep(Number.NaN)).toBe(0);
  });

  it("keeps the scene clock continuous across a hidden tab", () => {
    // A minute parked, then one ordinary frame. `THREE.Clock` would hand the
    // frame either the whole minute or, after a `start()`, a clock reset to
    // zero; both teleport the panes.
    let t = 10;
    t += clampStep(60);
    t += clampStep(1 / 60);
    expect(t).toBeGreaterThan(10);
    expect(t).toBeLessThan(10 + MAX_STEP_SEC * 2);
  });
});

describe("approach", () => {
  it("covers the same ground per second whatever the frame rate", () => {
    let slow = 0;
    for (let i = 0; i < 60; i++) slow = approach(slow, 1, 1 / 60, 0.06);
    let fast = 0;
    for (let i = 0; i < 144; i++) fast = approach(fast, 1, 1 / 144, 0.06);
    expect(fast).toBeCloseTo(slow, 6);
    expect(slow).toBeCloseTo(1 - 0.06, 6);
  });

  it("stays put when it is already there", () => {
    expect(approach(0.5, 0.5, 0.016, 0.25)).toBe(0.5);
  });
});

describe("pane content", () => {
  it("gives every pane both layouts", () => {
    for (const p of PANES) {
      expect(p.wide).toHaveLength(4);
      expect(p.narrow).toHaveLength(4);
      expect(p.wide.every(Number.isFinite)).toBe(true);
      expect(p.narrow.every(Number.isFinite)).toBe(true);
    }
  });

  it("has exactly one dux miniature, and it is the frontmost pane in both layouts", () => {
    const tui = PANES.filter((p) => p.tui);
    expect(tui).toHaveLength(1);
    const maxWide = Math.max(...PANES.map((p) => p.wide[2]));
    const maxNarrow = Math.max(...PANES.map((p) => p.narrow[2]));
    expect(tui[0].wide[2]).toBe(maxWide);
    expect(tui[0].narrow[2]).toBe(maxNarrow);
  });

  it("paints only colours the depth grader precomputes", () => {
    const known = new Set<string>(PANE_PALETTE);
    for (const [, tint] of FEED) expect(known.has(tint)).toBe(true);
    for (const row of CHANGED_ROWS) expect(known.has(row[3])).toBe(true);
  });
});

describe("clip", () => {
  it("leaves anything that fits alone", () => {
    expect(clip(6)("abcdef")).toBe("abcdef");
  });

  it("spends the last cell on the ellipsis", () => {
    expect(clip(6)("abcdefg")).toBe("abcde…");
    expect(clip(6)("abcdefg")).toHaveLength(6);
  });
});

describe("grade", () => {
  it("is the identity at the front of the fleet", () => {
    expect(grade("#00d4ff", 0)).toBe("rgb(0,212,255)");
  });

  it("both dims and desaturates with depth", () => {
    const near = grade("#4ade80", 0);
    const far = grade("#4ade80", 1);
    const [nr, ng, nb] = /(\d+),(\d+),(\d+)/.exec(near)!.slice(1).map(Number);
    const [fr, fg, fb] = /(\d+),(\d+),(\d+)/.exec(far)!.slice(1).map(Number);
    expect(fg).toBeLessThan(ng);
    // The far copy is closer to grey: its channel spread has collapsed.
    expect(Math.max(fr, fg, fb) - Math.min(fr, fg, fb)).toBeLessThan(
      Math.max(nr, ng, nb) - Math.min(nr, ng, nb),
    );
  });
});

describe("glslFloat", () => {
  it("always carries a decimal point, because GLSL ES has no int-to-float promotion", () => {
    expect(glslFloat(0)).toBe("0.0");
    expect(glslFloat(44)).toBe("44.0");
    expect(glslFloat(-3)).toBe("-3.0");
    expect(glslFloat(0.831)).toBe("0.831");
  });

  it("holds the plain-decimal end of its domain", () => {
    // `Number.prototype.toString` switches to exponential below 1e-6, and the
    // default five places round anything smaller away before it can.
    expect(glslFloat(1e-5)).toBe("0.00001");
    expect(glslFloat(1e-7)).toBe("0.0");
  });

  it("refuses a magnitude it could only write in exponential notation", () => {
    // `1e-7.0` is not a GLSL literal, so this must fail loudly at generation
    // rather than compile-fail the shader in the browser.
    expect(() => glslFloat(1e-7, 7)).toThrow(RangeError);
    expect(() => glslFloat(1e21)).toThrow(RangeError);
    expect(() => glslFloat(Number.NaN)).toThrow(RangeError);
  });
});

describe("the CSS fallback and the shader agree", () => {
  const css = fallbackBackgroundImage("--fold");
  const glsl = ambienceGlsl();

  it("paints the lattice plus one layer per ambience field", () => {
    expect(css.split("radial-gradient(").length - 1).toBe(
      AMBIENCE_FIELDS.length + 1,
    );
    expect(fallbackBackgroundSize().split(", ")).toHaveLength(
      AMBIENCE_FIELDS.length + 1,
    );
    expect(glsl.split("\n")).toHaveLength(AMBIENCE_FIELDS.length);
  });

  it("tiles only the lattice, at the shader's cell pitch", () => {
    expect(fallbackBackgroundSize().startsWith(`${LATTICE.cellPx}px ${LATTICE.cellPx}px`)).toBe(true);
    expect(fallbackBackgroundSize().split(", ").slice(1).every((s) => s === "auto")).toBe(true);
  });

  it("places every field at the same anchor in both", () => {
    // The GLSL side is matched whole and in position rather than by substring:
    // `toContain(glslFloat(f.x))` passes against any line that happens to hold
    // the digits somewhere, including another field's drift amplitude.
    const line =
      /^ {2}c \+= field\(p, vec2\(\((\S+) \+ sin\(uTime \* (\S+) \+ (\S+)\) \* (\S+)\) \* uAspect, (\S+) \+ sin\(uTime \* (\S+) \+ (\S+)\) \* (\S+)\), (\S+), vec3\((\S+), (\S+), (\S+)\), (\S+)\);$/;
    const lines = glsl.split("\n");
    AMBIENCE_FIELDS.forEach((f, i) => {
      const cx = Number(((0.5 + f.x) * 100).toFixed(2));
      const cy = Number(((0.5 - f.y) * 100).toFixed(2));
      expect(css).toContain(`at ${cx}% ${cy}%`);

      const m = line.exec(lines[i]);
      expect(m, lines[i]).not.toBeNull();
      expect(m!.slice(1)).toEqual([
        glslFloat(f.x),
        glslFloat(f.drift.xFreq),
        glslFloat(f.drift.xPhase),
        glslFloat(f.drift.xAmp),
        glslFloat(f.y),
        glslFloat(f.drift.yFreq),
        glslFloat(f.drift.yPhase),
        glslFloat(f.drift.yAmp),
        glslFloat(f.radius),
        ...f.tint.map((c) => glslFloat(c)),
        glslFloat(f.amp),
      ]);
    });
  });

  it("sizes every field's CSS circle against the fold height, never a percentage", () => {
    // Percentages resolve against the box and would squash the shader's circles
    // into ellipses.
    for (const f of AMBIENCE_FIELDS) {
      expect(css).toContain(
        `circle calc(var(--fold) * ${Number((f.radius * 2.2).toFixed(4))})`,
      );
    }
  });

  it("starts every field at its own amplitude and ends at nothing", () => {
    for (const f of AMBIENCE_FIELDS) {
      const rgb = f.tint.map((c) => Math.round(c * 255)).join(", ");
      expect(css).toContain(`rgba(${rgb}, ${f.amp}) 0%`);
      expect(css).toContain(`rgba(${rgb}, 0) 100%`);
    }
  });

  it("puts the CSS lattice dot exactly where the shader puts one", () => {
    // The shader's dots are wherever `mod(px + CELL * 0.5, CELL) - CELL * 0.5`
    // is zero, which is every whole multiple of the cell from the canvas's
    // bottom-left corner. The CSS layer tiles from the top-left, so the two
    // agree only once the tiling is offset. Half a cell out and the lattice
    // slides visibly during the crossfade.
    const cell = LATTICE.cellPx;
    const layers = fallbackBackgroundPosition("--fold").split(", ");
    expect(layers).toHaveLength(AMBIENCE_FIELDS.length + 1);
    expect(layers.slice(1).every((s) => s === "0 0")).toBe(true);

    const pos = /^(-?[\d.]+)px calc\(var\(--fold\) - (-?[\d.]+)px\)$/.exec(
      layers[0],
    );
    expect(pos, layers[0]).not.toBeNull();
    const originX = Number(pos![1]);
    const originYFromTop = (h: number) => h - Number(pos![2]);

    const at = /radial-gradient\(circle at ([\d.]+)% ([\d.]+)%,/.exec(css);
    expect(at, css.slice(0, 80)).not.toBeNull();
    const insideX = (Number(at![1]) / 100) * cell;
    const insideY = (Number(at![2]) / 100) * cell;

    // A dot centre in CSS pixels, then converted to the shader's bottom-left
    // origin, for fold heights that are and are not multiples of the cell.
    for (const h of [880, 900, 44 * 17, 1013.5]) {
      const dotX = originX + insideX;
      const dotYFromBottom = h - (originYFromTop(h) + insideY);
      expect(mod(dotX, cell)).toBeCloseTo(0, 9);
      expect(mod(dotYFromBottom, cell)).toBeCloseTo(0, 9);
    }
  });

  it("rests on the same ink both layers clear to", () => {
    expect(inkCss()).toBe("rgb(7, 7, 13)");
  });

  it("emits no bare integer literal into the shader", () => {
    // Comments go first, then the two places an integer is the correct type:
    // an array size, and the loop counter over the ripple slots. Everything
    // else in the pass is a float, and GLSL ES 1.00 will not promote one.
    const scanned = fragmentShader()
      .replace(/\/\/[^\n]*/g, "")
      .replace(/\[\s*\d+\s*\]/g, "[]")
      .replace(/for \([^)]*\)/g, "for ()");
    for (const literal of scanned.match(/(?<![\w.])-?\d+(\.\d+)?/g) ?? []) {
      expect(literal).toContain(".");
    }
  });
});
