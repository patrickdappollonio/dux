// Shared GitHub PR state to presentation mapping, the one intentional semantic-color exception
// in the web UI: open is green, merged purple, closed red, matching the TUI banner. The sidebar
// icon and the PR banner lane both read from here so the two surfaces cannot drift.

import type { PrView } from "@/lib/types"

// A PR state plus a defensive "unknown" bucket for a value the server may add later, so a
// caller falls back to a neutral presentation rather than crashing.
type PrStateLike = PrView["state"] | (string & {})

// The human word for a state, mirroring the TUI's lowercase wording
// ("open"/"merged"/"closed"). Anything unrecognized reads as "unknown".
export function prStateLabel(state: PrStateLike): string {
  if (state === "open") return "open"
  if (state === "merged") return "merged"
  if (state === "closed") return "closed"
  return "unknown"
}

// The sentence a screen reader speaks. "Pull request" is spelled out, since the abbreviation
// is read as letters, and an unrecognized state is left off rather than spoken as "unknown".
export function prAriaLabel(number: number, state: PrStateLike): string {
  const label = prStateLabel(state)
  return label === "unknown"
    ? `Pull request #${number}`
    : `Pull request #${number}, ${label}`
}

// Icon-only tint for the sidebar/mobile session row: just the glyph color, no
// background (the row carries its own hover/active surface).
export function prIconClass(state: PrStateLike): string {
  if (state === "open") return "text-green-500"
  if (state === "merged") return "text-purple-400"
  if (state === "closed") return "text-red-400"
  return "text-muted-foreground"
}

// Explicit hover for the icon link: the row's near-white hover surface washes a plain glyph
// out, so a brighter glyph plus a subtle same-hue chip keeps it readable on both surfaces.
export function prIconHoverClass(state: PrStateLike): string {
  if (state === "open") return "hover:bg-green-600/15 hover:text-green-400"
  if (state === "merged") return "hover:bg-purple-600/15 hover:text-purple-300"
  if (state === "closed") return "hover:bg-red-600/15 hover:text-red-300"
  return "hover:bg-muted hover:text-foreground"
}

// Soft state-colored strip for the PR banner lane: tinted background + matching
// border + readable text, mirroring the TUI banner's green/purple/red fill.
export function prBannerClass(state: PrStateLike): string {
  if (state === "open")
    return "bg-green-600/10 border-green-600/30 text-green-500 hover:bg-green-600/15"
  if (state === "merged")
    return "bg-purple-600/10 border-purple-600/30 text-purple-400 hover:bg-purple-600/15"
  if (state === "closed")
    return "bg-red-600/10 border-red-600/30 text-red-400 hover:bg-red-600/15"
  return "bg-muted border-border text-muted-foreground hover:bg-muted/80"
}
