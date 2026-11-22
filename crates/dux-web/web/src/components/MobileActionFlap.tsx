import type * as React from "react"
import { useLayoutEffect, useRef, useState } from "react"
import { PanelTopClose, PanelTopOpen } from "lucide-react"

import { PaneActionCluster } from "@/components/PaneActionCluster"
import { PaneMenu, type PaneMenuSubject } from "@/components/PaneMenu"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { useFlapHidden } from "@/hooks/use-flap-hidden"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { useTheaterToggleFocusWhen } from "@/hooks/use-theater"
import { setFlapHidden } from "@/lib/flapHidden"
import { buildFlapShape, FLAP_BLEED } from "@/lib/flapShape"
import {
  FLAP_FILL_VAR,
  FLIGHT_ATTACH_MS,
  FLIGHT_EASE,
  registerFlapElement,
} from "@/lib/theaterFlight"
import { useDux, type SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"

// Where the phone's pane actions live when theater is off: a flap hanging from
// the band above the terminal, taking the colour of whichever band that is
// (usually the header, since the tab strip is only on screen for an agent with
// two or more tabs).
//
// Every pane screen has one, agent or terminal. A project or standalone
// terminal has no changed-file count and no pull request, so its cluster is one
// control shorter and its `⋯` opens the terminal's menu; everything else is the
// same component doing the same thing.
//
// It is dux's own chrome painted over the terminal, so a press on it is never
// forwarded to the PTY, and it sits outside the pane's overlay slot: that slot
// is withheld while a full-pane cover owns the pane, and the flap is the only
// surface carrying the `⋯`, the changed files and the way into theater.
export function MobileActionFlap({
  target,
  subject,
  band,
  hidden = false,
}: {
  /// The pane on screen, which is what Macros writes to and what the `⋯` reads
  /// its input group under. A different question from `subject`: a companion
  /// terminal's pane wears its agent's menu.
  target: SelectedTarget
  /// What the pane is about, which decides the menu the `⋯` opens and whether
  /// there is a changed-file count at all. A project or standalone terminal has
  /// neither a count nor a pull request, so its cluster is one control shorter.
  subject: PaneMenuSubject
  /// What the flap is hanging from, which decides its body color: the tab
  /// strip's own composited tone, or the plain app background when the strip is
  /// not on screen (a single-tab agent, or a hidden top bar).
  band: "strip" | "plain"
  /// Mounted but not painted, which is what the flap is for the whole return
  /// flight: it is the dock the capsule flies onto, so the choreography
  /// measures the real element and the final swap moves nothing.
  hidden?: boolean
}) {
  const ref = useRef<HTMLDivElement | null>(null)
  const theaterRef = useRef<HTMLButtonElement | null>(null)
  const shape = useFlapShape(ref)
  // Folded away by the header's show/hide button: still mounted, so showing it
  // again can slide it back down out of the band.
  const folded = useFlapHidden()
  const reducedMotion = usePrefersReducedMotion()
  // The way back into focus after the pill's own theater button was pressed:
  // that press destroyed the pill, and this is the control that replaced it.
  // Only once the flap is really on screen; focus on an invisible control is a
  // keyboard pointed at nothing.
  useTheaterToggleFocusWhen(theaterRef, !hidden && !folded)
  // Published so the flight can measure the dock. What travels is one
  // measurement, never control over what the flap does.
  useLayoutEffect(() => registerFlapElement(ref.current), [])

  return (
    // The fold's clip: it starts at the top of the SVG's bleed into the band,
    // so a flap sliding up retracts INTO the band rather than painting over the
    // header. It passes every press through to the terminal beneath.
    <div
      className="pointer-events-none absolute inset-x-0 bottom-0 overflow-hidden"
      style={{ top: -(1 + FLAP_BLEED) }}
    >
      <div
        ref={ref}
        data-testid="mobile-action-flap"
        // One pixel above the pane (the clip's top plus the bleed) is the one
        // pixel that makes it a flap rather than a box: the body starts ON the
        // band's bottom hairline, so the fill covers that line for the flap's own
        // width and the two are visibly one shape.
        //
        // Top right, over the few cells where xterm paints its own scrollbar,
        // which is the accepted cost of chrome painted over the terminal.
        //
        // `z-30` clears the chrome stack's `z-10` and the pane's full-pane covers
        // at `z-20`: the flap paints over the band's border to interrupt it, and
        // it is the only surface carrying these controls while a cover owns the
        // terminal. Same level as the floating pill it becomes, which is never
        // painted at the same time.
        className={cn(
          "pointer-events-auto absolute right-3 z-30 flex items-center gap-0.5 p-[5px]",
          // No drop shadow, deliberately: on the near-black terminal a big soft
          // shadow quantizes into one-step bands whose contours read as a squared
          // ghost box around the flap. The hairline outline is its whole edge
          // treatment.
        )}
        // Hidden by its own opacity rather than `visibility`: visibility is
        // inherited, and the cluster's buttons transition `all`, so each of them
        // eased an inherited `hidden` in and stayed painted for a whole button
        // transition beside the pill already flying away. `inert` is what
        // `visibility` also did: no press, no focus, nothing announced.
        inert={hidden || folded}
        // Folded, it is out of the accessibility tree as well as out of reach.
        aria-hidden={folded || undefined}
        // PUBLISHED, not merely used: the detach and the arrival snap paint the
        // travelling capsule in this colour, and they read it back off this
        // element rather than assuming the strip's tone (see `peekFlapFill`).
        style={
          {
            [FLAP_FILL_VAR]:
              band === "strip" ? "var(--dux-flap-bg)" : "var(--background)",
            top: FLAP_BLEED,
            // Folded: lifted by its own height and the band rows above it, which
            // puts the whole silhouette, bleed included, above the clip.
            transform: folded
              ? `translateY(calc(-100% - ${FLAP_BLEED + 2}px))`
              : undefined,
            transition: reducedMotion
              ? undefined
              : `transform ${FLIGHT_ATTACH_MS}ms ${FLIGHT_EASE}`,
            ...(hidden ? { opacity: 0 } : null),
          } as React.CSSProperties
        }
      >
        {shape ? (
          <svg
            aria-hidden
            className="pointer-events-none absolute -z-10"
            width={shape.width}
            height={shape.height}
            viewBox={shape.viewBox}
            style={{ left: shape.left, top: shape.top }}
          >
            <path d={shape.fill} fill={`var(${FLAP_FILL_VAR})`} />
            <path
              d={shape.stroke}
              fill="none"
              stroke="var(--border)"
              strokeWidth={1}
            />
          </svg>
        ) : null}

        <PaneActionCluster
          target={target}
          sessionId={subject.kind === "agent" ? subject.session.id : undefined}
          theaterRef={theaterRef}
          // The one pane menu, which the floating pill opens too: the cluster
          // flies across the screen as one object, so its `⋯` cannot mean
          // something else once it lands.
          ellipsis={
            <PaneMenu
              subject={subject}
              pane={target}
              side="bottom"
              // A phone pane screen's header is Back and identity only, and the
              // cog stayed behind on the hub, so the drill is the way to the
              // app's own actions from here.
              settingsDrill
            />
          }
        />
      </div>
    </div>
  )
}

/**
 * The phone header's one show/hide for the whole flap: the flap covers the
 * terminal's first lines, and this takes it off them and brings it back,
 * remembered on this device for every pane.
 *
 * Ghost like Back beside it: it changes what the header's own chrome shows
 * rather than acting on the session, so it stays quieter than the cluster. Its
 * name, like its tooltip, says what a press does, which also says which way it
 * is set, so it carries no expanded state.
 */
export function FlapToggle() {
  const folded = useFlapHidden()
  const { theater } = useDux()
  const ref = useRef<HTMLButtonElement | null>(null)
  // Leaving theater from the pill destroys the control that was pressed; with
  // the flap folded its theater button is out of reach, so this takes focus.
  // Asked on the mode leaving too, since the collapsing header may never remount.
  useTheaterToggleFocusWhen(ref, folded && !theater)
  const label = folded ? "Show actions" : "Hide actions"
  return (
    <SimpleTooltip content={label} side="bottom">
      <Button
        ref={ref}
        variant="ghost"
        size="icon"
        className="size-10 shrink-0"
        aria-label={label}
        onClick={() => setFlapHidden(!folded)}
      >
        {folded ? <PanelTopOpen /> : <PanelTopClose />}
      </Button>
    </SimpleTooltip>
  )
}

/// Measure the flap's own box and turn it into a silhouette. Generated rather
/// than drawn once, because the cluster's width is not a constant: the count
/// grows a digit, and a pane with no agent behind it carries no count at all.
function useFlapShape(ref: React.RefObject<HTMLDivElement | null>) {
  const [box, setBox] = useState({ width: 0, height: 0 })
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const read = () => {
      // The width the browser PAINTS: the box's edges snapped to device
      // pixels, which is how the pill landing on this box paints its own. The
      // cluster is fractional whenever it carries the changed-file count, and
      // `offsetWidth` rounds in whole CSS pixels, so the silhouette's right side
      // could sit half a pixel off the pill's border at the swap. The SVG's
      // origin snaps with the box's left edge wherever the fillet and overhang
      // span a whole number of device pixels, which every integer ratio gives.
      const r = el.getBoundingClientRect()
      const dpr = window.devicePixelRatio || 1
      const snap = (v: number) => Math.round(v * dpr) / dpr
      const width = snap(r.right) - snap(r.left)
      const height = snap(r.bottom) - snap(r.top)
      setBox((prev) =>
        prev.width === width && prev.height === height
          ? prev
          : { width, height },
      )
    }
    read()
    if (typeof ResizeObserver === "undefined") return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref])
  return buildFlapShape(box)
}
