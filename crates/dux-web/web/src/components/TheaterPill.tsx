import { Bot, ChevronUp, Ellipsis, GripVertical, Minimize2 } from "lucide-react"
import type * as React from "react"
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"

import { AppMenuBody } from "@/components/AppMenu"
import { AttentionDot } from "@/components/AttentionDot"
import { InputMenuItems } from "@/components/InputMenuItems"
import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useAttachCapability } from "@/lib/attachRegistry"
import { usePaneInputMenu } from "@/lib/paneInputMenu"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import {
  armTheaterToggleFocus,
  useTheaterPillFocus,
} from "@/hooks/use-theater"
import { tabLabels } from "@/lib/agentTabs"
import { notifyInfo } from "@/lib/notify"
import { exitTheater, selectTab, type SelectedTarget } from "@/lib/store"
import { registerTheaterTabs, theaterPillModel } from "@/lib/theater"
import {
  classifyPillGesture,
  clampPillPosition,
  markPillHintShown,
  nudgePillPosition,
  readPillHintPending,
  readPillPosition,
  resolvePillPosition,
  THEATER_PILL_HOLD_MS,
  writePillPosition,
  type PillPosition,
  type PillSize,
} from "@/lib/theaterPill"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// THE FLOATING PILL: the only chrome theater mode leaves on screen.
//
// It carries exactly four things, and nothing else earns a permanent floating
// control: what the tabs theater HID are doing, the macros trigger, the app
// menu, and the way out. The status half expands into a mini strip of the same tab pills, so
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
  const coarse = useIsCoarsePointer()
  const reducedMotion = usePrefersReducedMotion()
  const drag = usePillDrag(boxRef, collapse)
  usePillHint()
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
  // The PTY behind the pane this pill is painted over, named the same way the
  // shells key the pane itself, so the pill reads the input menu of the pane it
  // is actually on rather than whichever one registered last.
  const paneId = target.kind === "agent" ? target.tabId : target.terminalId

  return (
    <div
      ref={boxRef}
      data-testid="theater-pill"
      className={cn(
        "absolute z-30 flex items-center gap-0.5 rounded-full border p-1",
        "bg-card/90 shadow-lg backdrop-blur-md",
        // The corner it starts in, until a measurement gives it real
        // coordinates. Keeping the CSS default for that frame is what stops the
        // pill flashing at the origin on a pane that has not been laid out yet.
        drag.position === null && "right-3.5 bottom-3.5",
        // The settle after a nudge or a re-clamp. It is deliberately absent
        // while a drag is live (the pointer already moves the pill, and easing
        // toward the finger would lag it) and for a viewer who asked for less
        // motion, who still gets every clamp, just instantly.
        !reducedMotion &&
          !drag.dragging &&
          !drag.justDropped &&
          "transition-[left,top] duration-150 ease-out",
      )}
      style={
        drag.position === null
          ? undefined
          : { left: drag.position.x, top: drag.position.y }
      }
    >
      <SimpleTooltip content={coarse ? "" : "Hold to drag"}>
        <Button
          variant="ghost"
          size="icon"
          data-testid="theater-pill-grip"
          // The pill floats over the newest lines of output, so the answer to
          // it covering something is to move it. The grip is where that gesture
          // lives, and it is a real 40px control rather than the pill's whole
          // body precisely because the body is buttons: a hold that started
          // anywhere would make every tap ambiguous.
          aria-label="Drag handle: hold to move the pill"
          // `touch-none` is load-bearing twice over: it stops the browser
          // scrolling or long-pressing the page out from under the drag, and it
          // is what keeps the terminal's own long-press selection from starting
          // under the finger. The handlers stop the events reaching the pane too.
          className="size-10 shrink-0 cursor-grab touch-none rounded-full text-muted-foreground select-none active:cursor-grabbing"
          onPointerDown={drag.onPointerDown}
          onPointerMove={drag.onPointerMove}
          onPointerUp={drag.onPointerUp}
          onPointerCancel={drag.onPointerCancel}
          onLostPointerCapture={drag.onLostPointerCapture}
          // A keyboard cannot hold and pull, so the arrow keys move the pill a
          // step at a time. Without it a keyboard user has no way at all to
          // clear a pill that sits over what they are reading.
          onKeyDown={drag.onKeyDown}
        >
          <GripVertical />
        </Button>
      </SimpleTooltip>

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

      <TheaterAppMenu paneId={paneId} />

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

// THE APP MENU, WHILE THE MODE HAS TAKEN EVERY OTHER ANCHOR AWAY.
//
// On a computer theater unmounts the sidebar (and with it the launcher corner's
// `⋯`) and the whole header stack (and with it the cog); on a phone it takes
// the top bar. Without this the mode is the one state in which Preferences, New
// agent and every other global action are unreachable, which is exactly what
// the "exactly one surface-scoped `⋯` is on screen whatever the surrounding
// state" rule forbids. So the pill grows the app menu for the duration, and it
// stays in the collapsed form too: a single-tab agent or a terminal pane folds
// the tab strip away, never the way to the app's own actions.
//
// It renders `AppMenuBody`, the same body the header's cog renders, so the two
// cannot offer different things; the wrapper adds only the theater exit, from
// the shared item the input `⋯` uses, so the way out is inside the one `⋯` too.
//
// It also carries the pane's INPUT items whenever the pane has nowhere of its
// own to put them, which on a computer is the ordinary case: theater takes the
// whole window, and a bordered `⋯` row under the terminal would be both a
// second ellipsis beside this one and exactly the chrome the mode removes. The
// pane publishes them for precisely that state (see `lib/paneInputMenu.ts`), so
// the typing-surface switch and "Attach a file…" stay one press away and there
// is still one trigger on screen. A phone keeps its own row and publishes
// nothing, and then this menu is the app menu plus the theater exit as before.
//
// NAMED "Settings", like the control it stands in for: a user looking for the
// cog's menu should find it under the name they know, and the pill's own
// buttons already say what each of them does.
function TheaterAppMenu({ paneId }: { paneId: string }) {
  const paneMenu = usePaneInputMenu(paneId)
  // The attach item is the mounted OWNER pane's own capability, borrowed the
  // way the row menus borrow it, so the file travels through that pane's
  // already-gated socket and lands in its own sink. Both halves have to be
  // there: the pane says the item belongs, the registry hands over the act.
  const attachToPane = useAttachCapability([paneId])
  return (
    <DropdownMenu>
      <SimpleTooltip content="Settings">
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              size="icon"
              aria-label="Settings"
              className="size-10 shrink-0 rounded-full"
            />
          }
        >
          <Ellipsis />
        </DropdownMenuTrigger>
      </SimpleTooltip>
      {/* Anchored ABOVE the trigger, as the input `⋯` is: the pill lives in the
          bottom corner of the pane, where a downward popup has nowhere to go.
          On a phone the primitive renders it as a sheet and ignores this. */}
      <DropdownMenuContent side="top" align="end">
        <AppMenuBody />
        <DropdownMenuSeparator />
        <InputMenuItems
          gates={{
            attach: (paneMenu?.gates.attach ?? false) && attachToPane !== null,
            surfaceSwitch: paneMenu?.gates.surfaceSwitch ?? false,
            keysToggle: paneMenu?.gates.keysToggle ?? false,
            // Never here: the top bar is one of the things theater took away,
            // and this menu only exists while the mode is on.
            topBarToggle: false,
            // The guaranteed way out, whatever the pane published.
            theaterExit: true,
          }}
          composeSurface={paneMenu?.composeSurface ?? false}
          onAttach={() => attachToPane?.()}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

// THE ONE-TIME "you can move this" HINT.
//
// Nothing about a floating pill says it can be dragged, and the whole point of
// the drag is the user who is being covered by it right now. So the first time
// theater is entered on a device the pill says so, once, through the one raiser,
// on the ordinary display window. It is INFO and not sticky: nothing is lost if
// it goes unread, the grip is still there, and a pinned toast over a mode whose
// entire purpose is screen space would be its own joke.
//
// The latch is `localStorage` rather than the page-session flags the terminal's
// two modifier hints use, because entering theater is rarer than dragging in a
// mouse-reporting app: a page-lifetime latch would re-teach the same person on
// every reload.
function usePillHint(): void {
  useEffect(() => {
    if (!readPillHintPending()) return
    // Marked BEFORE the raise, so a double-invoked effect (React's development
    // strict mode) cannot produce two toasts.
    markPillHintShown()
    notifyInfo("Hold the pill's grip to drag it anywhere in the terminal.")
  }, [])
}

interface PillDrag {
  /// Where the pill sits, or `null` before anything has been measured.
  position: PillPosition | null
  /// Whether a drag is live, which suppresses the settle animation.
  dragging: boolean
  /// True for the single commit that lands a drop, which suppresses the settle
  /// animation for the same reason a live drag does: the pill is already where
  /// the pointer left it, and easing it there would come from the wrong place.
  justDropped: boolean
  onPointerDown: (ev: React.PointerEvent<HTMLElement>) => void
  onPointerMove: (ev: React.PointerEvent<HTMLElement>) => void
  onPointerUp: (ev: React.PointerEvent<HTMLElement>) => void
  onPointerCancel: (ev: React.PointerEvent<HTMLElement>) => void
  onLostPointerCapture: (ev: React.PointerEvent<HTMLElement>) => void
  onKeyDown: (ev: React.KeyboardEvent<HTMLElement>) => void
}

interface DragGesture {
  pointerId: number
  pointerType: string
  startX: number
  startY: number
  startedAt: number
  base: PillPosition
  last: PillPosition
  lifted: boolean
  timer: ReturnType<typeof setTimeout> | null
  frame: number | null
}

/**
 * MOVING THE PILL, and everything that has to be true while it moves.
 *
 * The position is live state rather than a CSS corner because four different
 * things set it: a restore from this device's memory, a drag, an arrow key, and
 * a re-clamp when the surface changes shape under it. All four end in the same
 * clamp, so the pill is always somewhere every one of its buttons can be reached.
 *
 * WHILE A DRAG IS LIVE the pill is moved by a transform on the element, not by
 * React state: a re-render per pointer move would be a re-render per frame over
 * a terminal that is already painting. The state is written once, on release,
 * along with the memory. Pointer capture is what makes the gesture survive the
 * pointer leaving the 40px grip, and it is also what keeps the terminal
 * underneath from ever seeing the move; the handlers stop propagation as well,
 * so nothing in the pane's own tree can start a selection from this gesture.
 */
function usePillDrag(
  boxRef: React.RefObject<HTMLDivElement | null>,
  collapseTabs: () => void,
): PillDrag {
  const [position, setPosition] = useState<PillPosition | null>(null)
  const [dragging, setDragging] = useState(false)
  const [justDropped, setJustDropped] = useState(false)
  const posRef = useRef<PillPosition | null>(null)
  // WHERE THE USER ASKED FOR IT, which is a different question from where it
  // fits today. Every clamp is against the surface and the pill OF THE MOMENT,
  // and both change under it: folding the tab strip out widens the pill and
  // shoves it left. Clamping the already-clamped value makes that shove
  // permanent, because the place to come back to has been overwritten by the
  // place it was pushed to. So the intent is what is kept, what is stored, and
  // what every re-clamp is re-derived from. `null` means nobody has placed it,
  // and the default corner is re-derived for whatever surface is there today.
  const intentRef = useRef<PillPosition | null>(null)
  // The device's memory is read once, at the first measurement; after that the
  // intent above is the live answer and storage is write-only.
  const restoredRef = useRef(false)
  const sizesRef = useRef<{ surface: PillSize; pill: PillSize }>({
    surface: { width: 0, height: 0 },
    pill: { width: 0, height: 0 },
  })
  const gestureRef = useRef<DragGesture | null>(null)

  const place = useCallback((next: PillPosition, persist: boolean) => {
    posRef.current = next
    intentRef.current = next
    setPosition(next)
    if (persist) writePillPosition(next)
  }, [])

  // Read both boxes and settle on a position for them. The pill's SURFACE is
  // its own offset parent by construction: the pane renders the overlay inside
  // its positioned box, which is exactly the area the pill may roam.
  const measure = useCallback(() => {
    const box = boxRef.current
    const surfaceEl = box?.parentElement
    if (!box || !surfaceEl) return
    const s = surfaceEl.getBoundingClientRect()
    const p = box.getBoundingClientRect()
    const sizes = {
      surface: { width: s.width, height: s.height },
      pill: { width: p.width, height: p.height },
    }
    sizesRef.current = sizes
    if (!restoredRef.current) {
      restoredRef.current = true
      intentRef.current = readPillPosition()
    }
    // Always from the INTENT, never from the last clamp: this is the whole
    // reason the two are kept apart.
    const next = resolvePillPosition(
      intentRef.current,
      sizes.surface,
      sizes.pill,
    )
    if (!next) return
    const current = posRef.current
    if (current && current.x === next.x && current.y === next.y) return
    // Nothing here writes to storage. The user did not move the pill, a window
    // did, and the position they chose has to survive the window changing back.
    posRef.current = next
    setPosition(next)
  }, [boxRef])


  const endGesture = useCallback(
    (commit: boolean) => {
      const g = gestureRef.current
      if (!g) return
      gestureRef.current = null
      if (g.timer !== null) clearTimeout(g.timer)
      if (g.frame !== null) cancelAnimationFrame(g.frame)
      const box = boxRef.current
      if (box) box.style.transform = ""
      setDragging(false)
      // A gesture that never lifted was a tap, and a tap on the grip does
      // nothing at all: the buttons beside it keep their own meanings.
      if (!commit || !g.lifted) return
      // THE DROP IS NOT A SETTLE. One commit both swaps the transform for real
      // coordinates and re-enables the settle animation, so an animated drop
      // eases the pill from the corner it was dragged out of back to the finger
      // that already left it here. Suppressed for this commit only.
      setJustDropped(true)
      place(g.last, true)
    },
    [boxRef, place],
  )

  useLayoutEffect(() => {
    measure()
    if (typeof ResizeObserver === "undefined") return
    const box = boxRef.current
    const surfaceEl = box?.parentElement
    // Both boxes: the surface because a window resize or a rotation changes
    // where the edges are, and the pill because folding the tab strip out
    // changes how much of it has to fit inside them.
    const ro = new ResizeObserver((entries) => {
      // A SURFACE THAT CHANGES SHAPE MID-DRAG ENDS THE DRAG. The transform the
      // pill is moved by is measured from the base it was pressed at, and the
      // re-clamp below moves the coordinates that base is written in: applied
      // together they move the pill twice. Rebasing a live gesture would mean
      // re-projecting the pointer into a surface it was never pressed in, for a
      // gesture whose causes (a rotation, a keyboard coming up) have already
      // interrupted the user, so it ends instead, committing what it had.
      //
      // The PILL's own box is deliberately not that: the tab strip folds away on
      // the lift itself, so every drag starts with the pill changing size.
      if (surfaceEl && entries.some((e) => e.target === surfaceEl)) {
        endGesture(true)
      }
      measure()
    })
    if (surfaceEl) ro.observe(surfaceEl)
    if (box) ro.observe(box)
    return () => ro.disconnect()
  }, [boxRef, measure, endGesture])

  // The suppression lasts exactly one painted frame: long enough for the drop's
  // own coordinates to land without easing, short enough that the next nudge or
  // re-clamp still settles.
  useEffect(() => {
    if (!justDropped) return
    const frame = requestAnimationFrame(() => setJustDropped(false))
    return () => cancelAnimationFrame(frame)
  }, [justDropped])

  const lift = useCallback(() => {
    const g = gestureRef.current
    if (!g || g.lifted) return
    g.lifted = true
    if (g.timer !== null) {
      clearTimeout(g.timer)
      g.timer = null
    }
    // The strip is a transient surface anchored to a pill that is about to
    // move; carrying it along would drag a tablist across the terminal.
    collapseTabs()
    setDragging(true)
  }, [collapseTabs])

  const paint = useCallback(() => {
    const g = gestureRef.current
    if (!g || g.frame !== null) return
    g.frame = requestAnimationFrame(() => {
      const live = gestureRef.current
      if (!live) return
      live.frame = null
      const box = boxRef.current
      if (!box) return
      box.style.transform = `translate3d(${live.last.x - live.base.x}px, ${
        live.last.y - live.base.y
      }px, 0)`
    })
  }, [boxRef])

  const onPointerDown = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      if (!ev.isPrimary) return
      ev.stopPropagation()
      // A pane that has never been measured (the very first frame) has no
      // coordinates to drag from, so measure now rather than starting from a
      // guess.
      measure()
      const base = posRef.current
      if (!base) return
      try {
        ev.currentTarget.setPointerCapture(ev.pointerId)
      } catch {
        // Some browsers refuse capture for a pointer that has already been
        // released. The gesture still works while the pointer is over the grip.
      }
      const gesture: DragGesture = {
        pointerId: ev.pointerId,
        pointerType: ev.pointerType,
        startX: ev.clientX,
        startY: ev.clientY,
        startedAt: Date.now(),
        base,
        last: base,
        lifted: false,
        timer: null,
        frame: null,
      }
      gestureRef.current = gesture
      // A FINGER lifts on time, and it must lift even if it never moves: a hold
      // with no travel is exactly the gesture, and there would otherwise be no
      // event left to notice it.
      if (ev.pointerType === "touch") {
        gesture.timer = setTimeout(() => lift(), THEATER_PILL_HOLD_MS)
      }
    },
    [lift, measure],
  )

  const onPointerMove = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      const g = gestureRef.current
      if (!g || ev.pointerId !== g.pointerId) return
      ev.stopPropagation()
      const dx = ev.clientX - g.startX
      const dy = ev.clientY - g.startY
      if (!g.lifted) {
        const verdict = classifyPillGesture({
          pointerType: g.pointerType,
          heldMs: Date.now() - g.startedAt,
          travel: Math.hypot(dx, dy),
          ended: false,
        })
        if (verdict === "cancel") {
          endGesture(false)
          return
        }
        if (verdict !== "lift") return
        lift()
      }
      const { surface, pill } = sizesRef.current
      g.last = clampPillPosition(
        { x: g.base.x + dx, y: g.base.y + dy },
        surface,
        pill,
      )
      paint()
    },
    [endGesture, lift, paint],
  )

  const onPointerUp = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      const g = gestureRef.current
      if (!g) return
      ev.stopPropagation()
      // The release goes through the same classifier every move does, so tap
      // versus drag is decided in exactly one place. A press that already lifted
      // is a drag whatever the release looks like; one that has not is a tap,
      // and a tap on the grip does nothing at all.
      const verdict = g.lifted
        ? "lift"
        : classifyPillGesture({
            pointerType: g.pointerType,
            heldMs: Date.now() - g.startedAt,
            travel: Math.hypot(ev.clientX - g.startX, ev.clientY - g.startY),
            ended: true,
          })
      endGesture(verdict !== "tap")
    },
    [endGesture],
  )

  const onPointerCancel = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      if (!gestureRef.current) return
      ev.stopPropagation()
      endGesture(false)
    },
    [endGesture],
  )

  // A CAPTURE THAT GOES AWAY ENDS THE GESTURE, exactly as it does for the
  // divider drags: the browser can hand the pointer to something else, and the
  // element can be removed, and neither produces a pointerup. It COMMITS, the
  // opposite of a cancel: the pill is already under the pointer, the user
  // watched it get there, and reverting would undo a deliberate move after the
  // fact. A press that never lifted still commits nothing, because that is a tap.
  // An ordinary release fires this too, after `onPointerUp` has already retired
  // the gesture, so it lands on nothing.
  const onLostPointerCapture = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      const g = gestureRef.current
      if (!g || ev.pointerId !== g.pointerId) return
      endGesture(true)
    },
    [endGesture],
  )

  // A pill that goes away under the finger takes its gesture with it: the hold
  // timer and the paint frame are the pill's, and neither has anything left to
  // do. Nothing is committed, because an unmount is not a drop.
  useEffect(() => () => endGesture(false), [endGesture])

  // The same rule for a window that loses focus. An alt-tab out of a live drag
  // never delivers the release either, and the drag would otherwise still be
  // armed when the user came back to a pill stuck under a transform.
  useEffect(() => {
    const onBlur = () => endGesture(true)
    window.addEventListener("blur", onBlur)
    return () => window.removeEventListener("blur", onBlur)
  }, [endGesture])

  const onKeyDown = useCallback(
    (ev: React.KeyboardEvent<HTMLElement>) => {
      const current = posRef.current
      if (!current) return
      const { surface, pill } = sizesRef.current
      const next = nudgePillPosition(current, ev.key, surface, pill)
      // Every other key still belongs to the page: the grip is a button, and
      // Space and Enter on it are a tap, which does nothing by design.
      if (!next) return
      ev.preventDefault()
      place(next, true)
    },
    [place],
  )

  return {
    position,
    dragging,
    justDropped,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
    onLostPointerCapture,
    onKeyDown,
  }
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
