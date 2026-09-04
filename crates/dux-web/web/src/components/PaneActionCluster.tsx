import { Diff, Maximize2, Minimize2 } from "lucide-react"
import type * as React from "react"
import type { ReactNode } from "react"

import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { changesSummary } from "@/lib/changesSummary"
import { armTheaterToggleFocus } from "@/hooks/use-theater"
import { openChangesScreen, toggleTheater, useDux } from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"

// THE PANE'S ONE ACTION CLUSTER, in every place it is ever painted.
//
// It is the same controls at every dock: the theater toggle, Macros, the
// changed-file count where there is an agent to have one, and the surface `⋯`.
// The phone's docked flap renders it, and so does the floating theater pill on
// BOTH form factors, because the two are not merely similar: the phone's detach
// animation flies one into the other as a single object, and a computer whose
// pill looked like something else was a second design for one control.
//
// WHICH CONTROLS RENDER IS A PARAMETER, never a form factor. A pane with no
// agent behind it carries no count, on a phone and on a computer alike; the
// `⋯` is handed in because only the surface knows which pane it is over. What
// is NOT a parameter is how any of them look: every control that appears here
// wears the same treatment wherever the cluster is painted.
//
// The geometry is therefore load-bearing, not decorative. Every control is
// 40px tall (the touch-target floor), the row's gap is 2px, and the count is
// the only control that is wider than it is tall, because a count is DATA and
// stays legible text on a phone.

/// The shared button treatment: a bare 40px circle on whichever rounded surface
/// the cluster is sitting on. Not `outline`, deliberately: the flap and the pill
/// are each ONE surface, and a bordered button inside either reads as two.
const CLUSTER_BUTTON = "size-10 shrink-0 rounded-full"

export function PaneActionCluster({
  target,
  sessionId,
  theaterRef,
  ellipsis,
}: {
  target: SelectedTarget
  /// The agent whose changed files the count is about, or `undefined` for a
  /// pane with no agent behind it, which carries no count at all. Same rule at
  /// every dock: a terminal has no changed-file summary to show.
  sessionId: string | undefined
  /// The theater toggle's node, so the surface around it can hand focus back
  /// when the OTHER surface's press brought it on screen.
  theaterRef?: React.RefObject<HTMLButtonElement | null>
  /// The surface's own `⋯`. It is the SAME menu at both docks, which is what
  /// makes the flight honest: a button that changed what it opens on arrival
  /// would be the one thing the animation says cannot happen. It is a prop
  /// rather than something this component builds because only the surface knows
  /// which pane it is over and what that pane is about.
  ellipsis: ReactNode
}) {
  return (
    <>
      <TheaterMorphButton buttonRef={theaterRef} />
      <MacroPopover variant="pill" target={target} />
      <ChangesCountButton sessionId={sessionId} />
      {ellipsis}
    </>
  )
}

// THE TOGGLE THAT MORPHS RATHER THAN SWAPS.
//
// It stacks both icons in one grid cell and lets `aria-pressed` pick the
// settled one, because the detach and re-dock flights ROTATE one out while the
// other rotates in (see the `dux-flight-*` rules in index.css). A conditional
// `{on ? <Minimize2/> : <Maximize2/>}` has nothing to animate between: the old
// node is gone in the same frame the new one appears, and the control reads as
// two different buttons rather than one changing state.
//
// IT IS ALSO THE WAY OUT, wherever the cluster is floating: the pill only
// exists while theater is on, so this toggle is always painted, labelled and
// announced as "Leave theater mode" there. The computer's pill used to carry a
// separate exit button beside it, which made the mode's one control two
// different-looking things depending on which machine you were sitting at.
function TheaterMorphButton({
  buttonRef,
}: {
  buttonRef?: React.RefObject<HTMLButtonElement | null>
}) {
  const { theater } = useDux()
  const label = theater ? "Leave theater mode" : "Theater mode"
  return (
    <SimpleTooltip content={label}>
      <Button
        ref={buttonRef}
        variant="ghost"
        size="icon"
        data-testid="pane-theater-toggle"
        className={cn(CLUSTER_BUTTON, theater && "text-foreground")}
        aria-label={label}
        aria-pressed={theater}
        onClick={() => {
          // The press is about to destroy whichever surface it was made on, so
          // the surface that replaces it takes focus rather than the body.
          armTheaterToggleFocus()
          toggleTheater()
        }}
      >
        <span aria-hidden className="dux-icswap grid place-items-center">
          <Maximize2 className="dux-ic-max" />
          <Minimize2 className="dux-ic-min" />
        </span>
      </Button>
    </SimpleTooltip>
  )
}

// THE CHANGED-FILE COUNT, as its own control rather than a badge on the `⋯`.
//
// The diff glyph already draws the `±`, so the text beside it is the BARE
// count: printing "±3" next to a plus-minus icon says the same thing twice.
// Content-sized on the shared 40px height, the one control in the cluster that
// is wider than it is tall, because the number is data.
//
// It ACTS rather than reports: it opens the changed files. On a phone that is
// the changes screen; on a computer the same call gives the chrome back, which
// is where the Changes pane theater unmounted lives, and pushes a history entry
// so Back returns. One call, because "show me the changed files" is one intent
// and a second implementation is a second thing to keep in step.
function ChangesCountButton({ sessionId }: { sessionId: string | undefined }) {
  const { changes } = useDux()
  const summary = changesSummary(changes, sessionId)
  if (!summary) return null
  return (
    <SimpleTooltip content="Changed files">
      <Button
        variant="ghost"
        size="icon"
        data-testid="pane-changes-count"
        className="h-10 w-auto shrink-0 gap-1.5 rounded-full px-3"
        aria-label={summary.countLabel}
        onClick={() => openChangesScreen()}
      >
        <Diff />
        <span className="text-sm">{summary.count}</span>
      </Button>
    </SimpleTooltip>
  )
}
