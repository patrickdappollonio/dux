// Shared GitHub PR state -> presentation mapping. This is the ONE intentional
// semantic-color exception in the web UI: PR states carry real-world meaning
// that maps directly to green/purple/red, matching the dux TUI banner colors
// (open=green, merged=purple, closed=red). Both the sidebar icon and the PR
// banner lane read from here so the two surfaces can never drift.

import type { PrView } from "@/lib/types"

// A PR state plus a defensive "unknown" bucket for any value the server might
// add later — callers never crash on an unrecognized state, they fall back to a
// neutral presentation.
type PrStateLike = PrView["state"] | (string & {})

// The human word for a state, mirroring the TUI's lowercase wording
// ("open"/"merged"/"closed"). Anything unrecognized reads as "unknown".
export function prStateLabel(state: PrStateLike): string {
  if (state === "open") return "open"
  if (state === "merged") return "merged"
  if (state === "closed") return "closed"
  return "unknown"
}

// Icon-only tint for the sidebar/mobile session row: just the glyph color, no
// background (the row carries its own hover/active surface).
export function prIconClass(state: PrStateLike): string {
  if (state === "open") return "text-green-500"
  if (state === "merged") return "text-purple-400"
  if (state === "closed") return "text-red-400"
  return "text-muted-foreground"
}

// Explicit, state-consistent hover for the icon link. Without this the row's
// hover surface (a light, near-white tint) washed the icon out — green-on-near-
// white was nearly invisible. A brighter glyph plus a subtle same-hue chip keeps
// the icon readable on both the sidebar background and the row hover surface.
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
