import { Bot, ChevronUp, Minimize2 } from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"

import { AttentionDot } from "@/components/AttentionDot"
import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  armTheaterToggleFocus,
  useTheaterPillFocus,
} from "@/hooks/use-theater"
import { tabLabels } from "@/lib/agentTabs"
import { exitTheater, selectTab, type SelectedTarget } from "@/lib/store"
import { registerTheaterTabs, theaterPillModel } from "@/lib/theater"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// THE FLOATING PILL: the only chrome theater mode leaves on screen.
//
// It carries exactly three things, and nothing else earns a permanent floating
// control: what the tabs theater HID are doing, the macros trigger, and the way
// out. The status half expands into a mini strip of the same tab pills, so
// switching tab does not cost leaving the mode; it is absent entirely for a
// terminal pane and for a single-tab agent, because an expander that opens onto
// nothing is the never-renders-empty rule.
//
// Bottom right, because that is the corner a thumb reaches on a held tablet and
// the corner an agent CLI is least likely to be drawing something that must be
// read. It is rendered INSIDE the terminal surface's own positioned box, never
// beside it: the pane column holds the compose row and the terminal keys under
// the terminal, and a pill anchored to that column lands on top of the Send
// button. Structural placement rather than an offset measured off the input
// rows, so a bar appearing or disappearing cannot move it onto a tap target.
export function TheaterPill({
  target,
  session,
}: {
  target: SelectedTarget
  /// The focused pane's owning session, when it has one. A terminal pane passes
  /// `undefined` and gets the collapsed pill.
  session: SessionView | undefined
}) {
  const [expanded, setExpanded] = useState(false)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const exitRef = useRef<HTMLButtonElement | null>(null)
  useTheaterPillFocus(exitRef)
  const collapse = useCallback(() => setExpanded(false), [])
  // The page-wide Escape rule cannot see this component's state, so the strip
  // publishes itself: Escape collapses the strip before it leaves the mode.
  useEffect(
    () => registerTheaterTabs({ expanded: () => expanded, collapse }),
    [expanded, collapse],
  )
  // A tap anywhere else puts the strip away, the way every other transient
  // surface behaves. Pointerdown rather than click, so a press that starts on
  // the terminal dismisses before the gesture reaches xterm.
  useEffect(() => {
    if (!expanded) return
    const onDown = (ev: PointerEvent) => {
      const box = boxRef.current
      if (box && ev.target instanceof Node && box.contains(ev.target)) return
      setExpanded(false)
    }
    document.addEventListener("pointerdown", onDown, true)
    return () => document.removeEventListener("pointerdown", onDown, true)
  }, [expanded])
  const activeTabId = target.kind === "agent" ? target.tabId : null
  const model = theaterPillModel(
    target.kind === "agent" ? session?.tabs : undefined,
    activeTabId,
  )
  const labels = tabLabels(model.tabs)
  const sessionId = session?.id

  return (
    <div
      ref={boxRef}
      data-testid="theater-pill"
      className={cn(
        "absolute right-3.5 bottom-3.5 z-30 flex items-center gap-0.5 rounded-full border p-1",
        "bg-card/90 shadow-lg backdrop-blur-md",
      )}
    >
      {model.expandable && expanded && sessionId ? (
        <div role="tablist" className="flex items-center gap-1 pr-1">
          {model.tabs.map((tab, i) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={tab.id === activeTabId}
              // The switch CARRIES the mode. Reading the destination tab's own
              // memory here would drop out of theater the moment the user
              // reached for a sibling that had never been in it, which is the
              // opposite of what pressing a tab inside the mode asks for.
              onClick={() => selectTab(sessionId, tab.id, { theater: true })}
              className={cn(
                // 34px tall inside a 48px-tall pill: the vertical axis is
                // relaxed because the only neighbours above and below are the
                // pill's own padding and the pane behind it, and the button
                // keeps a comfortable width. Its horizontal neighbours are
                // other tab pills, so the gap between them is what an
                // imprecise tap has to clear.
                "flex h-[34px] shrink-0 items-center gap-1.5 rounded-full border px-3 text-sm transition-colors",
                tab.id === activeTabId
                  ? "border-border bg-background text-foreground"
                  : "border-transparent bg-muted text-muted-foreground hover:text-foreground",
              )}
            >
              <Bot
                className={cn(
                  "size-3.5 shrink-0 motion-safe:transition-transform motion-safe:duration-300",
                  tab.working && "motion-safe:animate-agent-working",
                )}
              />
              {tab.needs_attention ? <AttentionDot withTooltip={false} /> : null}
              <span className="max-w-32 truncate">{labels[i]}</span>
            </button>
          ))}
        </div>
      ) : null}

      {model.expandable ? (
        <SimpleTooltip content={expanded ? "Hide the other tabs" : "Show the other tabs"}>
          <Button
            variant="ghost"
            size="icon"
            className={cn(
              // One fixed height whatever it carries, so it never disagrees
              // with the controls beside it; it grows into a small pill for
              // its cues and is a bare 40px circle without them.
              "h-10 shrink-0 gap-1.5 rounded-full",
              model.working || model.attention
                ? "w-auto min-w-10 px-2.5"
                : "w-10",
            )}
            aria-expanded={expanded}
            aria-label={hiddenTabsLabel(model.working, model.attention)}
            onClick={() => setExpanded((open) => !open)}
          >
            {/* The two cues the tab strip uses, on the one control that speaks
                for the strip while it is not on screen. The dot is the shared
                marker; the bob is the same working animation the pills and the
                sidebar rows run.

                IN THE ROW, exactly as a tab pill carries them, never parked in
                the corners of the button's box. This is a GHOST control: it
                paints no surface, so a mark in the corner of its square has no
                disc to belong to. It sits in the pill's dead space, nearer
                whatever is beside it than the control it speaks for, and reads
                as the neighbour's; expanded, that neighbour is a tab pill, and
                the mark becomes a lie about which tab is working. Between the
                chevron and the button's own edge it cannot be anybody else's,
                and it needs no chip, no offset and no clipping to say so. */}
            {model.working ? (
              <Bot className="size-3.5 shrink-0 text-muted-foreground motion-safe:animate-agent-working" />
            ) : null}
            {model.attention ? <AttentionDot withTooltip={false} /> : null}
            <ChevronUp
              className={cn(
                "shrink-0 motion-safe:transition-transform motion-safe:duration-300",
                expanded && "rotate-180",
              )}
            />
          </Button>
        </SimpleTooltip>
      ) : null}

      <MacroPopover variant="pill" target={target} />

      <span aria-hidden className="mx-0.5 h-5.5 w-px shrink-0 bg-border" />

      <SimpleTooltip content="Leave theater mode">
        <Button
          ref={exitRef}
          variant="ghost"
          size="icon"
          className="size-10 shrink-0 rounded-full text-foreground"
          aria-label="Leave theater mode"
          onClick={() => {
            // This button is about to be unmounted, so hand focus on to the
            // header control that replaces it rather than to the body.
            armTheaterToggleFocus()
            exitTheater()
          }}
        >
          <Minimize2 />
        </Button>
      </SimpleTooltip>
    </div>
  )
}

// What the status button announces. It speaks for the HIDDEN tabs only, so a
// screen reader hears about the ones off screen and never about the one filling
// it; the visible tab says that for itself.
function hiddenTabsLabel(working: boolean, attention: boolean): string {
  const parts: string[] = []
  if (working) parts.push("one is working")
  if (attention) parts.push("one needs attention")
  return parts.length === 0
    ? "Other tabs"
    : `Other tabs: ${parts.join(", ")}`
}
