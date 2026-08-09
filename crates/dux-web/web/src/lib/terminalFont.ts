// Pure helpers for the web terminal's font stack. No internal imports so these
// stay unit-testable without mounting a Terminal (see terminalFont.test.ts) and
// so other modules (settingsDescriptors.ts) can import the constants below
// with no cycle risk.
import type { FitAddon } from "@xterm/addon-fit"
import type { Terminal } from "@xterm/xterm"

// The two bundled font-family names, matching the `@font-face` declarations
// in index.css (see terminalFonts.test.ts, which pins both against the CSS).
export const DUX_MONO_FAMILY = "Dux Mono"
export const DUX_MONO_SYMBOLS_FAMILY = "Dux Mono Symbols"

// The exact `unicode-range` value on the "Dux Mono Symbols" `@font-face` in
// index.css, exported so terminalFonts.test.ts can pin the two against each
// other without a second hand-copied literal drifting from the CSS.
export const UNICODE_RANGES =
  "U+2190-21FF, U+2300-23FF, U+2500-25FF, U+2600-27BF, U+2800-28FF, U+E0A0-E0D7"

// The bundled fallback stack. "Dux Mono Symbols" is listed first so structural
// glyphs (box drawing, blocks, braille, arrows, powerline) always come from a
// verified single-cell-advance font rather than whatever the browser picks;
// "Dux Mono" (Roboto Mono) is the ordinary text face and carries none of those
// glyphs itself. The system-font tail is a last-resort backstop for any
// remaining code point neither bundled face covers.
export const DUX_TERMINAL_FONT_STACK =
  `"${DUX_MONO_SYMBOLS_FAMILY}", "${DUX_MONO_FAMILY}", ui-monospace, SFMono-Regular, Menlo, monospace`

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
export function loadTerminalFontsThenRefit(
  term: Terminal,
  termRef: { current: Terminal | null },
  fitAddonRef: { current: FitAddon | null },
  size: number,
  family: string,
): void {
  if (typeof document.fonts?.load !== "function") {
    return
  }
  // The sample text includes a glyph from the unicode-range-restricted
  // symbols face (█⣿) so the browser actually loads THAT face rather than
  // only the always-matched text face.
  const sample = "█⣿"
  const refit = () => {
    if (termRef.current === term) fitAddonRef.current?.fit()
  }
  void Promise.race([
    Promise.all([
      document.fonts.load(`${size}px "${DUX_MONO_FAMILY}"`, sample),
      document.fonts.load(`bold ${size}px "${DUX_MONO_FAMILY}"`, sample),
      document.fonts.load(`${size}px "${DUX_MONO_SYMBOLS_FAMILY}"`, sample),
      document.fonts.load(`${size}px ${family}`, sample),
    ]),
    new Promise((resolve) => setTimeout(resolve, 2000)),
  ])
    .then(refit)
    .catch(() => {
      console.warn(`dux: terminal font "${family}" failed to load; keeping current metrics`)
      refit()
    })
}
