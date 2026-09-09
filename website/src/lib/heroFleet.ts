/**
 * The parts of the homepage hero that hold still: its palette, pane contents,
 * background arithmetic and spawn rules. Apart from `heroScene.ts` because the
 * canvas crossfades in over the CSS fallback layer, so both are generated from
 * these values rather than written down twice and left to diverge.
 */

/** Site `--color-ink`, as the shader's sRGB byte triple. */
export const INK_RGB: [number, number, number] = [7, 7, 13];

export const TEXT = "#f4f7ff";
export const MUTED = "#aab2d5";
export const DIM = "#828bac";
export const ACCENT = "#00d4ff";
export const SUCCESS = "#4ade80";
export const WARNING = "#facc15";
export const ERROR = "#fb7185";
export const PURPLE = "#8b5cf6";

/** Every colour a pane may paint with, so a grade pass can precompute them. */
export const PANE_PALETTE = [
  TEXT,
  MUTED,
  DIM,
  ACCENT,
  SUCCESS,
  WARNING,
  ERROR,
  PURPLE,
] as const;

export const SPINNER_FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

/** Scrollback the feed panes cycle through, as (line, colour) pairs. */
export const FEED: ReadonlyArray<readonly [string, string]> = [
  ["running 41 tests", DIM],
  ["test worktree::adopts_existing ... ok", SUCCESS],
  ["test gate::refuses_second_driver ... ok", SUCCESS],
  ["   Compiling dux-core v0.9.1", MUTED],
  ["    Finished dev profile in 6.42s", SUCCESS],
  ["@@ -118,7 +118,9 @@ impl Engine {", PURPLE],
  ["+    let decision = self.tab_resume(id);", SUCCESS],
  ["-    let decision = Resume::Always;", ERROR],
  ["  3 files changed, 41 insertions(+)", MUTED],
  ["reading crates/dux-web/src/api.rs", DIM],
  ["$ git worktree add ../agent-search", TEXT],
  ["Preparing worktree (new branch)", DIM],
  ["warning: unused import: `Duration`", WARNING],
  ["patched lib/termkeys.ts, 2 hunks", MUTED],
  ["test statusline::busy_gets_a_final ... ok", SUCCESS],
];

/** Rows in the miniature dux TUI's sidebar, as (branch, state, tint). */
export const AGENT_ROWS: ReadonlyArray<readonly [string, string, string]> = [
  ["feature/search-index", "Working", SUCCESS],
  ["fix/pty-owner-race", "", ""],
  ["spike/tailscale-auto", "dot", ACCENT],
  ["chore/bump-deps", "Idle", DIM],
];

/** Rows in the miniature dux TUI's changes pane, as (status, path, stat, tint). */
export const CHANGED_ROWS: ReadonlyArray<
  readonly [string, string, string, string]
> = [
  ["M", "engine/tabs.rs", "+18 -4", SUCCESS],
  ["M", "web/api.rs", "+7 -7", SUCCESS],
  ["A", "lib/termkeys.ts", "+62", SUCCESS],
  ["D", "old/gate.rs", "-31", ERROR],
];

export type PaneState = "run" | "idle" | "attention";

export interface PaneDef {
  name: string;
  agent: string;
  state: PaneState;
  /** The dux TUI miniature; everything else is a plain output feed. */
  tui?: boolean;
  /** Wide layout, as (x, y, z, scale). */
  wide: readonly [number, number, number, number];
  /** Narrow layout. Authored separately rather than scaled from the wide one:
      a fleet that reads as composed at 1440 collapses into overlap once the
      frustum narrows to a phone's. */
  narrow: readonly [number, number, number, number];
}

export const PANES: readonly PaneDef[] = [
  {
    name: "dux",
    agent: "dux",
    state: "run",
    tui: true,
    wide: [1.9, 1.35, 0.4, 1.14],
    narrow: [0.0, 3.0, 1.2, 0.94],
  },
  {
    name: "feature/search-index",
    agent: "claude",
    state: "run",
    wide: [-6.6, 2.2, -3.4, 0.96],
    narrow: [-3.2, 0.9, -3.6, 0.66],
  },
  {
    name: "fix/pty-owner-race",
    agent: "codex",
    state: "run",
    wide: [-2.4, 5.9, -5.2, 0.86],
    narrow: [-2.9, 6.5, -2.2, 0.7],
  },
  {
    name: "spike/tailscale-auto",
    agent: "opencode",
    state: "attention",
    wide: [7.2, 2.0, -2.8, 0.98],
    narrow: [3.4, 0.7, -4.2, 0.66],
  },
  {
    name: "chore/bump-deps",
    agent: "copilot",
    state: "idle",
    wide: [3.3, 5.6, -6.0, 0.84],
    narrow: [2.8, 6.9, -2.8, 0.7],
  },
  {
    name: "fix/status-line-keys",
    agent: "codex",
    state: "run",
    wide: [-10.4, 5.5, -7.2, 0.8],
    narrow: [-4.4, 4.2, -5.6, 0.6],
  },
  {
    name: "docs/config-comments",
    agent: "copilot",
    state: "run",
    wide: [9.4, 5.9, -7.6, 0.8],
    narrow: [4.6, 4.5, -6.0, 0.6],
  },
  {
    name: "feature/web-editor",
    agent: "claude",
    state: "run",
    wide: [-7.4, -2.1, -5.6, 0.88],
    narrow: [-1.5, -1.7, -6.4, 0.58],
  },
  {
    name: "feature/macro-picker",
    agent: "claude",
    state: "idle",
    wide: [3.0, -3.3, -3.2, 0.9],
    narrow: [-1.8, 2.0, -5.6, 0.62],
  },
  {
    name: "fix/worktree-cleanup",
    agent: "opencode",
    state: "run",
    wide: [9.2, -1.0, -4.0, 0.92],
    narrow: [2.3, -2.3, -6.8, 0.58],
  },
  {
    name: "chore/theme-tokens",
    agent: "opencode",
    state: "run",
    wide: [7.4, -4.8, -6.4, 0.82],
    narrow: [2.4, 1.6, -6.6, 0.6],
  },
];

/** Below this hero width the narrow pane layout and a wider field of view. */
export const NARROW_WIDTH_PX = 760;

/**
 * Depth grading baked into the palette rather than left to fog, which only pulls
 * colours toward the ink and dims without desaturating.
 */
export function grade(hex: string, depth: number): string {
  const n = parseInt(hex.slice(1), 16);
  let r = (n >> 16) & 255;
  let g = (n >> 8) & 255;
  let b = n & 255;
  const lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  const desat = depth * 0.55;
  const dim = 1 - depth * 0.3;
  r = Math.round((r + (lum - r) * desat) * dim);
  g = Math.round((g + (lum - g) * desat) * dim);
  b = Math.round((b + (lum - b) * desat) * dim);
  return `rgb(${r},${g},${b})`;
}

/** Truncates to a character budget, because pane text is drawn into a canvas
    that will not wrap or ellipsize for us. */
export function clip(chars: number): (s: string) => string {
  return (s) => (s.length > chars ? s.slice(0, chars - 1) + "…" : s);
}

// -- Background ambience ---------------------------------------------------

export interface AmbienceField {
  /** Anchor, as a fraction of the frame's full width and height, measured
      from its centre: 0.5 is the right edge or the top one. The shader
      corrects for aspect, so the composition survives the frustum narrowing to
      a phone's. */
  x: number;
  y: number;
  /** Gaussian radius, as a fraction of the frame's HEIGHT. */
  radius: number;
  /** Linear sRGB-ish triple in 0..1, matching the shader's own units. */
  tint: readonly [number, number, number];
  amp: number;
  /** Slow wander, so the fields never sit exactly still. */
  drift: {
    xAmp: number;
    xFreq: number;
    xPhase: number;
    yAmp: number;
    yFreq: number;
    yPhase: number;
  };
}

export const AMBIENCE_FIELDS: readonly AmbienceField[] = [
  {
    x: -0.22,
    y: 0.1,
    radius: 0.58,
    tint: [0.0, 0.831, 1.0],
    amp: 0.058,
    drift: {
      xAmp: 0.03,
      xFreq: 0.019,
      xPhase: 0,
      yAmp: 0.025,
      yFreq: 0.014,
      yPhase: 1.7,
    },
  },
  {
    x: 0.26,
    y: -0.02,
    radius: 0.62,
    tint: [0.545, 0.361, 0.965],
    amp: 0.044,
    drift: {
      xAmp: 0.03,
      xFreq: 0.011,
      xPhase: 2.4,
      yAmp: 0.02,
      yFreq: 0.017,
      yPhase: 0.6,
    },
  },
  {
    x: -0.04,
    y: 0.4,
    radius: 0.5,
    tint: [0.22, 0.741, 0.973],
    amp: 0.03,
    drift: {
      xAmp: 0.03,
      xFreq: 0.009,
      xPhase: 4.1,
      yAmp: 0.02,
      yFreq: 0.013,
      yPhase: 3.2,
    },
  },
];

export const LATTICE = {
  /** The site's own 44px grid pitch, shared with `.grid-bg`. */
  cellPx: 44,
  /** sRGB bytes, the CSS layer's units. */
  tint: [143, 161, 214] as const,
  amp: 0.055,
  /** The shader feathers the dot between these two radii; the CSS layer, which
      has no smoothstep, approximates the same footprint with a hard-ish stop
      partway up the ramp. */
  shaderCoreRadiusPx: 0.7,
  cssCoreRadiusPx: 1.1,
  edgeRadiusPx: 1.7,
};

/** Where the lattice dot sits inside its own CSS tile, as a percentage. */
const LATTICE_ANCHOR_PCT = 50;

export const RIPPLE = {
  slots: 8,
  /** One impulse per this many milliseconds of sustained motion. A fast drag
      across the hero would otherwise overwrite the whole array in a frame or
      two and leave a smear instead of separated rings. */
  throttleMs: 120,
  speedPxPerSec: 210,
  bandPx: 34,
  lifeSec: 3,
  /** Amplitude of a hover trail impulse and of a press. */
  glideAmp: 0.7,
  pressAmp: 1.6,
};

/**
 * Round-robin allocator for the shader's fixed ripple array. At the throttled
 * spawn rate the oldest slot is already dead of old age when the newest
 * overwrites it, so the array never truncates a ring the eye can still see.
 */
export class RippleRing {
  private slot = 0;
  private lastGlide = -Infinity;

  constructor(
    private readonly slots = RIPPLE.slots,
    private readonly throttleMs = RIPPLE.throttleMs,
  ) {}

  /** A press, which always splashes. */
  claim(): number {
    const index = this.slot;
    this.slot = (this.slot + 1) % this.slots;
    return index;
  }

  /** A hover trail, which is rate limited. `null` means "too soon". */
  claimGlide(nowMs: number): number | null {
    if (nowMs - this.lastGlide < this.throttleMs) return null;
    this.lastGlide = nowMs;
    return this.claim();
  }
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * Pointer position in the shader's coordinates: CSS pixels from the canvas's
 * bottom-left corner, which is where `gl_FragCoord` counts from once the device
 * pixel ratio is divided out. `null` for a pointer outside the frame.
 */
export function rippleOrigin(
  rect: Rect,
  clientX: number,
  clientY: number,
): { x: number; y: number } | null {
  const x = clientX - rect.left;
  const y = rect.height - (clientY - rect.top);
  if (x < 0 || y < 0 || x > rect.width || y > rect.height) return null;
  return { x, y };
}

// -- Idle sway -------------------------------------------------------------

/** How long a fine pointer suppresses the autonomous camera sway for. */
export const SWAY_IDLE_MS = 3000;

/**
 * `pointermove` never fires on a touch screen, so the camera takes over whenever
 * no mouse has been seen lately: a touch device or an abandoned desktop tab.
 */
export function swayTarget(nowMs: number, lastFineMs: number): number {
  return nowMs - lastFineMs > SWAY_IDLE_MS ? 1 : 0;
}

/** The autonomous drift the sway blends the pointer parallax toward. */
export function swayOffset(t: number): { x: number; y: number } {
  return { x: Math.sin(t * 0.11) * 0.8, y: Math.sin(t * 0.077 + 1.3) * 0.55 };
}

/**
 * The frame clock's largest step, in seconds. A longer gap is a pause the scene
 * slept through rather than motion anybody watched.
 */
export const MAX_STEP_SEC = 0.05;

/**
 * The seconds a frame may advance the scene by. Elapsed time accumulates from
 * these rather than off a `THREE.Clock`, which cannot be held across a pause:
 * letting it run hands the frame after a hidden tab the whole pause at once, and
 * `Clock.start()` resets elapsed time to zero. Either way the panes teleport.
 */
export function clampStep(deltaSec: number): number {
  if (!(deltaSec > 0)) return 0;
  return Math.min(deltaSec, MAX_STEP_SEC);
}

/**
 * Frame-rate independent easing: `retention` is the fraction of the remaining
 * distance still left after one second, so the drift reads the same on a 60Hz
 * and a 144Hz display.
 */
export function approach(
  current: number,
  target: number,
  stepSec: number,
  retention: number,
): number {
  return current + (target - current) * (1 - Math.pow(retention, stepSec));
}

// -- Generated background ---------------------------------------------------

function round(n: number, places: number): string {
  return Number(n.toFixed(places)).toString();
}

/**
 * A CSS radial gradient has no gaussian, so the fallback layer traces the
 * shader's curve through stops sampled where the gaussian reaches nothing.
 */
const FALLBACK_STOPS: ReadonlyArray<[number, number]> = [
  [0, 1],
  [45, 0.37],
  [68, 0.105],
  [100, 0],
];

/** The extent of the CSS gradient, in gaussian radii. */
const FALLBACK_EXTENT = 2.2;

/**
 * `background-image` for the pure-CSS layer under the canvas: what the hero shows
 * before the first WebGL frame, when WebGL never comes up, and with scripts off.
 * The fields are circles in the shader's aspect-corrected space, so their radii
 * are fractions of the hero's HEIGHT, written against `foldVar` rather than as
 * percentages, which would resolve against the box and squash them into ellipses.
 */
export function fallbackBackgroundImage(foldVar: string): string {
  const [lr, lg, lb] = LATTICE.tint;
  // Centred in its own tile, and the tiling itself is then offset by
  // `fallbackBackgroundPosition` so the dot lands where the shader puts one.
  const lattice =
    `radial-gradient(circle at ${LATTICE_ANCHOR_PCT}% ${LATTICE_ANCHOR_PCT}%, ` +
    `rgba(${lr}, ${lg}, ${lb}, ${LATTICE.amp}) 0, ` +
    `rgba(${lr}, ${lg}, ${lb}, ${LATTICE.amp}) ${LATTICE.cssCoreRadiusPx}px, ` +
    `rgba(${lr}, ${lg}, ${lb}, 0) ${LATTICE.edgeRadiusPx}px)`;

  const fields = AMBIENCE_FIELDS.map((f) => {
    const r = round(f.radius * FALLBACK_EXTENT, 4);
    const cx = round((0.5 + f.x) * 100, 2);
    const cy = round((0.5 - f.y) * 100, 2);
    const rgb = f.tint.map((c) => Math.round(c * 255)).join(", ");
    const stops = FALLBACK_STOPS.map(
      ([at, k]) => `rgba(${rgb}, ${round(f.amp * k, 3)}) ${at}%`,
    ).join(", ");
    return `radial-gradient(circle calc(var(${foldVar}) * ${r}) at ${cx}% ${cy}%, ${stops})`;
  });

  return [lattice, ...fields].join(", ");
}

/** Only the lattice tiles; the fields are one frame-sized circle each. */
export function fallbackBackgroundSize(): string {
  return [
    `${LATTICE.cellPx}px ${LATTICE.cellPx}px`,
    ...AMBIENCE_FIELDS.map(() => "auto"),
  ].join(", ");
}

/**
 * `background-position` for the same layers, which puts the CSS lattice on the
 * shader's grid rather than half a cell off it, where the lattice would visibly
 * slide during the canvas crossfade. The shader's dots sit at whole multiples of
 * the cell from the canvas's BOTTOM-LEFT corner, where `gl_FragCoord` counts
 * from, so the top-left CSS tiling is shifted by half a cell in x and by the
 * frame's height less half a cell in y. Both shifts wrap modulo the cell.
 */
export function fallbackBackgroundPosition(foldVar: string): string {
  const half = LATTICE.cellPx * (LATTICE_ANCHOR_PCT / 100);
  return [
    `${half}px calc(var(${foldVar}) - ${half}px)`,
    ...AMBIENCE_FIELDS.map(() => "0 0"),
  ].join(", ");
}

export function inkCss(): string {
  return `rgb(${INK_RGB.join(", ")})`;
}

/** GLSL ES 1.00 has no implicit int-to-float conversion, so every generated
    literal carries a decimal point. Refuses anything JavaScript would print in
    exponential notation, because `1e-7.0` is not a GLSL literal. */
export function glslFloat(n: number, places = 5): string {
  const s = round(n, places);
  if (!Number.isFinite(n) || /[eE]/.test(s)) {
    throw new RangeError(
      `glslFloat: ${n} has no plain-decimal GLSL literal at ${places} places`,
    );
  }
  return s.includes(".") ? s : `${s}.0`;
}

/** The shader's ambience accumulation, generated so it cannot drift from the
    CSS layer above. */
export function ambienceGlsl(): string {
  return AMBIENCE_FIELDS.map((f) => {
    const d = f.drift;
    const x =
      `(${glslFloat(f.x)} + sin(uTime * ${glslFloat(d.xFreq)} + ${glslFloat(d.xPhase)})` +
      ` * ${glslFloat(d.xAmp)}) * uAspect`;
    const y =
      `${glslFloat(f.y)} + sin(uTime * ${glslFloat(d.yFreq)} + ${glslFloat(d.yPhase)})` +
      ` * ${glslFloat(d.yAmp)}`;
    const tint = f.tint.map((c) => glslFloat(c)).join(", ");
    return `  c += field(p, vec2(${x}, ${y}), ${glslFloat(f.radius)}, vec3(${tint}), ${glslFloat(f.amp)});`;
  }).join("\n");
}
