/**
 * The homepage hero's fleet: floating terminal panes over an analytic
 * background pass, driven by three.js. Everything that can be decided without a
 * canvas lives in `heroFleet.ts` and is unit tested there; this module is the
 * WebGL orchestration around it.
 */
import * as THREE from "three";

import {
  ACCENT,
  AGENT_ROWS,
  CHANGED_ROWS,
  DIM,
  ERROR,
  FEED,
  INK_RGB,
  LATTICE,
  MUTED,
  NARROW_WIDTH_PX,
  PANES,
  PANE_PALETTE,
  RIPPLE,
  RippleRing,
  SPINNER_FRAMES,
  SUCCESS,
  TEXT,
  WARNING,
  ambienceGlsl,
  approach,
  clampStep,
  clip,
  glslFloat,
  grade,
  rippleOrigin,
  swayOffset,
  swayTarget,
  type PaneDef,
} from "./heroFleet";

/** Pane texture size. Fixed, because the panes are small on screen and a
    texture per pane per resize would cost more than the sharpness is worth. */
const W = 640;
const H = 400;
const PAD = 12;
const TITLE = 36;
/** Rows of scrollback a pane holds, which is what its height fits. */
const ROWS = 13;
/** A pane repaints at about 7fps; its texture is the expensive part of a frame
    and nothing in it moves faster than the spinner. */
const REDRAW_SEC = 0.14;

const MONO = "'JetBrains Mono', ui-monospace, monospace";

interface Pane {
  def: PaneDef;
  ctx: CanvasRenderingContext2D;
  mat: THREE.MeshBasicMaterial;
  tex: THREE.CanvasTexture;
  mesh: THREE.Mesh;
  baseX: number;
  baseY: number;
  z: number;
  depth: number;
  pal: Record<string, string>;
  phase: number;
  feed: number;
  lines: Array<readonly [string, string]>;
  nextLine: number;
  nextDraw: number;
}

// `CanvasRenderingContext2D.roundRect` is only in Safari 16.4 and Firefox 112
// and later, and it throws rather than no-ops where it is missing: once per
// pane per redraw, forever, because the loop reschedules before the throw. The
// arcTo path draws the same rectangle everywhere else.
const HAS_ROUND_RECT =
  typeof CanvasRenderingContext2D !== "undefined" &&
  typeof CanvasRenderingContext2D.prototype.roundRect === "function";

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  ctx.beginPath();
  if (HAS_ROUND_RECT) {
    ctx.roundRect(x, y, w, h, r);
    return;
  }
  const rr = Math.min(r, w / 2, h / 2);
  ctx.moveTo(x + rr, y);
  ctx.arcTo(x + w, y, x + w, y + h, rr);
  ctx.arcTo(x + w, y + h, x, y + h, rr);
  ctx.arcTo(x, y + h, x, y, rr);
  ctx.arcTo(x, y, x + w, y, rr);
  ctx.closePath();
}

export function fragmentShader(): string {
  const [ir, ig, ib] = INK_RGB;
  return `
precision highp float;

varying vec2 vUv;
uniform float uAspect;
uniform float uTime;
uniform float uDpr;
uniform vec4 uRipples[${RIPPLE.slots}];

// A ShaderMaterial carries no colorspace chunk, so whatever this writes is what
// lands in the framebuffer: these are sRGB bytes, and with the amplitudes at
// zero the result is exactly --color-ink.
const vec3 INK = vec3(${glslFloat(ir)}, ${glslFloat(ig)}, ${glslFloat(ib)}) / 255.0;

const float CELL = ${glslFloat(LATTICE.cellPx)};
const float DOT_CORE = ${glslFloat(LATTICE.shaderCoreRadiusPx)};
const float DOT_EDGE = ${glslFloat(LATTICE.edgeRadiusPx)};
const float DOT_AMP = ${glslFloat(LATTICE.amp)};
const float RIPPLE_SPEED = ${glslFloat(RIPPLE.speedPxPerSec)};
const float RIPPLE_BAND = ${glslFloat(RIPPLE.bandPx)};
const float RIPPLE_LIFE = ${glslFloat(RIPPLE.lifeSec)};
const vec3 ACCENT = vec3(0.0, 0.831, 1.0);
const vec3 LATTICE = vec3(${LATTICE.tint.map((c) => glslFloat(c / 255)).join(", ")});

vec3 field(vec2 p, vec2 at, float radius, vec3 tint, float amp) {
  vec2 d = (p - at) / radius;
  return tint * (amp * exp(-dot(d, d)));
}

void main() {
  // Anchors are fractions of the full width and height measured from the
  // centre, corrected here for aspect, so the composition survives the frustum
  // narrowing to a phone's.
  vec2 p = (vUv - 0.5) * vec2(uAspect, 1.0);

  vec3 c = INK;
${ambienceGlsl()}

  // Everything below is strictly additive over the ambience, so the pass keeps
  // its two guarantees: no term can render a pixel darker than the backdrop,
  // and none of them has an edge.

  // The lattice is anchored in CSS pixels rather than in the aspect-normalised
  // p, so the dots stay square and stay CELL apart whatever the viewport ratio
  // does. Dividing out the device pixel ratio is what makes gl_FragCoord a
  // CSS-pixel coordinate.
  vec2 px = gl_FragCoord.xy / uDpr;
  float rd = length(mod(px + CELL * 0.5, CELL) - CELL * 0.5);

  float glow = 0.0;
  for (int i = 0; i < ${RIPPLE.slots}; i++) {
    vec4 ring = uRipples[i];
    if (ring.w <= 0.0) continue;
    float age = uTime - ring.z;
    if (age < 0.0 || age >= RIPPLE_LIFE) continue;
    float front = age * RIPPLE_SPEED;
    // A gaussian on the signed distance to the expanding front: soft on both
    // shoulders, so a wavefront has no rim of its own to trace even where two
    // of them overlap.
    float band = (length(px - ring.xy) - front) / RIPPLE_BAND;
    // Two fades multiply: age, and the front's own radius, so a ring thins out
    // as its energy is spread over a longer circle.
    glow += ring.w
      * (1.0 - age / RIPPLE_LIFE)
      * exp(-band * band)
      / (1.0 + front / 1100.0);
  }
  glow = min(glow, 1.4);

  // Resting lattice: just above the ambience, so it reads as the grain of the
  // backdrop rather than as a pattern competing with the panes.
  c += LATTICE * (DOT_AMP * (1.0 - smoothstep(DOT_CORE, DOT_EDGE, rd)));

  if (glow > 0.0) {
    float lit = min(glow, 1.0);
    // The crest whitens; everything else stays the site accent.
    vec3 tint = mix(ACCENT, vec3(1.0), smoothstep(0.55, 1.15, glow) * 0.65);
    float coreR = 1.2 + lit * 1.9;
    float core = 1.0 - smoothstep(coreR - 0.75, coreR + 0.75, rd);
    float halo = 1.0 - smoothstep(0.0, 4.5 + lit * 4.0, rd);
    // A whisper of the wavefront itself between the dots: without it a slow
    // ring reads as unrelated dots blinking in sequence rather than as one
    // front travelling through them.
    c += tint * (core * lit * 1.10 + halo * halo * lit * 0.20 + glow * 0.035);
  }

  // Interleaved gradient noise at a single level: the fields cross an 8-bit
  // step every few hundred pixels, which is exactly the width at which a
  // quantisation boundary reads as a ring.
  float ign = fract(
    52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
  c += (ign - 0.5) / 255.0;

  gl_FragColor = vec4(c, 1.0);
}
`;
}

const VERTEX_SHADER = `
varying vec2 vUv;
void main() {
  vUv = uv;
  gl_Position = vec4(position.xy, 0.0, 1.0);
}
`;

export interface HeroScene {
  destroy(): void;
}

/**
 * Starts the hero scene on `canvas`, sized and pointed at by `hero`. Returns
 * `null` when WebGL is unavailable, which is the caller's cue to leave the CSS
 * fallback layer showing.
 */
export function mountHeroScene(
  hero: HTMLElement,
  canvas: HTMLCanvasElement,
): HeroScene | null {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  let renderer: THREE.WebGLRenderer;
  try {
    renderer = new THREE.WebGLRenderer({
      canvas,
      antialias: true,
      alpha: true,
      powerPreference: "low-power",
    });
  } catch {
    return null;
  }

  renderer.setClearAlpha(0);
  // The background pass owns the clear: it paints first, into the same
  // framebuffer, so the fleet blends over ambience rather than over nothing.
  renderer.autoClear = false;

  const scene = new THREE.Scene();
  scene.fog = new THREE.Fog(
    (INK_RGB[0] << 16) | (INK_RGB[1] << 8) | INK_RGB[2],
    11,
    33,
  );

  const camera = new THREE.PerspectiveCamera(46, 1, 0.5, 80);
  camera.position.set(0, -0.3, 11);

  const group = new THREE.Group();
  scene.add(group);

  // Ambience is one fullscreen analytic pass, not sprites. A glow sprite on
  // this canvas composited as `sprite + (1 - accumulated alpha) * page`:
  // additive blending accumulates DESTINATION alpha too, so a sprite punched a
  // hole in the page's own gradients and handed back far less light than it
  // hid, rendering darker than the backdrop it was meant to lift.
  const bgScene = new THREE.Scene();
  const bgCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
  // Each slot is (x, y, birth, strength), the origin in CSS pixels from the
  // canvas's bottom-left corner. A zero strength is the empty slot, which is
  // also the state the whole array stays in under reduced motion.
  const ripples = Array.from(
    { length: RIPPLE.slots },
    () => new THREE.Vector4(0, 0, -1000, 0),
  );
  const bgUniforms = {
    uAspect: { value: 1 },
    uTime: { value: 0 },
    uDpr: { value: 1 },
    uRipples: { value: ripples },
  };
  const bgMaterial = new THREE.ShaderMaterial({
    uniforms: bgUniforms,
    depthTest: false,
    depthWrite: false,
    transparent: false,
    fog: false,
    vertexShader: VERTEX_SHADER,
    fragmentShader: fragmentShader(),
  });
  const bgQuad = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), bgMaterial);
  bgQuad.frustumCulled = false;
  bgScene.add(bgQuad);

  const paneGeometry = new THREE.PlaneGeometry(3.5, 3.5 * (H / W));

  function makePane(def: PaneDef, index: number): Pane {
    const c = document.createElement("canvas");
    c.width = W;
    c.height = H;
    const ctx = c.getContext("2d") as CanvasRenderingContext2D;
    // Deliberately no `colorSpace`: three then treats these sRGB canvas bytes
    // as linear and re-encodes them on output, which brightens them. The pane
    // colours were tuned by eye against exactly that pipeline and the approved
    // screenshots were rendered through it, so setting SRGBColorSpace here
    // would change the signed-off look. That is a redesign, not a fix.
    const tex = new THREE.CanvasTexture(c);
    tex.minFilter = THREE.LinearFilter;
    tex.generateMipmaps = false;
    tex.anisotropy = 4;

    const mat = new THREE.MeshBasicMaterial({
      map: tex,
      transparent: true,
      depthWrite: false,
      fog: true,
    });
    const mesh = new THREE.Mesh(paneGeometry, mat);
    group.add(mesh);

    return {
      def,
      ctx,
      mat,
      tex,
      mesh,
      baseX: 0,
      baseY: 0,
      z: 0,
      depth: 0,
      pal: {},
      phase: index * 1.31 + 0.4,
      feed: Math.floor(Math.random() * FEED.length),
      lines: [],
      nextLine: 0,
      nextDraw: 0,
    };
  }

  // Applied on every resize, because the two layouts differ in z as well as in
  // x and y, and z decides depth grading and draw order.
  function place(p: Pane, narrow: boolean): void {
    const [x, y, z, s] = narrow ? p.def.narrow : p.def.wide;
    p.baseX = x;
    p.baseY = y;
    p.mesh.position.set(x, y, z);
    p.mesh.scale.setScalar(s);
    p.mesh.renderOrder = Math.round(z * 100);
    p.z = z;
    p.depth = Math.min(1, Math.max(0, (0.6 - z) / 9));
    p.mat.color.setScalar(1 - p.depth * 0.22);
    p.pal = {};
    for (const key of PANE_PALETTE) p.pal[key] = grade(key, p.depth);
    p.nextDraw = 0;
  }

  function drawChrome(
    p: Pane,
    t: number,
  ): { bw: number; bh: number; top: number } {
    const ctx = p.ctx;
    const attention = p.def.state === "attention";
    const d = p.depth;
    ctx.clearRect(0, 0, W, H);

    const bw = W - PAD * 2;
    const bh = H - PAD * 2;

    // So a pane still separates from an ambient glow it is floating in front of.
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,0.55)";
    ctx.shadowBlur = 14;
    ctx.shadowOffsetY = 5;
    const bg = ctx.createLinearGradient(0, PAD, 0, H - PAD);
    bg.addColorStop(0, `rgba(20,21,38,${0.96 - d * 0.06})`);
    bg.addColorStop(1, `rgba(11,12,22,${0.97 - d * 0.06})`);
    roundRect(ctx, PAD, PAD, bw, bh, 11);
    ctx.fillStyle = bg;
    ctx.fill();
    ctx.restore();

    ctx.save();
    roundRect(ctx, PAD, PAD, bw, bh, 11);
    ctx.clip();

    const tb = ctx.createLinearGradient(0, PAD, 0, PAD + TITLE);
    tb.addColorStop(0, "rgba(31,33,56,0.95)");
    tb.addColorStop(1, "rgba(22,22,40,0.95)");
    ctx.fillStyle = tb;
    ctx.fillRect(PAD, PAD, bw, TITLE);
    ctx.fillStyle = `rgba(255,255,255,${0.05 - d * 0.02})`;
    ctx.fillRect(PAD, PAD, bw, 1);
    ctx.fillStyle = `rgba(42,46,76,${0.95 - d * 0.25})`;
    ctx.fillRect(PAD, PAD + TITLE, bw, 1);

    const cy = PAD + TITLE / 2;
    ctx.textBaseline = "middle";
    ctx.font = `600 15px ${MONO}`;

    if (p.def.state === "run") {
      ctx.fillStyle = p.pal[SUCCESS];
      ctx.fillText(
        SPINNER_FRAMES[Math.floor(t * 11) % SPINNER_FRAMES.length],
        PAD + 14,
        cy,
      );
    } else if (attention) {
      ctx.fillStyle = Math.floor(t * 1.6) % 2 ? p.pal[ACCENT] : "rgba(0,212,255,0.15)";
      ctx.beginPath();
      ctx.arc(PAD + 20, cy, 5, 0, Math.PI * 2);
      ctx.fill();
    } else {
      ctx.fillStyle = p.pal[DIM];
      ctx.fillText("○", PAD + 14, cy);
    }

    ctx.fillStyle = p.pal[TEXT];
    ctx.fillText(clip(30)(p.def.name), PAD + 38, cy);

    ctx.font = `500 13px ${MONO}`;
    const label = p.def.agent;
    const chipW = ctx.measureText(label).width + 18;
    const chipX = W - PAD - 12 - chipW;
    roundRect(ctx, chipX, cy - 11, chipW, 22, 6);
    ctx.fillStyle = `rgba(31,33,56,${0.9 - d * 0.2})`;
    ctx.fill();
    ctx.lineWidth = 1;
    ctx.strokeStyle = `rgba(42,46,76,${0.9 - d * 0.3})`;
    ctx.stroke();
    ctx.fillStyle = p.pal[p.def.tui ? ACCENT : MUTED];
    ctx.fillText(label, chipX + 9, cy);

    return { bw, bh, top: PAD + TITLE + 1 };
  }

  function drawBorder(p: Pane, front: boolean): void {
    const ctx = p.ctx;
    const attention = p.def.state === "attention";
    ctx.restore();
    ctx.lineWidth = front || attention ? 2 : 1.5;
    ctx.strokeStyle = attention
      ? "rgba(0,212,255,0.7)"
      : front
        ? "rgba(0,212,255,0.4)"
        : `rgba(46,50,82,${0.95 - p.depth * 0.3})`;
    roundRect(ctx, PAD, PAD, W - PAD * 2, H - PAD * 2, 11);
    ctx.stroke();
    p.tex.needsUpdate = true;
  }

  function drawFeedPane(p: Pane, t: number, front: boolean): void {
    const ctx = p.ctx;
    const box = drawChrome(p, t);
    const cut = clip(66);

    ctx.font = `500 13.5px ${MONO}`;
    ctx.textBaseline = "top";
    const rows = p.lines.slice(-ROWS);
    rows.forEach((row, i) => {
      ctx.fillStyle = p.pal[row[1]] || row[1];
      ctx.fillText(cut(row[0]), PAD + 14, box.top + 12 + i * 23);
    });

    const y = box.top + 12 + rows.length * 23;
    if (p.def.state === "run" && Math.floor(t * 2) % 2 === 0) {
      ctx.fillStyle = p.pal[ACCENT];
      ctx.fillRect(PAD + 14, y + 2, 8, 14);
    }
    if (p.def.state === "attention") {
      ctx.fillStyle = p.pal[ACCENT];
      ctx.fillText("waiting on you", PAD + 14, y);
    }
    drawBorder(p, front);
  }

  // The dux pane draws dux itself: sidebar, centre terminal, changed files.
  function drawTuiPane(p: Pane, t: number, front: boolean): void {
    const ctx = p.ctx;
    const box = drawChrome(p, t);
    const x0 = PAD + 1;
    const y0 = box.top;
    const w = box.bw - 2;
    const h = H - PAD - y0;
    const side = Math.round(w * 0.34);
    const right = Math.round(w * 0.27);
    const midX = x0 + side;
    const rightX = x0 + w - right;
    const rule = `rgba(42,46,76,${0.9 - p.depth * 0.3})`;

    ctx.fillStyle = rule;
    ctx.fillRect(midX, y0, 1, h);
    ctx.fillRect(rightX, y0, 1, h);

    ctx.textBaseline = "middle";
    const head = `600 11px ${MONO}`;
    const body = `500 12px ${MONO}`;

    ctx.font = head;
    ctx.fillStyle = p.pal[DIM];
    ctx.fillText("AGENTS", x0 + 12, y0 + 16);
    ctx.font = body;
    const sideCut = clip(Math.floor((side - 34) / 7.2));
    AGENT_ROWS.forEach((row, i) => {
      const ry = y0 + 42 + i * 34;
      if (i === 0) {
        ctx.fillStyle = `rgba(31,33,56,${0.95 - p.depth * 0.2})`;
        ctx.fillRect(x0 + 4, ry - 13, side - 10, 27);
        ctx.fillStyle = p.pal[ACCENT];
        ctx.fillRect(x0 + 4, ry - 13, 2, 27);
      }
      ctx.fillStyle = p.pal[row[1] === "Working" ? SUCCESS : DIM];
      ctx.fillText(row[1] === "Working" ? "◆" : "◇", x0 + 14, ry - 5);
      ctx.fillStyle = p.pal[TEXT];
      ctx.fillText(sideCut(row[0]), x0 + 30, ry - 5);
      if (row[1] === "Working" || row[1] === "Idle") {
        ctx.font = head;
        ctx.fillStyle = p.pal[row[1] === "Working" ? SUCCESS : DIM];
        ctx.fillText(row[1], x0 + 30, ry + 10);
        ctx.font = body;
      } else if (row[1] === "dot") {
        ctx.fillStyle = Math.floor(t * 1.6) % 2 ? p.pal[ACCENT] : "rgba(0,212,255,0.2)";
        ctx.beginPath();
        ctx.arc(x0 + side - 18, ry - 5, 4, 0, Math.PI * 2);
        ctx.fill();
      }
    });

    ctx.textBaseline = "top";
    ctx.font = `500 11.5px ${MONO}`;
    const midCut = clip(Math.floor((rightX - midX - 22) / 6.9));
    const rows = p.lines.slice(-ROWS);
    rows.forEach((row, i) => {
      ctx.fillStyle = p.pal[row[1]] || row[1];
      ctx.fillText(midCut(row[0]), midX + 12, y0 + 12 + i * 19);
    });
    if (Math.floor(t * 2) % 2 === 0) {
      ctx.fillStyle = p.pal[ACCENT];
      ctx.fillRect(midX + 12, y0 + 14 + rows.length * 19, 7, 12);
    }

    ctx.textBaseline = "middle";
    ctx.font = head;
    ctx.fillStyle = p.pal[DIM];
    ctx.fillText("CHANGES", rightX + 12, y0 + 16);
    const fileCut = clip(Math.floor((right - 34) / 6.9));
    CHANGED_ROWS.forEach((row, i) => {
      const ry = y0 + 44 + i * 32;
      ctx.font = `600 11.5px ${MONO}`;
      ctx.fillStyle =
        p.pal[row[0] === "D" ? ERROR : row[0] === "A" ? SUCCESS : WARNING];
      ctx.fillText(row[0], rightX + 12, ry);
      ctx.font = `500 11.5px ${MONO}`;
      ctx.fillStyle = p.pal[TEXT];
      ctx.fillText(fileCut(row[1]), rightX + 26, ry);
      ctx.font = `500 10.5px ${MONO}`;
      ctx.fillStyle = p.pal[row[3]];
      ctx.fillText(row[2], rightX + 26, ry + 13);
    });

    drawBorder(p, front);
  }

  const panes = PANES.map(makePane);
  for (const p of panes) {
    const n = p.def.state === "idle" ? 7 : 10 + Math.floor(Math.random() * 3);
    for (let i = 0; i < n; i++) {
      p.lines.push(FEED[p.feed % FEED.length]);
      p.feed++;
    }
  }

  let front = panes[0];

  const pointer = { x: 0, y: 0, tx: 0, ty: 0 };
  let lastFine = -1e9;
  const ring = new RippleRing();
  // The shader reads the same clock the frame loop feeds `uTime`, so impulses
  // are stamped with the last rendered scene time rather than with
  // `performance.now()`: at most one frame stale, and never on a different
  // timebase from the front that grows out of it.
  let sceneTime = 0;

  function onPointerMove(e: PointerEvent): void {
    if (e.pointerType && e.pointerType !== "mouse") return;
    lastFine = performance.now();
    const r = hero.getBoundingClientRect();
    pointer.tx = ((e.clientX - r.left) / r.width) * 2 - 1;
    pointer.ty = -(((e.clientY - r.top) / r.height) * 2 - 1);
  }
  function onPointerLeave(): void {
    pointer.tx = 0;
    pointer.ty = 0;
  }
  hero.addEventListener("pointermove", onPointerMove);
  hero.addEventListener("pointerleave", onPointerLeave);

  function dropRipple(slot: number, clientX: number, clientY: number, amp: number): void {
    const at = rippleOrigin(hero.getBoundingClientRect(), clientX, clientY);
    if (!at) return;
    ripples[slot].set(at.x, at.y, sceneTime, amp);
  }

  function onGlide(e: PointerEvent): void {
    // A hover-capable touchscreen laptop reports finger drags here too, and a
    // finger is meant to splash on a tap rather than leave a trail.
    if (e.pointerType && e.pointerType !== "mouse") return;
    const slot = ring.claimGlide(performance.now());
    if (slot === null) return;
    dropRipple(slot, e.clientX, e.clientY, RIPPLE.glideAmp);
  }
  function onPress(e: PointerEvent): void {
    dropRipple(ring.claim(), e.clientX, e.clientY, RIPPLE.pressAmp);
  }

  // Reduced motion gets a still lattice and no ripple listeners, so the slots
  // stay empty and the ripple branch in the shader never fires. The parallax
  // listeners above are unconditional; under reduced motion nothing renders
  // after the still frame, so they move nothing.
  const hoverCapable = window.matchMedia("(hover: hover)").matches;
  if (!reduced) {
    // A glide trail needs a cursor that hovers. A finger dragging has no such
    // state, and spawning off touch-moves would fight the page's own scrolling;
    // a tap still splashes, through pointerdown.
    if (hoverCapable) {
      hero.addEventListener("pointermove", onGlide, { passive: true });
    }
    hero.addEventListener("pointerdown", onPress, { passive: true });
  }

  function advance(p: Pane, t: number): void {
    if (p.def.state !== "run") return;
    if (t < p.nextLine) return;
    p.nextLine = t + 0.7 + Math.random() * 1.9;
    p.lines.push(FEED[p.feed % FEED.length]);
    p.feed++;
    if (p.lines.length > ROWS) p.lines.shift();
  }

  let sway = 0;
  let live = false;

  function frame(t: number, dt: number): void {
    const step = clampStep(dt) || 1 / 60;

    sway = approach(sway, swayTarget(performance.now(), lastFine), step, 0.25);
    const drift = swayOffset(t);
    const tx = pointer.tx * (1 - sway) + drift.x * sway;
    const ty = pointer.ty * (1 - sway) + drift.y * sway;

    pointer.x = approach(pointer.x, tx, step, 0.06);
    pointer.y = approach(pointer.y, ty, step, 0.06);
    group.rotation.y = pointer.x * 0.085;
    group.rotation.x = pointer.y * -0.055;
    // The sway carries extra translation of its own. Rotation alone stays at
    // the few degrees a parallax tilt is worth, and a few degrees is not enough
    // travel to read as motion on a phone-sized frustum.
    group.position.x = pointer.x * 0.4 + sway * drift.x * 0.7;
    group.position.y = pointer.y * 0.14 + sway * drift.y * 0.4;

    for (const p of panes) {
      p.mesh.position.x = p.baseX + Math.sin(t * 0.19 + p.phase) * 0.3;
      p.mesh.position.y = p.baseY + Math.sin(t * 0.26 + p.phase * 1.7) * 0.24;
      p.mesh.rotation.z = Math.sin(t * 0.14 + p.phase) * 0.011;
      advance(p, t);
      if (t >= p.nextDraw) {
        p.nextDraw = t + REDRAW_SEC;
        if (p.def.tui) drawTuiPane(p, t, p === front);
        else drawFeedPane(p, t, p === front);
      }
    }

    sceneTime = t;
    bgUniforms.uTime.value = t;
    renderer.clear();
    renderer.render(bgScene, bgCamera);
    renderer.render(scene, camera);
    if (!live) {
      live = true;
      canvas.classList.add("is-live");
    }
  }

  function resize(): void {
    const w = hero.clientWidth || 1;
    const h = hero.clientHeight || 1;
    const narrow = w < NARROW_WIDTH_PX;
    // The CSS layer sizes its circles and phases its lattice against the fold
    // height; the shader uses the canvas's real one. They diverge when the copy
    // overflows the fold, and the crossfade pops. The stylesheet's default is
    // left in place for a scripts-off visit, where there is no shader to
    // disagree with.
    hero.style.setProperty("--hero-fold-h", `${h}px`);
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    // The lattice is measured in CSS pixels, so the shader needs the ratio it
    // must divide gl_FragCoord by. Read it back from the renderer rather than
    // from `window`, so the clamp above cannot be missed here.
    bgUniforms.uDpr.value = renderer.getPixelRatio();
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    bgUniforms.uAspect.value = w / h;
    camera.fov = narrow ? 66 : 50;
    camera.updateProjectionMatrix();
    for (const p of panes) place(p, narrow);
    front = panes.reduce((a, b) => (a.z > b.z ? a : b));
    if (reduced) frame(2.6, 0.016);
  }

  const observer = new ResizeObserver(resize);
  observer.observe(hero);
  resize();

  let raf = 0;
  let onScreen = true;
  // Accumulated by hand, so the scene's clock holds still while it is parked
  // rather than either racing ahead or restarting. See `clampStep`.
  let elapsed = 0;
  let lastMs = 0;
  const loop = (): void => {
    raf = requestAnimationFrame(loop);
    const nowMs = performance.now();
    const dt = clampStep((nowMs - lastMs) / 1000);
    lastMs = nowMs;
    elapsed += dt;
    frame(elapsed, dt);
  };
  const start = (): void => {
    if (!raf) {
      lastMs = performance.now();
      raf = requestAnimationFrame(loop);
    }
  };
  const stop = (): void => {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  };
  const sync = (): void => {
    if (onScreen && !document.hidden) start();
    else stop();
  };

  // Off-screen and hidden tabs pay nothing: the hero is a full fold, so the
  // scene is idle for most of a visit.
  const intersection = new IntersectionObserver(
    (entries) => {
      onScreen = entries[0].isIntersecting;
      sync();
    },
    { threshold: 0 },
  );

  if (reduced) {
    frame(2.6, 0.016);
  } else {
    document.addEventListener("visibilitychange", sync);
    intersection.observe(hero);
    sync();
  }

  return {
    destroy() {
      stop();
      observer.disconnect();
      intersection.disconnect();
      document.removeEventListener("visibilitychange", sync);
      hero.removeEventListener("pointermove", onPointerMove);
      hero.removeEventListener("pointerleave", onPointerLeave);
      hero.removeEventListener("pointermove", onGlide);
      hero.removeEventListener("pointerdown", onPress);
      for (const p of panes) {
        p.tex.dispose();
        p.mat.dispose();
      }
      paneGeometry.dispose();
      bgMaterial.dispose();
      bgQuad.geometry.dispose();
      renderer.dispose();
    },
  };
}
