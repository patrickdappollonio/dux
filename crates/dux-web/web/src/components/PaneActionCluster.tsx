import {
  Bot,
  Diff,
  Keyboard,
  KeyboardOff,
  Maximize2,
  Minimize2,
  SquareTerminal,
} from "lucide-react"
import type * as React from "react"
import { useLayoutEffect, useRef, type ReactNode } from "react"

import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import { Button } from "@/components/ui/button"
import { changesSummary } from "@/lib/changesSummary"
import { phoneSwitch, type PartCue, type PhoneView } from "@/lib/layoutModel"
import { useIsMobile } from "@/hooks/use-mobile"
import { useAgentRowWatch } from "@/lib/paneCover"
import { keysToggleLabel, usePaneInputGroup } from "@/lib/paneInputGroup"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { armTheaterToggleFocus } from "@/hooks/use-theater"
import {
  mobileAccessoryBarVisible,
  openChangedFiles,
  setAccessoryBarVisibility,
  switchPhonePart,
  toggleTheater,
  useDux,
} from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"

import { swapTheaterGlyphs } from "./theaterFlightParts"

// The pane's one action cluster, wherever it is painted: the theater toggle,
// Macros, the changed-file count where an agent has one, the terminal-keys
// toggle where the pane has a key row on offer, on a phone the switch between
// an agent and its terminals, and the surface `⋯`.
// The phone's docked flap renders it, and so does the floating theater pill on
// both form factors, because the detach animation flies one into the other as a
// single object.
//
// Which controls render is a parameter, never a form factor, and the `⋯` is
// handed in because only the surface knows which pane it is over. How they look
// is not a parameter: every control wears the same treatment at every dock.
//
// The geometry is load-bearing. Every control is 40px tall (the touch-target
// floor), the row's gap is 2px, and the count is the only control wider than it
// is tall, because a count is data and must stay legible on a phone.

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
  /// The surface's own `⋯`, the same menu at both docks so the flight is
  /// honest. A prop rather than something this component builds, because only
  /// the surface knows which pane it is over and what that pane is about.
  ellipsis: ReactNode
}) {
  return (
    <>
      <TheaterMorphButton buttonRef={theaterRef} />
      <MacroPopover variant="pill" target={target} />
      <ChangesCountButton sessionId={sessionId} />
      <KeysToggleButton target={target} />
      <PartSwitchButton target={target} sessionId={sessionId} />
      {ellipsis}
    </>
  )
}

// The toggle morphs rather than swaps: both icons are stacked in one grid cell
// and `aria-pressed` picks the settled one, and the mode flipping rotates one
// out while the other rotates in, right under the finger (see
// `swapTheaterGlyphs`). A conditional pair of icons has nothing to animate
// between, and the control reads as two buttons rather than one changing state.
//
// It is also the way out wherever the cluster is floating: the pill exists only
// while theater is on, so this toggle is always painted there, labelled and
// announced as "Leave theater mode".
function TheaterMorphButton({
  buttonRef,
}: {
  buttonRef?: React.RefObject<HTMLButtonElement | null>
}) {
  const { theater } = useDux()
  const label = theater ? "Leave theater mode" : "Theater mode"
  const glyphsRef = useGlyphSwap(theater)
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
        <span
          ref={glyphsRef}
          aria-hidden
          className="dux-icswap grid place-items-center"
        >
          <Maximize2 className="dux-ic-max" />
          <Minimize2 className="dux-ic-min" />
        </span>
      </Button>
    </SimpleTooltip>
  )
}

/// Swaps the glyphs when the mode flips under a mounted toggle, in the commit
/// that flips `aria-pressed`, so the first frame of the new state is the
/// swap's first frame. A toggle that mounts into a mode shows it settled.
function useGlyphSwap(theater: boolean) {
  const ref = useRef<HTMLSpanElement | null>(null)
  const reducedMotion = usePrefersReducedMotion()
  const shown = useRef(theater)
  const swaps = useRef<Animation[]>([])
  useLayoutEffect(() => {
    const flipped = shown.current !== theater
    shown.current = theater
    // Asking for less motion stops a swap already playing, whatever the mode
    // is doing: the glyph the stylesheet picks is the settled one.
    if (reducedMotion) {
      swaps.current.forEach((a) => a.cancel())
      swaps.current = []
      return
    }
    const root = ref.current
    if (!flipped || !root) return
    swaps.current = swapTheaterGlyphs(root, swaps.current, theater)
  }, [theater, reducedMotion])
  return ref
}

// The changed-file count, its own control rather than a badge on the `⋯`. The
// diff glyph already draws the `±`, so the text beside it is the bare count.
// Content-sized on the shared 40px height, the one control wider than it is
// tall, because the number is data.
//
// It acts rather than reports: one call opens the changed files, which is the
// changes screen on a phone and, on a computer, leaving theater for the layout
// with the Changes pane in it. Either way it pushes, so Back returns.
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
        onClick={() => openChangedFiles()}
      >
        <Diff />
        <span className="text-sm">{summary.count}</span>
      </Button>
    </SimpleTooltip>
  )
}

// The terminal-keys show/hide toggle, on a phone only: a computer carries the
// same toggle as a row of the top menu's Input group, so each width has it in
// exactly one place. It stays on offer while the keys are hidden, because
// hiding them must never take away the way to show them again. Present exactly
// while the pane under this cluster publishes a key row for this device, which
// it does only for the input's owner.
function KeysToggleButton({ target }: { target: SelectedTarget }) {
  const isMobile = useIsMobile()
  const duxState = useDux()
  const group = usePaneInputGroup([
    target.kind === "agent" ? target.tabId : target.terminalId,
  ])
  if (!isMobile || !group?.keysToggle) return null
  const visible = mobileAccessoryBarVisible(duxState)
  const label = keysToggleLabel(visible)
  return (
    <SimpleTooltip content={label}>
      <Button
        variant="ghost"
        size="icon"
        data-testid="pane-keys-toggle"
        className={CLUSTER_BUTTON}
        aria-label={label}
        onClick={() => void setAccessoryBarVisibility(!visible)}
      >
        {visible ? <KeyboardOff /> : <Keyboard />}
      </Button>
    </SimpleTooltip>
  )
}

// The look the switch's icon takes for the part it points to: the busy colour
// while it works (the pulse itself is the glyph's), and the shared attention
// colour and blink when it needs you. The icon carries it with no dot beside
// it, and it never bounces: one bouncing icon among still ones would pull the
// eye all the time.
const PART_CUE_LOOK: Record<PartCue, string | undefined> = {
  attention: "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none",
  working: "text-dux-working",
  plain: undefined,
}

const PART_CUE_WORDS: Record<PartCue, string> = {
  attention: ", needs you",
  working: ", working",
  plain: "",
}

// A phone shows an agent or its terminals, never both, and this is how it moves
// between them: the other part's icon, with the terminal count over the agent.
// A computer lays the two out together and has no switch. Present only on an
// agent with a terminal to switch to, and only on a row this device drives:
// under the take-over card, nothing may say the agent has terminals.
function PartSwitchButton({
  target,
  sessionId,
}: {
  target: SelectedTarget
  sessionId: string | undefined
}) {
  const isMobile = useIsMobile()
  const { spine } = useDux()
  const watching = useAgentRowWatch(sessionId).watched
  const session = spine?.sessions.find((s) => s.id === sessionId)
  if (!isMobile || watching || session === undefined) return null
  const terminals = (spine?.terminals ?? []).filter(
    (t) => t.owner.kind === "session" && t.owner.session_id === session.id,
  )
  const view: PhoneView = target.kind === "terminal" ? "terminals" : "agent"
  const shown = phoneSwitch(view, session, terminals)
  if (shown === null) return null
  const toTerminals = shown.shows === "terminals"
  const label = `${toTerminals ? `Show terminals (${shown.count})` : "Show the agent"}${PART_CUE_WORDS[shown.cue]}`
  const Icon = toTerminals ? SquareTerminal : Bot
  return (
    <SimpleTooltip content={label}>
      <Button
        variant="ghost"
        size="icon"
        data-testid="pane-part-switch"
        data-cue={shown.cue}
        // Content-sized like the changes count over the agent, because the
        // count beside the icon is data.
        className={
          toTerminals ? "h-10 w-auto shrink-0 gap-1.5 rounded-full px-3" : CLUSTER_BUTTON
        }
        aria-label={label}
        onClick={() => switchPhonePart(session.id)}
      >
        {/* The working glyph without its bounce: a hand-over to needs-you
          * lets the pulse land before the attention look eases in, as on the
          * list, and the flap and the pill it flies into share one clock, so
          * they pulse as one. */}
        <WorkingGlyph
          icon={Icon}
          working={shown.cue === "working"}
          look={PART_CUE_LOOK[shown.cue]}
          bounce={false}
          cueKey={`part-switch:${session.id}`}
        />
        {toTerminals ? <span className="text-sm">{shown.count}</span> : null}
      </Button>
    </SimpleTooltip>
  )
}
