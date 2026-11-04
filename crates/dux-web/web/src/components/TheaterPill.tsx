import { Ellipsis, GripVertical } from "lucide-react"
import type * as React from "react"
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"

import { AppMenuBody } from "@/components/AppMenu"
import { InputMenuItems } from "@/components/InputMenuItems"
import { PaneActionCluster } from "@/components/PaneActionCluster"
import { PaneMenu, type PaneMenuSubject } from "@/components/PaneMenu"
import { PaneInputGroup } from "@/components/PaneInputGroup"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { useTheaterPillFocus } from "@/hooks/use-theater"
import { FLAP_FILLET_BOX, filletShape } from "@/lib/flapShape"
import {
  FLIGHT_ATTACH_MS,
  FLIGHT_EASE,
  FLIGHT_SHAPE_MS,
  FLIGHT_TAB_RADIUS_PX,
  FLIGHT_TRAVEL_MS,
  FLAP_FILL_VAR,
  flightEaseProgress,
  flightOffset,
  flightTranslation,
  flightTurnedInAir,
  peekFlapFill,
  peekFlapRect,
  transparentShadow,
  type FlightPhase,
} from "@/lib/theaterFlight"
import { notifyInfo } from "@/lib/notify"
import type { SelectedTarget } from "@/lib/store"
import {
  classifyPillGesture,
  clampPillPosition,
  markPillHintShown,
  nudgePillPosition,
  readPillHintPending,
  readPillPosition,
  resolvePillPosition,
  THEATER_PILL_GRIP_SLOT_PX,
  writePillPosition,
  type PillPosition,
  type PillSize,
} from "@/lib/theaterPill"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

import {
  settleParts,
  startParts,
  turnParts,
  type FlightParts,
} from "./theaterFlightParts"
import { theaterPillBox } from "./theaterPillBox"

// The only chrome theater mode leaves on screen. It carries controls that act
// and nothing that reports, so no tab status rides here.
//
// One cluster, one look, both form factors: the phone's handoff overlays the
// pill on the docked flap and translates, so any difference tears the flight.
// Rendered inside the terminal surface's own positioned box, never beside it,
// so an input row appearing cannot move the pill onto a tap target.
export function TheaterPill({
  target,
  session,
  flight = null,
}: {
  target: SelectedTarget
  /// The focused pane's owning session, when it has one. A terminal pane passes
  /// `undefined`, which is what drops the changed-file count.
  session: SessionView | undefined
  /// The flight stage, or `null` on a surface with no docked flap to leave from
  /// or land on, where the cluster simply appears.
  flight?: FlightPhase | null
}) {
  const boxRef = useRef<HTMLDivElement | null>(null)
  // Focus lands on the cluster's theater toggle: the press that raised this pill
  // destroyed the control the user was on.
  const exitRef = useRef<HTMLButtonElement | null>(null)
  useTheaterPillFocus(exitRef)
  const coarse = useIsCoarsePointer()
  const reducedMotion = usePrefersReducedMotion()
  const drag = usePillDrag(boxRef)
  usePillHint()
  const sessionId = session?.id
  // The PTY behind the pane this pill is painted over, keyed the way the shells
  // key the pane, so the input menu read is that pane's and not the last registered.
  const paneId = target.kind === "agent" ? target.tabId : target.terminalId
  // What the `⋯` is about, resolved as the docked flap resolves it: the agent
  // when there is one, otherwise the terminal on screen.
  const paneSubject: PaneMenuSubject | null = session
    ? { kind: "agent", session }
    : target.kind === "terminal"
      ? { kind: "terminal", terminalId: target.terminalId, owner: target.owner }
      : null
  // The fillets belong to the flight, not to a form factor, and are hidden
  // outside its stages, so this decides DOM weight only.
  const flies = flight !== null
  const gripless = useFlightChoreography(
    boxRef,
    flight,
    drag.position,
    drag.restMovedRef,
  )
  const box = theaterPillBox({
    flight,
    position: drag.position,
    dragging: drag.dragging,
    justDropped: drag.justDropped,
    reducedMotion,
    gripless,
  })

  return (
    <div
      ref={boxRef}
      data-testid="theater-pill"
      className={box.className}
      style={box.style}
    >
      {flies ? <FlapFillets /> : null}
      <SimpleTooltip content={coarse ? "" : "Drag to move"}>
        <Button
          variant="ghost"
          size="icon"
          data-testid="theater-pill-grip"
          // A native button, deliberately: no ARIA role describes a handle that
          // moves an object in two axes, and focusability plus keyboard delivery
          // come free. The consequence, accepted: a screen reader says "button"
          // and Enter or Space does nothing, since a press on the grip is inert,
          // so the label names the gesture and the keys that stand in for it.
          aria-label="Drag handle: drag, or use the arrow keys, to move the pill"
          // `touch-none` stops the browser scrolling or long-pressing the page
          // out from under the drag, and keeps the terminal's own long-press
          // selection from starting under the finger.
          //
          // `dux-pill-grip` is the slot the flight widens: the docked flap has
          // no grip, so this width is what the cluster gains and gives back.
          // Its 18px is a per-axis relaxation of the 40px floor, keeping the
          // full height; the pill has to be the flap's width plus exactly this
          // slot for the phone's handoff to be a pure translation, its
          // horizontal neighbours are the padding edge and the theater toggle,
          // and a stray tap on it does nothing at all.
          className={cn(
            "dux-pill-grip h-10 w-[18px] shrink-0 cursor-grab touch-none rounded-full px-0 text-muted-foreground select-none active:cursor-grabbing",
            // Inert paint: it indicates a grab, it does not offer a press. The
            // grip's own tooltip stamps data-popup-open, which the shared button
            // base would otherwise repaint as a pressed fill.
            "hover:bg-transparent hover:text-muted-foreground dark:hover:bg-transparent",
            "data-[popup-open]:bg-transparent data-[popup-open]:text-muted-foreground",
            "active:not-aria-[haspopup]:translate-y-0",
          )}
          onPointerDown={drag.onPointerDown}
          onPointerMove={drag.onPointerMove}
          onPointerUp={drag.onPointerUp}
          onPointerCancel={drag.onPointerCancel}
          onLostPointerCapture={drag.onLostPointerCapture}
          // A keyboard cannot hold and pull, so the arrow keys move the pill a
          // step at a time; without them it cannot be cleared at all.
          onKeyDown={drag.onKeyDown}
        >
          <GripVertical />
        </Button>
      </SimpleTooltip>

      {/* The flap's own cluster, in the air: same component, order and offsets,
        * because the phone's detach overlays the two exactly and then
        * translates. The way out is the theater toggle at its head, so it is one
        * control changing state rather than two buttons trading places. */}
      <PaneActionCluster
        target={target}
        sessionId={sessionId}
        theaterRef={exitRef}
        // The same `⋯` the flap carries, and the only way to the pane's own
        // actions while the mode is on. The app-menu fallback is for a pane that
        // is neither an agent nor a terminal, which the types say cannot happen.
        ellipsis={
          paneSubject ? (
            <PaneMenu
              subject={paneSubject}
              pane={target}
              side="top"
              // Theater unmounts the chrome the cog lives in on both form
              // factors, so without the drill the app's actions are unreachable.
              settingsDrill
            />
          ) : (
            <TheaterAppMenu paneId={paneId} />
          )
        }
      />
    </div>
  )
}

// The docked flap's concave fillets, worn by the pill: the same arcs, not an
// approximation, or the flight's two ends read as different objects. They are
// `display: none` outside the flight stages and faded out inside them unless
// the flight is animating them (`theaterFlightParts.ts`).
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
          // Faded out unless a flight animation is driving it: what makes a
          // tab shape that nothing is fading or growing impossible to paint.
          style={{ opacity: 0 }}
          width={box}
          height={box}
          viewBox={`0 0 ${box} ${box}`}
        >
          {/* The band's own colour, published by the flap and put on the pill's
            * root by the flight: a single-tab agent's flap hangs off the plain
            * background rather than the strip's tone. */}
          <path d={shape.fill} fill={`var(${FLAP_FILL_VAR}, var(--dux-flap-bg))`} />
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
 * The imperative half of the flight choreography. It lives in the pill because
 * it must run in a commit where the pill's own coordinates are already on the
 * element; a parent's layout effect would read the box before React flushed it.
 *
 * Every stage runs once per entry into it: the effect re-fires when the
 * placement lands, and a detach that ran twice would measure its own transform.
 *
 * Returns whether the grip slot is collapsed right now, which React has to own
 * because the transition needs the class to change after the box has been
 * measured at the collapsed width.
 */
function useFlightChoreography(
  boxRef: React.RefObject<HTMLDivElement | null>,
  flight: FlightPhase | null,
  /// Where the pill's own state says it sits, or `null` before anything has
  /// been measured. Every stage needs it: to fly from, or to know what to clear.
  position: PillPosition | null,
  /// Where the pill's resting corner is told it moved on screen.
  restMovedRef: React.RefObject<((shift: ScreenShift) => void) | null>,
): boolean {
  // Keyed on the stage it was written for, so a new stage's default takes over
  // by itself rather than needing a reset pass that would cost a render.
  const [override, setOverride] = useState<{
    phase: FlightPhase
    gripless: boolean
  } | null>(null)
  // Which stage has already had its routine run. Never reset: it only ever
  // holds the stage that ran, so any other stage fails the guard on its own.
  const ranRef = useRef<FlightPhase | null>(null)
  // The glyph, grip and fillet animations this pill is running.
  const partsRef = useRef<FlightParts>(new Map())
  // The coordinates on the element when the last commit was painted. A painted
  // transform is relative to THOSE, and a commit that hands a flight over can
  // move them: the pill's own state is re-clamped while a return is pinned on
  // where it left, and the departure that turns it around is placed from the
  // state again.
  const paintedAtRef = useRef<{ left: number; top: number } | null>(null)
  // When the running travel started, for a re-clamp that has to know how far
  // along it is where the browser's own clock for it cannot be read.
  const travelStartRef = useRef(0)
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
      // Both flights need a real dock, one to leave and one to land on; without
      // a measured pill the cluster simply appears.
      if (!position) return
      const from = peekFlapRect()
      if (!from) return
      // The stage whose routine ran last is the one this travel follows.
      const previous = ranRef.current
      ranRef.current = flight
      // Where this travel starts: off the band, or from a flight it turned
      // around, which leaves from the frame on screen and the shape it had.
      const start: FlightStart = flightTurnedInAir(previous, flight)
        ? "air"
        : flight === "detaching" && previous === "attaching"
          ? "snap"
          : "dock"
      // Both travels rest on the pill's own state, so a turn carries the
      // painted transform over by however far that state moved from the
      // coordinates it was painted on.
      travelStartRef.current = performance.now()
      const was = paintedAtRef.current
      const carry =
        start === "air" && was
          ? { x: was.left - position.x, y: was.top - position.y }
          : { x: 0, y: 0 }
      if (flight === "detaching") {
        runDetach(box, from, start, carry, (on) =>
          setStageGripless("detaching", on),
        )
      } else {
        runReturn(box, from, position, start, carry, (on) =>
          setStageGripless("returning", on),
        )
      }
      if (start === "dock") startParts(box, partsRef.current, flight)
      else turnParts(box, partsRef.current, flight, start === "air")
      return
    }

    if (flight === "attaching") {
      const dock = peekFlapRect()
      if (!dock) return
      ranRef.current = flight
      runAttach(box, dock)
      startParts(box, partsRef.current, "attaching")
      return
    }

    // A resting stage: leaving the flight's inline writes behind would pin the
    // pill's shape at whatever the last frame happened to be.
    ranRef.current = flight
    clearFlightStyles(box, position)
    settleParts(partsRef.current)
  }, [boxRef, flight, position, setStageGripless])

  // A departure flies to a corner it does not pin, and its travel is a
  // transform in the surface the corner is measured in, so anything that moves
  // that corner on screen (a re-clamp, the surface itself moving, or both)
  // would move the painted pill with it. Told before the move is committed,
  // and in the same frame, so the carry and the move are painted together.
  useLayoutEffect(() => {
    restMovedRef.current = (shift) => {
      const box = boxRef.current
      if (box && ranRef.current === "detaching") {
        carryReclamp(box, shift, travelStartRef.current)
      }
    }
    return () => {
      restMovedRef.current = null
    }
  }, [boxRef, restMovedRef])

  // After every commit, the routine above included: what the next frame paints
  // the transform from.
  useLayoutEffect(() => {
    const box = boxRef.current
    const left = Number.parseFloat(box?.style.left ?? "")
    const top = Number.parseFloat(box?.style.top ?? "")
    paintedAtRef.current =
      Number.isFinite(left) && Number.isFinite(top) ? { left, top } : null
  })

  // The flap has no grip and reserves no space for one, so the shape stages
  // start collapsed; the travels open or close the slot from their own effects.
  // A turned flight starts where the slot already is, and its CSS transition
  // reverses from that width when the travel flips the class.
  if (override && flight !== null && override.phase === flight) {
    return override.gripless
  }
  return flight === "detaching" || flight === "attaching"
}

/// Where a travel leaves from. `dock`: the flap on the band, in the flap's
/// shape. `snap`: the band too, but mid-way through squaring up, so the shape is
/// whatever the snap had reached. `air`: a flight turned around, so both the
/// place and the shape are whatever is on screen this frame.
type FlightStart = "dock" | "snap" | "air"

/// The values a turned-around travel starts from, read off the frame on screen.
/// Read before anything is written: the running transitions are cancelled the
/// moment the inline `transition` changes, and a cancelled transition reports
/// its END value, which is the jump this exists to avoid.
interface PaintedFlight {
  transform: string
  /// What a re-clamp's carry still adds on top of the transform, as computed.
  translate: string
  borderRadius: string
  borderTopColor: string
  boxShadow: string
}

function readPainted(box: HTMLElement): PaintedFlight {
  const cs = getComputedStyle(box)
  return {
    transform: cs.transform,
    translate: cs.translate,
    borderRadius: cs.borderRadius,
    borderTopColor: cs.borderTopColor,
    boxShadow: cs.boxShadow,
  }
}

/// Hold the shape the frame on screen has, so the next transition eases on
/// from it. The caller has already switched transitions off.
function holdPaintedShape(box: HTMLElement, painted: PaintedFlight): void {
  box.style.borderRadius = painted.borderRadius
  box.style.borderTopColor = painted.borderTopColor
  box.style.boxShadow = painted.boxShadow
}

/// The transform the frame on screen has, as an inline value, for a box whose
/// coordinates moved by `-carry` since that frame was painted.
function paintedTransform(
  painted: PaintedFlight,
  carry: { x: number; y: number },
): string {
  const own = painted.transform === "none" ? "" : painted.transform
  const carried = translateOf(painted.translate)
  const x = carry.x + carried.x
  const y = carry.y + carried.y
  if (x === 0 && y === 0) return own
  return `translate(${x}px, ${y}px) ${own}`.trim()
}

/// A computed `translate`, which only ever holds a re-clamp's carry, as the
/// offset it paints. `none` and an empty value are no offset at all.
function translateOf(value: string | undefined): { x: number; y: number } {
  if (!value || value === "none") return { x: 0, y: 0 }
  const [x = 0, y = 0] = value
    .trim()
    .split(/\s+/)
    .map((v) => Number.parseFloat(v) || 0)
  return { x, y }
}

/// The resting corner moved on screen in the middle of a departure, by a
/// re-clamp, by the surface itself moving, or both. The travel is a transform
/// off that corner, so the painted pill would move with it, by the whole move
/// in one frame when the move is abrupt. The move is put back as a separate
/// `translate`, which leaves the travel's own transition untouched, and that
/// offset is run down on the travel's own curve from the point the travel has
/// reached, so the two land together and the pill's path bends toward the new
/// corner rather than wobbling between two motions on different clocks.
///
/// Measured on screen, not in the surface: chrome collapsing above the pane
/// moves the surface up exactly as far as it moves a bottom corner down it, and
/// that corner never moved, while a corner placed away from the bottom moves
/// with the surface, and that move is carried like any other.
function carryReclamp(
  box: HTMLElement,
  shift: ScreenShift,
  startedAt: number,
): void {
  if (shift.x === 0 && shift.y === 0) return
  const elapsed = travelElapsed(box) ?? performance.now() - startedAt
  const remaining = FLIGHT_TRAVEL_MS - elapsed
  if (remaining <= 0) return
  const painted = translateOf(getComputedStyle(box).translate)
  const offset = { x: painted.x - shift.x, y: painted.y - shift.y }
  // What is left of the travel's distance at this point of its curve. Joining
  // the curve here means starting from `offset / left`, of which only `offset`
  // is ever painted. A carry already running is on that same curve, so folding
  // it in keeps it on its own path. Only in the travel's last instant, where
  // the factor grows without bound, does the offset ease out on its own.
  const left = 1 - flightEaseProgress(elapsed / FLIGHT_TRAVEL_MS)
  const lockstep = left >= 0.001
  const scale = lockstep ? 1 / left : 1
  // The travel's own transitions stay listed exactly as they are, so the ones
  // still running carry on. A carry is always the last entry, which is what
  // makes the previous one safe to cut off by position: the easing functions
  // in the list have commas of their own.
  const written = box.style.transition
  const cut = written.indexOf("translate ")
  const travel = (cut < 0 ? written : written.slice(0, cut))
    .trim()
    .replace(/,$/, "")
  const listed = travel === "" ? [] : [travel]
  box.style.transition = [...listed, "translate 0s"].join(", ")
  box.style.translate = `${offset.x * scale}px ${offset.y * scale}px`
  void box.offsetWidth
  box.style.transition = [
    ...listed,
    lockstep
      ? `translate ${FLIGHT_TRAVEL_MS}ms ${FLIGHT_EASE} ${-elapsed}ms`
      : `translate ${remaining}ms ease-out`,
  ].join(", ")
  box.style.translate = ""
}

/// How far the running travel transition is, on the clock the browser paints
/// it on, or `null` where that cannot be read (no Web Animations, or the
/// travel has already finished).
function travelElapsed(box: HTMLElement): number | null {
  if (typeof box.getAnimations !== "function") return null
  for (const anim of box.getAnimations()) {
    if (
      "transitionProperty" in anim &&
      anim.transitionProperty === "transform" &&
      anim.currentTime !== null
    ) {
      return Number(anim.currentTime)
    }
  }
  return null
}

/// Everything the flight writes inline, in one place, so a stage that ends can
/// hand the element back exactly as it found it. `left` and `top` are React's
/// while the pill rests (its next diff sees values that never changed), so they
/// are cleared only when the pill has no position of its own.
function clearFlightStyles(
  box: HTMLElement,
  position: PillPosition | null,
): void {
  const style = box.style
  style.transition = ""
  style.transform = ""
  style.translate = ""
  style.transformOrigin = ""
  style.borderRadius = ""
  style.boxShadow = ""
  style.borderTopColor = ""
  style.willChange = ""
  style.right = ""
  style.bottom = ""
  // THE FILL IS NOT A FLIGHT STYLE ANY MORE. The settled pill wears the band's
  // colour too, so the resting stage re-states it rather than dropping it: the
  // flap is unmounted for the whole floating stage, and a cleared property
  // would repaint a plain-band pill in the strip's tone one commit after it
  // landed.
  style.setProperty(FLAP_FILL_VAR, peekFlapFill())
  if (position) return
  style.left = ""
  style.top = ""
}

/// The pill's box radius as a pixel value. The morph's endpoint must never be
/// `999px`: transitioning to a clamped value spends the whole animation above
/// the clamp, so the corners sit finished and then appear to snap.
function capsuleRadiusPx(box: HTMLElement): string {
  return `${box.offsetHeight / 2}px`
}

/// Park the box on real coordinates, on both axes. An unmeasured pill holds the
/// fallback corner class's `right` and `bottom`, and an absolutely positioned
/// box given all four stops being content-sized and stretches, so the flight
/// overrides the pair it does not own.
function pinTopLeft(
  box: HTMLElement,
  here: { left: number; top: number },
): void {
  box.style.left = `${here.left}px`
  box.style.top = `${here.top}px`
  box.style.right = "auto"
  box.style.bottom = "auto"
}

/// Where the pill's offset parent sits in the viewport, which is the origin the
/// pill's own coordinates are written in.
function parentPoint(box: HTMLElement): { left: number; top: number } {
  const parent = box.parentElement?.getBoundingClientRect()
  return { left: parent?.left ?? 0, top: parent?.top ?? 0 }
}

function surfaceOffset(box: HTMLElement, rect: DOMRect): {
  left: number
  top: number
} {
  return flightOffset(rect, parentPoint(box))
}

/// PULL-OFF. The pill starts as the flap, in the flap's place, and becomes a
/// floating capsule at its dock over one travel.
function runDetach(
  box: HTMLElement,
  from: DOMRect,
  start: FlightStart,
  carry: { x: number; y: number },
  setGripless: (on: boolean) => void,
): void {
  const painted = readPainted(box)
  const shadow = transparentShadow(painted.boxShadow)

  box.style.transition = "none"
  box.style.willChange = "transform"
  box.style.transformOrigin = "top left"
  // Folded into the transform below wherever the flight starts from the frame
  // on screen.
  box.style.translate = ""
  if (start === "air") {
    // Turned around on the way home: React has placed the box on the pill's
    // own state again, so the frame on screen is the painted transform carried
    // over from the coordinates the return was pinned on.
    box.style.transform = paintedTransform(painted, carry)
  } else {
    // A box still carrying a transform measures where that puts it, not where
    // it rests, so it is taken off before the rest is read.
    box.style.transform = ""
    const move = flightTranslation(from, box.getBoundingClientRect())
    box.style.transform = `translate(${move.x}px, ${move.y}px)`
  }
  box.style.setProperty(FLAP_FILL_VAR, peekFlapFill())
  if (start === "dock") {
    // The shape it is leaving: the flap's square top and hanging corners, no
    // shadow, no top edge. The body colour is not morphed, since both ends wear
    // the band's fill; only which band's fill it is has to be said.
    box.style.borderRadius = `0 0 ${FLIGHT_TAB_RADIUS_PX}px ${FLIGHT_TAB_RADIUS_PX}px`
    box.style.borderTopColor = "transparent"
    if (shadow) box.style.boxShadow = shadow
  } else {
    holdPaintedShape(box, painted)
  }
  // Force the browser to take all of that before the end values land, or the
  // two writes coalesce into one and there is nothing to animate.
  void box.offsetWidth

  box.style.transition = [
    `transform ${FLIGHT_TRAVEL_MS}ms ${FLIGHT_EASE}`,
    `border-radius ${FLIGHT_SHAPE_MS}ms ${FLIGHT_EASE}`,
    `border-top-color ${FLIGHT_SHAPE_MS}ms ease`,
    // The shadow rides the whole travel, arriving with the floating pill.
    `box-shadow ${FLIGHT_TRAVEL_MS}ms ease`,
  ].join(", ")
  box.style.transform = ""
  box.style.borderTopColor = ""
  box.style.boxShadow = ""
  box.style.borderRadius = capsuleRadiusPx(box)
  // The slot opens on the travel's own clock, stretching the capsule leftward
  // as it goes. React owns this one, and flushes it before the next paint.
  setGripless(false)
}

/// The way home: travel first as a finished capsule, with the shape morph left
/// to the arrival snap, so nothing flies wearing a tab shape.
///
/// Where it leaves from is the pill's own state, never a box read here: React
/// stops writing the inline coordinates in the commit that hands the stage over,
/// so a measurement now reads the static-layout corner instead.
function runReturn(
  box: HTMLElement,
  dock: DOMRect,
  position: PillPosition,
  start: FlightStart,
  carry: { x: number; y: number },
  setGripless: (on: boolean) => void,
): void {
  const painted = readPainted(box)
  const shadow = transparentShadow(painted.boxShadow)
  const here = { left: position.x, top: position.y }
  const parent = parentPoint(box)
  const from = {
    left: parent.left + here.left,
    top: parent.top + here.top,
  }
  const move = flightTranslation(dock, from)

  box.style.transition = "none"
  box.style.willChange = "transform"
  box.style.translate = ""
  // Pinned left and top for the flight: the grip collapse shrinks the box, and a
  // right-anchored one would slide its left edge out from under the translation.
  pinTopLeft(box, here)
  box.style.transformOrigin = "top left"
  // Turned around on the way out: from the frame on screen, which is a
  // transform off the coordinates last painted, carried over to this pin, in
  // whatever shape the pull-off had reached.
  // The chrome is coming back during this travel and moves the dock with it,
  // but it moves the pill's own surface by the same amount, and the travel is
  // a transform in that surface's coordinates, so it still lands on the dock.
  const air = start === "air"
  if (air) {
    box.style.transform = paintedTransform(painted, carry)
    holdPaintedShape(box, painted)
  }
  const capsule = capsuleRadiusPx(box)
  if (!air) box.style.borderRadius = capsule
  void box.offsetWidth

  box.style.transition = [
    `transform ${FLIGHT_TRAVEL_MS}ms ${FLIGHT_EASE}`,
    `box-shadow ${FLIGHT_TRAVEL_MS}ms ease`,
    ...(air
      ? [
          `border-radius ${FLIGHT_SHAPE_MS}ms ${FLIGHT_EASE}`,
          `border-top-color ${FLIGHT_SHAPE_MS}ms ease`,
        ]
      : []),
  ].join(", ")
  if (air) {
    box.style.borderRadius = capsule
    box.style.borderTopColor = ""
  }
  // The capsule must land shadowless: the flap has none, so a swap with one
  // still painted wipes a dark smear in a single frame.
  if (shadow) box.style.boxShadow = shadow
  box.style.transform = `translate(${move.x}px, ${move.y}px)`
  setGripless(true)
}

/// ARRIVAL. Park on the pixel grid first, then square into the tab shape.
function runAttach(box: HTMLElement, dock: DOMRect): void {
  const here = surfaceOffset(box, dock)
  box.style.transition = "none"
  // The colour it arrives into, taken from the dock: the flap's body is the
  // strip's tone or the plain background depending on what it hangs from.
  box.style.setProperty(FLAP_FILL_VAR, peekFlapFill())
  pinTopLeft(box, here)
  box.style.transform = ""
  // A live fractional transform composites the glyphs off the device pixel grid,
  // so the final swap would nudge every icon; dropping the compositor layer
  // re-snaps the raster first.
  box.style.willChange = "auto"
  void box.offsetWidth

  box.style.transition = [
    `border-radius ${FLIGHT_ATTACH_MS}ms ${FLIGHT_EASE}`,
    `border-top-color ${FLIGHT_ATTACH_MS}ms ease`,
  ].join(", ")
  box.style.borderRadius = `0 0 ${FLIGHT_TAB_RADIUS_PX}px ${FLIGHT_TAB_RADIUS_PX}px`
  // The flap is flush with the band, so it has no top edge to draw.
  box.style.borderTopColor = "transparent"
}

// The fallback `⋯` for a pane that is neither an agent nor a terminal, which the
// types say cannot happen and the surface should survive anyway. Ordinarily the
// merged pane menu carries this same body as its Settings drill.
//
// Theater unmounts every other anchor (the sidebar, the header stack, the phone's
// top bar), so this is the one menu on screen: it renders `AppMenuBody` so it
// cannot diverge from the cog's, plus the pane's input group and the theater
// exit. Named "Settings" after the control it stands in for.
function TheaterAppMenu({ paneId }: { paneId: string }) {
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
      {/* Anchored above the trigger: the pill lives in the pane's bottom corner,
        * where a downward popup has nowhere to go. A phone renders a sheet. */}
      <DropdownMenuContent side="top" align="end">
        <PaneInputGroup ptyIds={[paneId]} />
        <AppMenuBody />
        <DropdownMenuSeparator />
        {/* The guaranteed way out, whatever the pane published. */}
        <InputMenuItems theaterExit />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

// The one-time hint that the pill can be dragged: nothing about a floating pill
// says so. Info and not sticky, since nothing is lost if it goes unread and a
// pinned toast over a mode about screen space would be its own joke. The latch
// is `localStorage` rather than a page-session flag, because entering theater is
// rare enough that a page-lifetime latch would re-teach on every reload.
function usePillHint(): void {
  useEffect(() => {
    if (!readPillHintPending()) return
    // Marked before the raise, so a double-invoked effect cannot toast twice.
    markPillHintShown()
    notifyInfo("Drag the pill's grip to move it anywhere in the terminal.")
  }, [])
}

/// How much narrower the pill is right now than it will settle at: the grip
/// slot's full width less what the grip takes up at this moment, read off the
/// layout as the distance from the grip to the control after it. The class
/// cannot answer this, because it comes off at the START of the slot's widening
/// and every frame of that widening is a box that is still too narrow.
///
/// `settled` is false while the slot is part open: the box and the grip's
/// offset are snapped to the device pixel grid separately, so that frame's sum
/// can be up to a device pixel off the width the pill settles at. A slot within
/// a device pixel of either end is settled, because at a fractional ratio the
/// collapsed grip's border and margin round apart and never meet exactly.
function gripSlotShortfall(box: HTMLElement): { px: number; settled: boolean } {
  const grip = box.querySelector<HTMLElement>('[data-testid="theater-pill-grip"]')
  const next = grip?.nextElementSibling
  if (!grip || !next) return { px: 0, settled: true }
  const occupied =
    next.getBoundingClientRect().left - grip.getBoundingClientRect().left
  const px = THEATER_PILL_GRIP_SLOT_PX - occupied
  const tolerance = devicePixel()
  return {
    px,
    settled:
      Math.abs(px) <= tolerance ||
      Math.abs(px - THEATER_PILL_GRIP_SLOT_PX) <= tolerance,
  }
}

/// One device pixel, in CSS pixels.
function devicePixel(): number {
  const ratio = typeof window === "undefined" ? 1 : window.devicePixelRatio
  return ratio > 0 ? 1 / ratio : 1
}

/// How far something moved on screen, in CSS pixels.
interface ScreenShift {
  x: number
  y: number
}

interface PillDrag {
  /// Where the pill sits, or `null` before anything has been measured.
  position: PillPosition | null
  /// Called whenever the surface changing moves the resting corner on screen,
  /// whether by a re-clamp, by the surface itself moving, or both, before the
  /// new coordinates are committed. The flight sets it.
  restMovedRef: React.RefObject<((shift: ScreenShift) => void) | null>
  /// Whether a drag is live, which suppresses the settle animation.
  dragging: boolean
  /// True for the single commit that lands a drop, suppressing the settle: the
  /// pill is already where the pointer left it.
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
  base: PillPosition
  last: PillPosition
  lifted: boolean
  frame: number | null
}

/**
 * Moving the pill. The position is live state because a restore from this
 * device's memory, a drag, an arrow key and a surface re-clamp all set it, and
 * all of them end in the same clamp, so every button stays reachable.
 *
 * While a drag is live the pill is moved by a transform rather than React state:
 * a re-render per pointer move is a re-render per frame over a painting
 * terminal. State and storage are written once, on release. Pointer capture is
 * what makes the gesture survive the pointer leaving the grip and keeps the
 * terminal underneath from seeing the move; the handlers stop propagation too.
 */
function usePillDrag(boxRef: React.RefObject<HTMLDivElement | null>): PillDrag {
  const [position, setPosition] = useState<PillPosition | null>(null)
  const [dragging, setDragging] = useState(false)
  const [justDropped, setJustDropped] = useState(false)
  const posRef = useRef<PillPosition | null>(null)
  // Where the user asked for it, which is not where it fits today: every clamp
  // is against the surface and pill of the moment, and clamping an already
  // clamped value makes a temporary shove permanent. So the intent is what is
  // stored and what every re-clamp is re-derived from; `null` means unplaced.
  const intentRef = useRef<PillPosition | null>(null)
  // The device's memory is read once, at the first measurement; after that the
  // intent above is the live answer and storage is write-only.
  const restoredRef = useRef(false)
  const sizesRef = useRef<{ surface: PillSize; pill: PillSize }>({
    surface: { width: 0, height: 0 },
    pill: { width: 0, height: 0 },
  })
  const gestureRef = useRef<DragGesture | null>(null)
  // Where the resting corner sat on screen at the last measurement. Kept on
  // screen, not in the surface, because the surface itself moves: a corner
  // re-clamped by exactly as much as its surface moved never moved at all.
  const restOnScreenRef = useRef<ScreenShift | null>(null)
  const restMovedRef = useRef<((shift: ScreenShift) => void) | null>(null)
  // The pill's width as the last frame with a settled grip slot measured it.
  const settledWidthRef = useRef<number | null>(null)

  const place = useCallback((next: PillPosition, persist: boolean) => {
    // A move the user made is meant to be seen, so it is never carried.
    restOnScreenRef.current = null
    posRef.current = next
    intentRef.current = next
    setPosition(next)
    if (persist) writePillPosition(next)
  }, [])

  // Read both boxes and settle on a position. The pill's surface is its own
  // offset parent by construction, and is exactly the area it may roam.
  //
  // `sync` commits the new coordinates before returning. A resize
  // notification is delivered after layout and straight before paint, and an
  // ordinary update lands a task later, so the pill would paint one frame on
  // the old coordinates against the new surface. While the chrome moves under
  // a flight that is a frame of its motion off the pill's path, every frame.
  const measure = useCallback((sync = false) => {
    const box = boxRef.current
    const surfaceEl = box?.parentElement
    if (!box || !surfaceEl) return
    const s = surfaceEl.getBoundingClientRect()
    const p = box.getBoundingClientRect()
    const shortfall = gripSlotShortfall(box)
    const settling = p.width + shortfall.px
    const known = settledWidthRef.current
    // Measured at the width it will settle at, not the one it is passing
    // through: a corner derived from the detach's collapsed box puts the right
    // edge outside the surface the moment the slot opens. A frame with the slot
    // part open is only trusted to say the controls changed: within a device
    // pixel of the width a settled frame gave, it is that width plus rounding,
    // and a surface changing mid-flight would otherwise rest the pill a
    // fraction of a pixel off its corner.
    let width = settling
    if (shortfall.settled) {
      settledWidthRef.current = settling
    } else if (known !== null && Math.abs(settling - known) <= devicePixel()) {
      width = known
    }
    const sizes = {
      surface: { width: s.width, height: s.height },
      pill: { width, height: p.height },
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
    const onScreen = { x: s.left + next.x, y: s.top + next.y }
    const was = restOnScreenRef.current
    restOnScreenRef.current = onScreen
    if (was && (onScreen.x !== was.x || onScreen.y !== was.y)) {
      restMovedRef.current?.({ x: onScreen.x - was.x, y: onScreen.y - was.y })
    }
    const current = posRef.current
    // Every surface change re-clamps, mid-flight included: a pill hanging
    // outside the surface is off screen. The flight stages change only whether
    // the move is animated (see `positionSnaps`).
    if (current && current.x === next.x && current.y === next.y) return
    // Nothing here writes to storage. The user did not move the pill, a window
    // did, and the position they chose has to survive the window changing back.
    posRef.current = next
    if (sync) flushSync(() => setPosition(next))
    else setPosition(next)
  }, [boxRef])


  const endGesture = useCallback(
    (commit: boolean) => {
      const g = gestureRef.current
      if (!g) return
      gestureRef.current = null
      if (g.frame !== null) cancelAnimationFrame(g.frame)
      const box = boxRef.current
      if (box) box.style.transform = ""
      setDragging(false)
      // A gesture that never lifted was a tap, and a tap on the grip does
      // nothing at all: the buttons beside it keep their own meanings.
      if (!commit || !g.lifted) return
      // The drop is not a settle: one commit swaps the transform for real
      // coordinates and re-enables the animation, which would then ease the pill
      // from the corner it was dragged out of. Suppressed for this commit only.
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
    // Both boxes: the surface moves the edges, and the pill changes how much of
    // it has to fit inside them.
    const ro = new ResizeObserver((entries) => {
      // A surface that changes shape mid-drag ends the drag: the transform is
      // measured from the press's base and the re-clamp below moves the
      // coordinates that base is written in, so together they move the pill
      // twice. It commits what it had rather than rebasing.
      //
      // The pill's own box deliberately does not end it: the tab strip folds
      // away on the lift, so every drag starts with the pill changing size.
      if (surfaceEl && entries.some((e) => e.target === surfaceEl)) {
        endGesture(true)
      }
      measure(true)
    })
    if (surfaceEl) ro.observe(surfaceEl)
    if (box) ro.observe(box)
    return () => ro.disconnect()
  }, [boxRef, measure, endGesture])

  // One painted frame: long enough for the drop's coordinates to land without
  // easing, short enough that the next nudge or re-clamp still settles.
  useEffect(() => {
    if (!justDropped) return
    const frame = requestAnimationFrame(() => setJustDropped(false))
    return () => cancelAnimationFrame(frame)
  }, [justDropped])

  const lift = useCallback(() => {
    const g = gestureRef.current
    if (!g || g.lifted) return
    g.lifted = true
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
      // A pane that has never been measured has no coordinates to drag from.
      measure()
      const base = posRef.current
      if (!base) return
      try {
        ev.currentTarget.setPointerCapture(ev.pointerId)
      } catch {
        // Some browsers refuse capture for a pointer that has already been
        // released. The gesture still works while the pointer is over the grip.
      }
      // Nothing is armed on a clock: the grip lifts on the first move past its
      // slop, for a finger as for a mouse, so the press only records where it landed.
      const gesture: DragGesture = {
        pointerId: ev.pointerId,
        pointerType: ev.pointerType,
        startX: ev.clientX,
        startY: ev.clientY,
        base,
        last: base,
        lifted: false,
        frame: null,
      }
      gestureRef.current = gesture
    },
    [measure],
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
          travel: Math.hypot(dx, dy),
          ended: false,
        })
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
    [lift, paint],
  )

  const onPointerUp = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      const g = gestureRef.current
      if (!g) return
      ev.stopPropagation()
      // The same classifier every move uses, so tap versus drag is decided in
      // one place. A press that already lifted is a drag whatever the release
      // looks like; a tap on the grip does nothing at all.
      const verdict = g.lifted
        ? "lift"
        : classifyPillGesture({
            pointerType: g.pointerType,
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

  // A capture that goes away ends the gesture: neither the browser handing the
  // pointer elsewhere nor the element being removed produces a pointerup. It
  // commits rather than cancelling, because the user watched the pill get there.
  // An ordinary release lands here after `onPointerUp` has retired the gesture.
  const onLostPointerCapture = useCallback(
    (ev: React.PointerEvent<HTMLElement>) => {
      const g = gestureRef.current
      if (!g || ev.pointerId !== g.pointerId) return
      endGesture(true)
    },
    [endGesture],
  )

  // A pill that goes away under the finger takes its gesture with it, committing
  // nothing: an unmount is not a drop.
  useEffect(() => () => endGesture(false), [endGesture])

  // A window that loses focus never delivers the release either, and the drag
  // would still be armed on return, with the pill stuck under a transform.
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
    restMovedRef,
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
