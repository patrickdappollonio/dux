/**
 * The parts of the homepage hero that hold still: its palette, its pane
 * contents, its background arithmetic and the small rules that decide when a
 * ripple is allowed to spawn and when the camera starts swaying on its own.
 *
 * They live apart from `heroScene.ts` because the CSS fallback layer and the
 * WebGL pass must agree on the same numbers: the canvas crossfades in over the
 * CSS layer, so any divergence reads as a jump. Both are generated from the
 * values below rather than written down twice.
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
 * Depth grading, baked into the palette instead of being left to fog alone:
 * fog only pulls colours toward the ink, which dims without desaturating, and a
 * far pane full of saturated green still shouts.
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
 * Round-robin allocator for the shader's fixed ripple array. Eight slots is
 * enough at the throttled spawn rate: the oldest slot is already dead of old
 * age by the time the newest overwrites it, so the array never truncates a ring
 * the eye can still see.
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
 * `pointermove` never fires on a touch screen, so a phone would otherwise sit
 * perfectly still. The camera takes over whenever no mouse has been seen
 * lately, which covers both a touch device and an abandoned desktop tab.
 */
export function swayTarget(nowMs: number, lastFineMs: number): number {
  return nowMs - lastFineMs > SWAY_IDLE_MS ? 1 : 0;
}

/** The autonomous drift the sway blends the pointer parallax toward. */
export function swayOffset(t: number): { x: number; y: number } {
  return { x: Math.sin(t * 0.11) * 0.8, y: Math.sin(t * 0.077 + 1.3) * 0.55 };
}

/**
 * The frame clock's largest step, in seconds. A longer gap is a pause the
 * scene slept through rather than motion anybody watched: a hidden tab, a
 * throttled background, a stalled main thread.
 */
export const MAX_STEP_SEC = 0.05;

/**
 * The seconds a frame is allowed to advance the scene by.
 *
 * The elapsed time is accumulated from these rather than read off a
 * `THREE.Clock`, which offers no way to hold it across a pause: letting the
 * clock run hands the frame after a hidden tab the whole pause at once, and
 * `Clock.start()` resets its elapsed time to zero, so resuming from it sends
 * the scene back to the beginning instead. Either way the panes teleport and
 * every live ripple lands outside its own lifetime.
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
 * The gaussian sampled at 0, 1, 1.5 and 2.2 radii, which is where it has
 * effectively reached nothing. A CSS radial gradient has no gaussian, so the
 * fallback layer traces the shader's curve through these four stops.
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
 * `background-image` for the pure-CSS layer painted under the canvas. Without
 * it the hero is an empty hole until the first WebGL frame lands; it is also
 * what the hero falls back to when WebGL never comes up, and what a scripts-off
 * visitor sees.
 *
 * The fields are circles in the shader's aspect-corrected space, so their radii
 * are fractions of the hero's HEIGHT and are written against `foldVar` rather
 * than as percentages, which would resolve against the box and squash the
 * circles into ellipses.
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
 * `background-position` for the same layers, which is what puts the CSS
 * lattice on the shader's grid rather than half a cell off it.
 *
 * The shader's dots sit where `mod(px + CELL * 0.5, CELL) - CELL * 0.5` is
 * zero, which is every whole multiple of the cell measured from the canvas's
 * BOTTOM-LEFT corner, because that is where `gl_FragCoord` counts from. The
 * CSS layer tiles from the top-left with the dot in the middle of its tile, so
 * the tiling is shifted by half a cell in x, and by the frame's height less
 * half a cell in y, to turn the top-left origin back into a bottom-left one.
 * Both shifts wrap modulo the cell, which is what a repeating background does
 * with any position, so the fold height need not be a multiple of anything.
 *
 * Without this the two layers disagree by half a cell and the lattice visibly
 * slides during the canvas crossfade.
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
    literal has to carry a decimal point even when it is a whole number.
    Refuses anything JavaScript would print in exponential notation, below
    about 1e-6 and at 1e21, because `1e-7.0` is not a GLSL literal: the scene's
    constants are nowhere near either bound, so this is a guard on the domain
    rather than a conversion. */
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
