import { useState } from "react"

import { paletteShortcutLabel } from "@/lib/platform"
import { useDux } from "@/lib/store"

// The dux welcome screen, mirroring the TUI's idle agent pane: the braille
// duck over the block-letter logo, with one playful tip underneath. Art is
// ported verbatim from crates/dux-tui/src/app/render.rs (ASCII_LOGO_ALT and
// ASCII_LOGO) so both surfaces share a face.
const DUCK_ART = `⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣤⠤⣄⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠔⠉⠀⠀⢸⢰⢸⢰⢰⠉⠢⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡔⢰⠀⢸⢰⢸⢸⢸⢸⢸⢸⢸⢸⢈⢦⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⣰⢠⢸⠤⣤⠤⢸⢸⢸⢸⢸⠤⠤⢐⢸⢸⡆⠀⠀⠀⠀⠀⠀⠀
⢠⠀⠀⠀⠀⠀⠀⠀⠀⢹⢸⢸⢸⢸⢸⣤⢰⢲⢤⣄⢸⢸⢸⢸⢸⡇⠀⠀⠀⠀⠀⠀⠀
⠀⠙⡄⠀⠀⠀⠀⠀⠀⠀⣄⢸⢰⣶⣿⣤⣤⣶⣤⣤⣬⣷⡤⢸⣰⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠈⢦⠀⠀⠀⠀⠀⠀⠈⢦⢸⢈⠛⠿⣿⣼⣼⠿⠛⠁⢸⡼⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠸⠉⢻⣍⠉⠤⣀⣠⣤⣾⢸⣿⣿⣿⣶⣾⣾⣿⣿⢸⢸⠓⠤⣄⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠈⠒⠭⣀⢸⢸⢈⢈⢸⢸⢸⠙⠻⢸⢸⢸⠿⠛⢸⢸⢸⢸⢸⢈⠑⢄⠀⠀⠀⠀
⠀⠀⠀⠀⠀⣼⢸⢸⢈⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣠⠤⠒⠁⢸⣦⠀⠀⠀
⠀⠀⠀⠀⠀⣿⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢩⡂⢸⢸⣀⣴⣿⠀⠀⠀
⠀⠀⠀⠀⠀⠹⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢉⠉⠉⠁⢸⠃⠀⠀⠀
⠀⠀⠀⠀⠀⠀⢳⢨⠘⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣴⢸⢸⢸⢸⡟⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠳⣀⢠⢨⢘⢘⢸⢸⢸⢸⢸⢸⢸⢸⣤⣿⢸⢸⢸⣀⠛⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠀⠀⠀⠀⠀⠀⠀⠀`

const TEXT_LOGO = `       ░██
       ░██
 ░████████ ░██    ░██ ░██    ░██
░██    ░██ ░██    ░██  ░██  ░██
░██    ░██ ░██    ░██   ░█████
░██   ░███ ░██   ░███  ░██  ░██
 ░█████░██  ░█████░██ ░██    ░██ `

// Tips come from the server's ViewModel — the single source of truth is crates/dux-core/src/welcome.rs (WELCOME_TIPS). Add new tips THERE, with both surface variants.

// Render a tip, highlighting `backticked` spans in the foreground accent
// (the backticks themselves are not shown) — same convention as the TUI.
function TipText({ tip }: { tip: string }) {
  // Server tips reference the palette as ⌘K (the canonical strings live in
  // dux-core and can't know the client's platform); swap in the label for the
  // key this machine actually has (Ctrl K outside Apple platforms).
  const localized = tip.replaceAll("\u2318K", paletteShortcutLabel())
  const parts = localized.split("`")
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <span key={i} className="font-medium text-foreground">
            {part}
          </span>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  )
}

export function Welcome() {
  const tips = useDux().viewModel?.welcome_tips ?? []
  // Pick a stable random fraction once per visit to the welcome screen (the
  // component remounts whenever the center pane returns to the idle state).
  // Storing the fraction — not an index — keeps the choice stable across
  // ViewModel re-renders while still working when tips arrive AFTER mount.
  const [tipFraction] = useState(() => Math.random())
  const tip =
    tips.length > 0
      ? tips[Math.floor(tipFraction * tips.length) % tips.length]
      : null

  return (
    <div className="flex h-full w-full select-none flex-col items-center justify-center gap-1 overflow-hidden">
      <pre
        aria-hidden
        className="font-mono text-[11px] leading-[1.15] text-amber-500/70"
      >
        {DUCK_ART}
      </pre>
      <pre
        aria-label="dux"
        className="font-mono text-[11px] leading-[1.15] text-muted-foreground"
      >
        {TEXT_LOGO}
      </pre>
      {tip && (
        <p className="mt-6 max-w-md px-6 text-center text-sm text-muted-foreground">
          <TipText tip={tip} />
        </p>
      )}
    </div>
  )
}
