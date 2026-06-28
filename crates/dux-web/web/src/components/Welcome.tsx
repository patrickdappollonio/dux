import { useState } from "react"

import { paletteShortcutLabel } from "@/lib/platform"
import { useDux } from "@/lib/store"

// The dux welcome screen, mirroring the TUI's idle agent pane: the duck mark
// over the block-letter logo, with one playful tip underneath. The duck is the
// real PNG logo (the same `/dux-logo.png` the login/unreachable screens use);
// the block-letter wordmark is ported VERBATIM from crates/dux-tui/src/app/
// render.rs (ASCII_LOGO), trailing padding included so the lines stay a clean
// 33-column rectangle and center properly.
const TEXT_LOGO = [
  "       ░██                       ",
  "       ░██                       ",
  " ░████████ ░██    ░██ ░██    ░██ ",
  "░██    ░██ ░██    ░██  ░██  ░██  ",
  "░██    ░██ ░██    ░██   ░█████   ",
  "░██   ░███ ░██   ░███  ░██  ░██  ",
  " ░█████░██  ░█████░██ ░██    ░██ ",
].join("\n")

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
