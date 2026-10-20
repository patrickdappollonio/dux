import type { Ref } from "react"

import type { StateWord } from "@/lib/flatList"
import { cn } from "@/lib/utils"

// An agent or terminal row's state word, shared by every list that names one
// (the sidebar's rows and the New dialog's agent picker), keyed by its caller on
// the label so a change replays the swap. A fade only: motion drops it off the
// line's baseline.
//
// While the row is working the word IS the cue: it pulses on the same clock as
// the glyph and carries the cycling ellipsis in a fixed slot behind it. The
// pulse takes the place of the one-shot swap fade rather than joining it, since
// both are the `animation` shorthand and only one of them can win; the word is
// already at full opacity when the pulse starts, so nothing snaps. A row with a
// bouncing glyph anchors it to that bounce (`useWorkingPulseAnchor`) through
// `ref`, so work resuming mid-settle keeps the word in step with the glyph.
export function RowStateWord({
  word,
  working,
  ref,
}: {
  word: StateWord
  working: boolean
  ref?: Ref<HTMLSpanElement>
}) {
  return (
    <span
      ref={ref}
      className={cn(
        "shrink-0 font-medium",
        working
          ? "motion-safe:animate-working-pulse"
          : "motion-safe:animate-state-word",
        word.className,
      )}
    >
      {word.label}
      {working ? <span aria-hidden className="working-dots" /> : null}
    </span>
  )
}
