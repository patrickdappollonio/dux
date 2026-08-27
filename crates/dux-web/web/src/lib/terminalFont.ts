// Pure helpers for the web terminal's font stack. No internal imports so these
// stay unit-testable without mounting a Terminal (see terminalFont.test.ts) and
// so other modules (settingsDescriptors.ts) can import the constants below
// with no cycle risk.
import type { Terminal } from "@xterm/xterm"

// The three bundled font-family names, matching the `@font-face` declarations
// in index.css (see terminalFonts.test.ts, which pins all three against the
// CSS).
export const DUX_MONO_FAMILY = "Dux Mono"
export const DUX_MONO_SYMBOLS_FAMILY = "Dux Mono Symbols"
export const DUX_MONO_FILL_FAMILY = "Dux Mono Fill"

// The exact `unicode-range` value on the "Dux Mono Symbols" `@font-face` in
// index.css, exported so terminalFonts.test.ts can pin the two against each
// other without a second hand-copied literal drifting from the CSS.
export const UNICODE_RANGES =
  "U+2190-21FF, U+2300-23FF, U+2500-25FF, U+2600-27BF, U+2800-28FF, U+E0A0-E0D7"

// The same, for the "Dux Mono Fill" `@font-face`: the symbol blocks its subset
// of Adwaita Mono covers.
export const FILL_UNICODE_RANGES =
  "U+2000-2BFF, U+2E00-2E7F, U+1F000-1FBFF"

// The bundled fallback stack. "Dux Mono Symbols" is listed first so structural
// glyphs (box drawing, blocks, braille, arrows, powerline) always come from a
// verified single-cell-advance font rather than whatever the browser picks;
// "Dux Mono" (Roboto Mono) is the ordinary text face and carries none of those
// glyphs itself.
//
// "Dux Mono Fill" comes third, AFTER both curated faces, and that order is
// load-bearing rather than incidental. It is a backstop for code points
// neither face ahead of it carries (U+23F5, U+23F8, U+2714 and the rest of the
// symbol blocks a device with no suitable installed font would otherwise draw
// as tofu), and its declared `unicode-range` is far wider than the handful of
// glyphs that motivated it: it takes in all of U+2000-2BFF, including General
// Punctuation (U+2000-206F).
//
// The ordering rule is simply that the two curated faces win wherever they
// overlap the backstop: the symbols face owns the structural glyphs it was cut
// for, the text face owns ordinary punctuation, and only what both lack may
// reach the fill. What the rule does NOT buy is a single typeface on screen.
// The honest cost is ADJACENCY: the fill supplies code points sitting in the
// very blocks the symbols face already draws, so they render in a different
// typeface right next to their neighbours. Measured against the current cuts,
// the fill adds 77 more Arrows, 94 more Misc Technical, 51 more Box Drawing
// and Geometric Shapes, and 159 more Misc Symbols and Dingbats, so 383 code
// points draw mixed with symbols-face neighbours (braille is untouched, 0
// added). That is accepted: a glyph in a mismatched typeface is readable and
// tofu is not.
//
// The system-font tail is the final backstop for any remaining code point no
// bundled face covers.
export const DUX_TERMINAL_FONT_STACK =
  `"${DUX_MONO_SYMBOLS_FAMILY}", "${DUX_MONO_FAMILY}", "${DUX_MONO_FILL_FAMILY}", ui-monospace, SFMono-Regular, Menlo, monospace`

// The eager-load list `loadTerminalFontsThenRefit` iterates: one entry per
// bundled face, each naming ONE family with a sample that sits inside that
// family's own `unicode-range`. Naming one family per entry is what makes each
// face's fetch independent of where it sits in the stack, and pairing it with
// an in-range sample is what makes the fetch happen at all: `document.fonts`
// only fetches a restricted face for text it actually covers. The samples are
// pinned against the declared ranges by terminalFonts.test.ts.
//
// The capture harness (tools/preview-env/tui-shot.js) declares the same faces
// and the same range literals for its own headless xterm and preloads them the
// same way; the two are deliberately separate copies rather than shared code,
// so a range change here needs the same edit there.
export const TERMINAL_FONT_PRELOADS: readonly {
  family: string
  weight?: "bold"
  sample: string
}[] = [
  { family: DUX_MONO_FAMILY, sample: "Ag" },
  { family: DUX_MONO_FAMILY, weight: "bold", sample: "Ag" },
  // U+2713, U+28FF and U+2500: one each from the Dingbats, Braille and Box
  // Drawing blocks the symbols face is cut for.
  { family: DUX_MONO_SYMBOLS_FAMILY, sample: "✓⣿─" },
  // U+203B sits in the fill face's range and in no other bundled face's, so
  // this sample cannot be satisfied by a neighbour.
  { family: DUX_MONO_FILL_FAMILY, sample: "※✷" },
]

export const MIN_TERMINAL_FONT_SIZE = 8
export const MAX_TERMINAL_FONT_SIZE = 32
export const DEFAULT_TERMINAL_FONT_SIZE = 14

// Longest user-supplied font-family value accepted. Mirrors the server-side
// cap in `wire.rs` `set_settings` (defense in depth, not the only guard).
const MAX_FAMILY_LENGTH = 200

// Characters a legitimate CSS font-family value or comma-separated font list
// can need: letters, digits, space, underscore, hyphen, comma, and straight
// quotes. Anything outside this ASCII allowlist is stripped before the value
// is concatenated ahead of DUX_TERMINAL_FONT_STACK, because that concatenation
// feeds two sinks verbatim: an inline CSS `font-family: <x>;` declaration
// xterm writes into a `<style>` element, and the shorthand string passed to
// `document.fonts.load`. A raw `;`, `{`, `}`, `<`, `>`, backslash, or a
// newline/control character in the value could otherwise terminate or hijack
// the CSS declaration. This is a deliberate ASCII-only tradeoff: a family
// name that uses non-ASCII characters (accented Latin, CJK, etc.) degrades
// safely to the bundled stack alone rather than being preserved, because a
// character class wide enough to admit legitimate non-ASCII font names is
// much harder to prove safe than a narrow one.
const SAFE_FAMILY_CHARS = /[^A-Za-z0-9 _\-,'"]/g

function sanitizeFontFamily(value: string): string {
  return value.replace(SAFE_FAMILY_CHARS, "").slice(0, MAX_FAMILY_LENGTH)
}

// Builds the font-family value xterm should use. An empty/whitespace-only or
// missing user family means "use the bundled default only." A real value is
// sanitized (see sanitizeFontFamily) and placed ahead of the bundled stack, so
// the user's font wins for the glyphs it has and the bundled faces backstop
// what it lacks. A value that sanitizes away to nothing (e.g. it was made
// entirely of stripped characters) also falls back to the bundled stack.
export function terminalFontFamily(
  userFamily: string | null | undefined,
): string {
  const trimmed = userFamily?.trim()
  if (!trimmed) {
    return DUX_TERMINAL_FONT_STACK
  }
  const safe = sanitizeFontFamily(trimmed)
  if (!safe) {
    return DUX_TERMINAL_FONT_STACK
  }
  return `${safe}, ${DUX_TERMINAL_FONT_STACK}`
}

// Clamps an arbitrary config/settings value to a valid terminal font size. A
// value outside [MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE] (including
// anything that isn't a finite number: unparsable strings, NaN, Infinity)
// degrades to DEFAULT_TERMINAL_FONT_SIZE rather than being clamped to the
// nearer bound, matching the server's documented semantics in
// `normalized_terminal_font_size` (crates/dux-core/src/config.rs): a value
// that is merely wrong reads as an obviously-reset default, not a
// silently-nudged number. The Preferences dialog's own NumberControl clamps
// to the nearest bound while the user is typing (an input affordance, see
// `clampToControl` in CustomizeWebappDialog.tsx); that is a different,
// intentionally different behavior from this function, which handles values
// arriving from config/bootstrap rather than live keystrokes.
export function clampTerminalFontSize(value: unknown): number {
  if (value === null || value === undefined || value === "") {
    return DEFAULT_TERMINAL_FONT_SIZE
  }
  const num = typeof value === "number" ? value : Number(value)
  if (!Number.isFinite(num)) {
    return DEFAULT_TERMINAL_FONT_SIZE
  }
  const rounded = Math.round(num)
  if (rounded < MIN_TERMINAL_FONT_SIZE || rounded > MAX_TERMINAL_FONT_SIZE) {
    return DEFAULT_TERMINAL_FONT_SIZE
  }
  return rounded
}

// Waits for the bundled terminal faces (plus the current user family, if any)
// to be ready, then refits an already-OPEN terminal so its cell metrics track
// the real glyphs rather than whatever fallback font was active when it
// opened. Called right after `term.open()` on mount (against fallback
// metrics: opening synchronously rather than awaiting fonts first keeps the
// PTY connection from being delayed by a font fetch) and again from the
// live-apply effect after a Preferences font change. Races against a 2s
// timeout and swallows a rejection (best-effort): a font that fails to load
// (offline first load, a corrupt cache entry) must never leave the terminal
// stuck, it just keeps whatever metrics are already in effect, same as
// before this feature existed. `document.fonts` is absent in some test
// environments (jsdom); this is a no-op there.
//
// Visual tradeoff, stated explicitly: on a cold first load the terminal opens
// against fallback metrics for one frame, then refits once the bundled font
// arrives (a standard FOUT). On a warm cache `document.fonts.load` resolves
// near-instantly, so there is no visible reflow.
//
// The REFIT ITSELF is the caller's, passed in rather than performed here,
// because there are two right answers and this module cannot tell them apart:
// an owner refits to its container, while a watcher rendering faithfully must
// never do that (its grid is the PTY's) and instead recomputes its shrink
// font. Both callers pass the mode-correct closure.
export function loadTerminalFontsThenRefit(
  term: Terminal,
  termRef: { current: Terminal | null },
  refitNow: () => void,
  size: number,
  family: string,
): void {
  if (typeof document.fonts?.load !== "function") {
    return
  }
  // Every bundled face is loaded explicitly, one family per call, from
  // TERMINAL_FONT_PRELOADS, so no face's fetch depends on where its family
  // sits in the stack: not on a user family prepended ahead of it, not on a
  // reordering, not on a range recut. A face that is not fetched before xterm
  // measures it gets a fallback advance cached in its place, and every row
  // carrying one of its glyphs is dragged sideways.
  //
  // Measured (headless Chromium, the four faces declared exactly as index.css
  // declares them): a whole-stack shorthand loads EVERY family in the list
  // whose range covers a sample code point, not just the one CSS matching
  // would pick. `document.fonts.load` on the stack with a "█⣿" sample
  // returns ["Dux Mono Symbols 400", "Dux Mono 400", "Dux Mono Fill 400"] and
  // leaves all three `loaded`, so the fill face's ~79 KB is paid on every
  // mount either way; naming it makes that deliberate.
  //
  // The user's own family is loaded separately below, through the whole
  // sanitized stack, because there is no declared face to name for it.
  //
  // ACCEPTED RACE, cold cache only: a terminal opens synchronously against
  // fallback metrics (deliberate, so the PTY connection is not held up by a
  // font fetch), and xterm's DOM renderer caches the glyph advances it
  // measured. Reading xterm 6.0: the options setter no-ops when the new value
  // equals the old, WidthCache.setFont no-ops on an equal font tuple, and
  // clearTextureAtlas is unimplemented for the DOM renderer, so there is no
  // sanctioned way to invalidate that cache without a real font change. A
  // glyph painted in that first cold frame therefore keeps a fallback advance
  // until the font size or family actually changes. The refit below fixes the
  // cell grid, which is what governs layout; the residue is per-glyph
  // letter-spacing on characters drawn before the faces resolved, and it is
  // accepted rather than worked around.
  const refit = () => {
    if (termRef.current === term) refitNow()
  }
  void Promise.race([
    Promise.all([
      ...TERMINAL_FONT_PRELOADS.map((preload) =>
        document.fonts.load(
          `${preload.weight ? `${preload.weight} ` : ""}${size}px "${preload.family}"`,
          preload.sample,
        ),
      ),
      document.fonts.load(`${size}px ${family}`, "Ag"),
    ]),
    new Promise((resolve) => setTimeout(resolve, 2000)),
  ])
    .then(refit)
    .catch(() => {
      // Several faces are loaded together, so a rejection says only that one
      // of them failed, never which. Naming the configured family here would
      // accuse the user's font of a failure that was just as likely a bundled
      // face.
      console.warn("dux: a terminal font failed to load; keeping current metrics")
      refit()
    })
}
