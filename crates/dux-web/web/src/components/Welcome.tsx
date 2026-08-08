import { useState } from "react"

import { useDux } from "@/lib/store"

// The dux welcome screen, mirroring the TUI's idle agent pane: the duck mark
// over the block-letter logo, with one playful tip underneath. The duck is the
// real PNG logo (the same `/dux-logo.png` the login/unreachable screens use);
// The wordmark deliberately DIVERGES from the TUI's block logo
// (crates/dux-tui/src/app/render.rs ASCII_LOGO): that one is drawn with
// shade/block glyphs (U+2591/U+2588), which terminal fonts carry but many
// system monospace fonts do not — in a browser the missing glyphs fall back
// to a different font with a different advance width and the art shears,
// device-dependently. Every character here is pure ASCII, which every
// monospace font renders aligned. Trailing padding keeps the lines a clean
// 28-column rectangle so the block centers properly.
const TEXT_LOGO = [
  "      888                   ",
  "      888                   ",
  "  .d88888 888  888 888  888 ",
  " d88\" 888 888  888  Y8bd8P' ",
  " 888  888 888  888   X88K   ",
  " Y88b 888 Y88b 888  d8\"8b.  ",
  "  \"Y88888  \"Y88888 888  888 ",
].join("\n")

// Tips come from the server's ViewModel — the single source of truth is crates/dux-core/src/welcome.rs (WELCOME_TIPS). Add new tips THERE, with both surface variants.

// Render a tip, highlighting `backticked` spans in the foreground accent
// (the backticks themselves are not shown) — same convention as the TUI.
function TipText({ tip }: { tip: string }) {
  // No platform localization pass here: the web has no command palette and no
  // keyboard shortcuts of its own, so web tips name affordances (the cog menu,
  // buttons, the ⋯ menu) rather than keys. See the `web` field's doc in
  // crates/dux-core/src/welcome.rs.
  const parts = tip.split("`")
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
  const tips = useDux().bootstrap?.welcome_tips ?? []
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
    <div className="flex h-full w-full select-none flex-col items-center justify-center gap-3 overflow-hidden">
      <img
        src="/dux-logo.png"
        alt=""
        aria-hidden
        className="size-28 object-contain"
      />
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
