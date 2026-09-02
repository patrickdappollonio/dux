import { Ellipsis, GripVertical, Minimize2 } from "lucide-react"
import type * as React from "react"
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"

import { AppMenuBody } from "@/components/AppMenu"
import { InputMenuItems } from "@/components/InputMenuItems"
import { MobileActionCluster } from "@/components/MobileActionCluster"
import { MobilePaneMenu } from "@/components/MobilePaneMenu"
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
import { FLAP_FILLET_BOX, filletShape } from "@/lib/flapShape"
import {
  FLIGHT_ATTACH_MS,
  FLIGHT_EASE,
  FLIGHT_SHAPE_MS,
  FLIGHT_TAB_RADIUS_PX,
  FLIGHT_TRAVEL_MS,
  flightOffset,
  flightOwnsPosition,
  flightTranslation,
  peekFlapRect,
  transparentShadow,
  type FlightPhase,
} from "@/lib/theaterFlight"
import { notifyInfo } from "@/lib/notify"
import { exitTheater, type SelectedTarget } from "@/lib/store"
import {
  classifyPillGesture,
  clampPillPosition,
  markPillHintShown,
  nudgePillPosition,
  PILL_GRIPLESS_CLASS,
  readPillHintPending,
  readPillPosition,
  resolvePillPosition,
  THEATER_PILL_GRIP_SLOT_PX,
  THEATER_PILL_HOLD_MS,
  writePillPosition,
  type PillPosition,
  type PillSize,
} from "@/lib/theaterPill"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// THE FLOATING PILL: the only chrome theater mode leaves on screen.
//
// It carries controls that ACT, and nothing that merely reports. On a computer
// that is the macros trigger, the app menu and the way out; on a phone it is
// the docked flap's own four, in the flap's order, because the pill IS the flap
// in the air.
//
// IT CARRIES NO TAB STATUS. It used to grow a status half that bobbed while a
// hidden tab worked, wore an attention dot, and folded out a mini strip of tab
// pills to switch between them. The agents list is where tab status lives, and
// a second, smaller copy of it floating over the terminal was a place for the
// two to disagree; attention arrives as a toast, which reaches the user
// whatever surface they are looking at.
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
  variant = "desktop",
  flight = null,
}: {
  target: SelectedTarget
  /// The focused pane's owning session, when it has one. A terminal pane passes
  /// `undefined` and gets the collapsed pill.
  session: SessionView | undefined
  /// WHICH CLUSTER THIS IS. On a computer the pill is a way BACK: theater is
  /// entered from the header, so the pill carries what the mode took away plus
  /// the exit. On a phone the pill IS the docked flap, in the air: the same
  /// four controls in the same order, so the flight between them reads as one
  /// object moving rather than two clusters swapping.
  variant?: "desktop" | "mobile"
  /// The phone's flight stage, or `null` on a surface that does not fly.
  flight?: FlightPhase | null
}) {
  const boxRef = useRef<HTMLDivElement | null>(null)
  const exitRef = useRef<HTMLButtonElement | null>(null)
  useTheaterPillFocus(exitRef)
  const coarse = useIsCoarsePointer()
  const reducedMotion = usePrefersReducedMotion()
  const drag = usePillDrag(boxRef)
  usePillHint()
  const sessionId = session?.id
  // The PTY behind the pane this pill is painted over, named the same way the
  // shells key the pane itself, so the pill reads the input menu of the pane it
  // is actually on rather than whichever one registered last.
  const paneId = target.kind === "agent" ? target.tabId : target.terminalId
  const mobile = variant === "mobile"
  const gripless = useFlightChoreography(boxRef, flight, drag.position)
  // WHILE IT IS FLYING HOME the flight owns the box's coordinates outright: it
  // pins the pill at the ones it is leaving and parks it on the flap's, neither
  // of which is a place the drag state has any business holding.
  const flightPlaces = flight !== null && flightOwnsPosition(flight)

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
        //
        // It applies DURING A FLIGHT TOO, for the pane that remounts mid-return:
        // a fresh pill has no coordinates of its own and the flight has not yet
        // written any, and the two together used to leave it painted at the
        // overlay's top-left corner. The two can never fight over the same edges,
        // because the run that parks the box pins `right` and `bottom` to `auto`
        // as it writes `left` and `top`.
        drag.position === null && "right-3.5 bottom-3.5",
        // The settle after a nudge or a re-clamp. It is deliberately absent
        // while a drag is live (the pointer already moves the pill, and easing
        // toward the finger would lag it) and for a viewer who asked for less
        // motion, who still gets every clamp, just instantly.
        !reducedMotion &&
          !drag.dragging &&
          !drag.justDropped &&
          !flightPlaces &&
          "transition-[left,top] duration-150 ease-out",
        gripless && PILL_GRIPLESS_CLASS,
        flight === "detaching" && "dux-flight-out",
        flight === "returning" && "dux-flight-in",
        flight === "attaching" && "dux-flight-attach",
      )}
      style={
        flightPlaces || drag.position === null
          ? undefined
          : { left: drag.position.x, top: drag.position.y }
      }
    >
      {mobile ? <FlapFillets /> : null}
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
          //
          // `dux-pill-grip` is the slot the flight widens: the docked flap has
          // no grip and reserves no blank space for one, so this width is what
          // the cluster GAINS on the way out and gives back on the way home.
          //
          // On the phone the slot is 18px wide, a deliberate per-axis
          // relaxation of the 40px floor. It keeps the full 40px HEIGHT; its
          // horizontal neighbours are the pill's own padding edge on one side
          // and the theater toggle on the other, and the pill has to be the
          // flap's width plus exactly this slot for the handoff to be a pure
          // translation. A stray tap costs a mode toggle with a visible way
          // back, the cheapest of the four to hit by mistake.
          className={cn(
            "dux-pill-grip h-10 shrink-0 cursor-grab touch-none rounded-full text-muted-foreground select-none active:cursor-grabbing",
            mobile ? "w-[18px] px-0" : "w-10",
          )}
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

      {mobile ? (
        // THE FLAP'S OWN CLUSTER, in the air. Same component, same order, same
        // offsets: the detach overlays the two exactly and then translates, so
        // anything that differed here would tear at the handoff. The way out is
        // the theater toggle at its head rather than a separate exit, which is
        // also what makes the toggle one control changing state rather than two
        // buttons trading places.
        <MobileActionCluster
          target={target}
          sessionId={sessionId}
          theaterRef={exitRef}
          // THE SAME `⋯` THE FLAP CARRIES, by the same name: the cluster flew
          // here as one object, and a button that changed what it opens on
          // arrival would make the animation a lie. It is also the only way to
          // the agent's own actions while the mode is on. A pane with no agent
          // behind it has no such menu, and falls back to the app menu the way
          // the desktop pill does.
          ellipsis={
            session ? (
              <MobilePaneMenu session={session} side="top" />
            ) : (
              <TheaterAppMenu paneId={paneId} />
            )
          }
        />
      ) : (
        <>
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
        </>
      )}
    </div>
  )
}

// THE FLAP'S CONCAVE FILLETS, worn by the pill.
//
// They are the same arcs the docked flap draws, not an approximation of them:
// the flight leaves a tab shape and re-forms one, and a gradient standing in
// for an arc makes the two visibly different objects at the moment they are
// supposed to be the same one. They are `display: none` except during the two
// stages that need them (see the `dux-flight-*` rules in index.css), so the
// floating pill is a plain capsule with nothing hanging off it.
function FlapFillets() {
  const left = filletShape("left")
  const right = filletShape("right")
  const box = FLAP_FILLET_BOX
  return (
    <>
      {[
        { side: "l", shape: left },
        { side: "r", shape: right },
      ].map(({ side, shape }) => (
        <svg
          key={side}
          aria-hidden
          className={`dux-pill-fillet dux-pill-fillet-${side}`}
          width={box}
          height={box}
          viewBox={`0 0 ${box} ${box}`}
        >
          <path d={shape.fill} fill="var(--dux-flap-bg)" />
          <path
            d={shape.stroke}
            fill="none"
            stroke="var(--border)"
            strokeWidth={1}
          />
        </svg>
      ))}
    </>
  )
}

/**
 * THE FLIGHT ITSELF: the imperative half of the choreography.
 *
 * It lives in the pill rather than in the shell that owns the phase because it
 * has to run in a commit where the pill's own coordinates are already ON the
 * element. The pill places itself in a layout effect and that placement is a
 * state write; a parent's layout effect in the same commit would read the box
 * before React had flushed it and fly the cluster to the corner the pill was
 * about to leave.
 *
 * Every stage runs exactly ONCE per entry into it: the effect re-fires when the
 * placement lands, and a detach that ran twice would measure its own transform.
 *
 * Returns whether the grip slot is collapsed right now, which is the one piece
 * of the animation React has to own: the slot's width is what the cluster gains
 * and gives back, and the transition needs the class to change AFTER the box
 * has been measured at the collapsed width.
 */
function useFlightChoreography(
  boxRef: React.RefObject<HTMLDivElement | null>,
  flight: FlightPhase | null,
  /// Where the pill's own state says it sits, or `null` before anything has
  /// been measured. The flights need a measured dock to fly from; the resting
  /// stages need it to know whether React has coordinates of its own to write.
  position: PillPosition | null,
): boolean {
  // The stage's own answer, until the stage's effect overrides it mid-flight.
  // KEYED ON THE STAGE it was written for, so a new stage's default takes over
  // by itself rather than needing a reset pass that would cost a render.
  const [override, setOverride] = useState<{
    phase: FlightPhase
    gripless: boolean
  } | null>(null)
  // Which stage has already had its routine run. Never reset: it only ever
  // holds the stage that ran, so any other stage fails the guard on its own.
  const ranRef = useRef<FlightPhase | null>(null)
  const setStageGripless = useCallback(
    (phase: FlightPhase, gripless: boolean) =>
      setOverride({ phase, gripless }),
    [],
  )

  useLayoutEffect(() => {
    const box = boxRef.current
    if (!box || flight === null) return
    if (ranRef.current === flight) return

    if (flight === "detaching" || flight === "returning") {
      // Both flights need a real dock: one to leave, one to land on. Without a
      // measured pill there is nothing to fly, so the cluster simply appears,
      // which is also the reduced-motion answer.
      if (!position) return
      const from = peekFlapRect()
      if (!from) return
      ranRef.current = flight
      if (flight === "detaching") {
        runDetach(box, from, (on) => setStageGripless("detaching", on))
      } else {
        runReturn(box, from, (on) => setStageGripless("returning", on))
      }
      return
    }

    if (flight === "attaching") {
      const dock = peekFlapRect()
      if (!dock) return
      ranRef.current = flight
      runAttach(box, dock)
      return
    }

    // A resting stage. Anything the flight wrote is over, and leaving it behind
    // would pin the pill's shape at whatever the last frame happened to be.
    ranRef.current = flight
    clearFlightStyles(box, position)
  }, [boxRef, flight, position, setStageGripless])

  // The flap has no grip and reserves no space for one, so the two stages that
  // are the flap's shape start collapsed; the two travels then open or close
  // the slot from their own effects.
  if (override && flight !== null && override.phase === flight) {
    return override.gripless
  }
  return flight === "detaching" || flight === "attaching"
}

/// Everything the flight writes inline, in one place, so a stage that ends can
/// hand the element back exactly as it found it.
///
/// LEFT AND TOP ARE REACT'S while the pill rests, and this is the one place that
/// has to remember it. React writes them from the pill's own position state, and
/// its next diff sees values that never changed, so a blanket clear here strands
/// the settled pill at the overlay's top-left corner with nothing left to put it
/// back. They are cleared only when the pill has no position of its own, which
/// is also the state the fallback corner class paints.
function clearFlightStyles(
  box: HTMLElement,
  position: PillPosition | null,
): void {
  const style = box.style
  style.transition = ""
  style.transform = ""
  style.transformOrigin = ""
  style.borderRadius = ""
  style.backgroundColor = ""
  style.boxShadow = ""
  style.borderTopColor = ""
  style.willChange = ""
  style.right = ""
  style.bottom = ""
  if (position) return
  style.left = ""
  style.top = ""
}

/// The pill's box radius as a PIXEL value.
///
/// The morph's endpoint is the painted capsule's radius, never `999px`:
/// transitioning to a clamped value spends the whole animation above the clamp,
/// so the corners sit finished for most of it and then appear to snap.
function capsuleRadiusPx(box: HTMLElement): string {
  return `${box.offsetHeight / 2}px`
}

/// Park the box on real coordinates, and say so on BOTH axes.
///
/// The fallback corner is a class rather than an inline style, so a pill that
/// has not been measured yet is holding `right` and `bottom` while a flight
/// writes `left` and `top`; an absolutely positioned box given all four stops
/// being content-sized and stretches. Overriding the pair the flight does not
/// own is what makes the two safe to coexist for the commit it takes React to
/// drop the class.
function pinTopLeft(
  box: HTMLElement,
  here: { left: number; top: number },
): void {
  box.style.left = `${here.left}px`
  box.style.top = `${here.top}px`
  box.style.right = "auto"
  box.style.bottom = "auto"
}

function surfaceOffset(box: HTMLElement, rect: DOMRect): {
  left: number
  top: number
} {
  const parent = box.parentElement?.getBoundingClientRect()
  return flightOffset(rect, parent ?? { left: 0, top: 0 })
}

/// PULL-OFF. The pill starts as the flap, in the flap's place, and becomes a
/// floating capsule at its dock over one travel.
function runDetach(
  box: HTMLElement,
  from: DOMRect,
  setGripless: (on: boolean) => void,
): void {
  const shadow = transparentShadow(getComputedStyle(box).boxShadow)
  const to = box.getBoundingClientRect()
  const move = flightTranslation(from, to)

  box.style.transition = "none"
  box.style.willChange = "transform"
  box.style.transformOrigin = "top left"
  box.style.transform = `translate(${move.x}px, ${move.y}px)`
  // The shape it is leaving: the flap's square top and hanging corners, its
  // body colour, no shadow, and no top edge at all.
  box.style.borderRadius = `0 0 ${FLIGHT_TAB_RADIUS_PX}px ${FLIGHT_TAB_RADIUS_PX}px`
  box.style.backgroundColor = "var(--dux-flap-bg)"
  box.style.borderTopColor = "transparent"
  if (shadow) box.style.boxShadow = shadow
  // Force the browser to take all of that before the end values land, or the
  // two writes coalesce into one and there is nothing to animate.
  void box.offsetWidth

  box.style.transition = [
    `transform ${FLIGHT_TRAVEL_MS}ms ${FLIGHT_EASE}`,
    `border-radius ${FLIGHT_SHAPE_MS}ms ${FLIGHT_EASE}`,
    `background-color ${FLIGHT_SHAPE_MS}ms ease`,
    `border-top-color ${FLIGHT_SHAPE_MS}ms ease`,
    // The shadow rides the WHOLE travel: it belongs to the floating pill, so it
    // arrives with it rather than appearing at pull-off.
    `box-shadow ${FLIGHT_TRAVEL_MS}ms ease`,
  ].join(", ")
  box.style.transform = ""
  box.style.backgroundColor = ""
  box.style.borderTopColor = ""
  box.style.boxShadow = ""
  box.style.borderRadius = capsuleRadiusPx(box)
  // The slot opens on the travel's own clock, stretching the capsule leftward
  // as it goes. React owns this one, and flushes it before the next paint.
  setGripless(false)
}

/// THE WAY HOME. Travel first, as a finished capsule; the shape morph is the
/// separate arrival snap, so nothing flies through the air wearing a tab shape.
function runReturn(
  box: HTMLElement,
  dock: DOMRect,
  setGripless: (on: boolean) => void,
): void {
  const shadow = transparentShadow(getComputedStyle(box).boxShadow)
  const from = box.getBoundingClientRect()
  const here = surfaceOffset(box, from)
  const move = flightTranslation(dock, from)

  box.style.transition = "none"
  box.style.willChange = "transform"
  // Pinned LEFT and TOP for the flight. The grip collapse shrinks the box, and
  // a right-anchored one would slide its left edge out from under the
  // translation; left-anchored, the buttons walk continuously toward that edge
  // as the slot narrows and the translation lands it on the flap's.
  pinTopLeft(box, here)
  box.style.transformOrigin = "top left"
  box.style.borderRadius = capsuleRadiusPx(box)
  void box.offsetWidth

  box.style.transition = [
    `transform ${FLIGHT_TRAVEL_MS}ms ${FLIGHT_EASE}`,
    `box-shadow ${FLIGHT_TRAVEL_MS}ms ease`,
  ].join(", ")
  // The capsule must land SHADOWLESS: the flap has no shadow, so a swap while
  // one was still painted would wipe a dark smear in a single frame.
  if (shadow) box.style.boxShadow = shadow
  box.style.transform = `translate(${move.x}px, ${move.y}px)`
  setGripless(true)
}

/// ARRIVAL. Park on the pixel grid first, then square into the tab shape.
function runAttach(box: HTMLElement, dock: DOMRect): void {
  const here = surfaceOffset(box, dock)
  box.style.transition = "none"
  pinTopLeft(box, here)
  box.style.transform = ""
  // A live fractional transform composites the pill's glyphs off the device
  // pixel grid, half a pixel adrift of where the in-flow flap will paint them,
  // and the final swap would nudge every icon. Parked on real coordinates with
  // the transform cleared and the compositor layer dropped, the raster
  // re-snaps and the swap moves nothing.
  box.style.willChange = "auto"
  void box.offsetWidth

  box.style.transition = [
    `border-radius ${FLIGHT_ATTACH_MS}ms ${FLIGHT_EASE}`,
    `background-color ${FLIGHT_ATTACH_MS}ms ease`,
    `border-top-color ${FLIGHT_ATTACH_MS}ms ease`,
  ].join(", ")
  box.style.borderRadius = `0 0 ${FLIGHT_TAB_RADIUS_PX}px ${FLIGHT_TAB_RADIUS_PX}px`
  box.style.backgroundColor = "var(--dux-flap-bg)"
  // The flap is flush with the band, so it has no top edge to draw.
  box.style.borderTopColor = "transparent"
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

/// How much width the collapsed grip slot is about to give back, or zero for a
/// pill whose slot is already open. Read from the element rather than threaded
/// down from the choreography, because the class and the measurement have to be
/// the same commit's answer and the class is what the browser is laying out.
function griplessSlotWidth(box: HTMLElement): number {
  return box.classList.contains(PILL_GRIPLESS_CLASS)
    ? THEATER_PILL_GRIP_SLOT_PX
    : 0
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
function usePillDrag(boxRef: React.RefObject<HTMLDivElement | null>): PillDrag {
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
      // MEASURED AT THE WIDTH IT WILL SETTLE AT, not the one it is passing
      // through. The detach starts with the grip slot collapsed (that is what
      // makes the handoff a pure translation), and a resting corner derived
      // from that narrower box puts the pill's right edge outside the surface
      // the moment the slot opens, which the re-clamp then yanks back with no
      // transition on `left` to carry it. The class the slot is collapsed by is
      // on the element in the same commit this reads, so the DOM answers.
      pill: {
        width: p.width + griplessSlotWidth(box),
        height: p.height,
      },
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
    setDragging(true)
  }, [])

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
