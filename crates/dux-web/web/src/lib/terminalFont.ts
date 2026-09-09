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

// The bundled fallback stack, whose order is load-bearing: the two curated
// faces must win wherever they overlap the fill, whose declared `unicode-range`
// is far wider than the tofu it exists to cover. The symbols face leads so
// structural glyphs (box drawing, blocks, braille, arrows, powerline) come from
// a verified single-cell-advance font; "Dux Mono" owns ordinary text; the fill
// supplies only what both lack, at the cost of drawing some symbol-block code
// points in a typeface differing from their neighbours. The system tail
// backstops whatever no bundled face covers.
export const DUX_TERMINAL_FONT_STACK =
  `"${DUX_MONO_SYMBOLS_FAMILY}", "${DUX_MONO_FAMILY}", "${DUX_MONO_FILL_FAMILY}", ui-monospace, SFMono-Regular, Menlo, monospace`

// One entry per bundled face, each naming ONE family so its fetch is
// independent of where it sits in the stack, with a sample inside that family's
// own `unicode-range` because `document.fonts` only fetches a restricted face
// for text it covers. tools/preview-env/tui-shot.js keeps its own copy of these
// faces and ranges for its headless xterm, so a range change needs both edits.
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

// Anything outside this allowlist is stripped, because the value is
// concatenated ahead of DUX_TERMINAL_FONT_STACK into two verbatim sinks: the
// inline CSS `font-family` declaration xterm writes into a `<style>` element,
// and the shorthand passed to `document.fonts.load`. Non-ASCII family names
// (accented Latin, CJK) therefore degrade to the bundled stack: a class wide
// enough to admit them is much harder to prove safe than a narrow one.
const SAFE_FAMILY_CHARS = /[^A-Za-z0-9 _\-,'"]/g

function sanitizeFontFamily(value: string): string {
  return value.replace(SAFE_FAMILY_CHARS, "").slice(0, MAX_FAMILY_LENGTH)
}

// The font-family value xterm should use: a sanitized user family ahead of the
// bundled stack, so the user's font wins for the glyphs it has. A missing,
// blank, or wholly-stripped value falls back to the bundled stack alone.
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

// A config/bootstrap value resolved to a valid terminal font size. Anything out
// of range or not a finite number degrades to DEFAULT_TERMINAL_FONT_SIZE rather
// than being clamped to the nearer bound, matching the server's
// `normalized_terminal_font_size` (crates/dux-core/src/config.rs): a wrong value
// must read as an obviously-reset default. `clampToControl` in
// CustomizeWebappDialog.tsx deliberately differs, clamping live keystrokes.
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

// Waits for the bundled faces plus the current user family, then refits an
// already-OPEN terminal so its cell metrics track the real glyphs. Best effort:
// it races a 2s timeout and swallows a rejection, so a font that never loads
// leaves the existing metrics in place rather than wedging the terminal, and it
// is a no-op where `document.fonts` is absent (jsdom).
//
// `refitNow` is the caller's because the two modes disagree and this module
// cannot tell them apart: an owner refits to its container, a watcher must not
// (its grid is the PTY's) and recomputes its shrink font instead.
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
  // One call per bundled face, so no face's fetch depends on where its family
  // sits in the stack. A face xterm measures before it is fetched gets a
  // fallback advance cached in its place, dragging every row of its glyphs
  // sideways. The user's own family is loaded through the whole sanitized
  // stack below, because it has no declared face to name.
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
      // Several faces load together, so a rejection never says which failed:
      // naming the configured family would accuse the user's font at random.
      console.warn("dux: a terminal font failed to load; keeping current metrics")
      refit()
    })
}
