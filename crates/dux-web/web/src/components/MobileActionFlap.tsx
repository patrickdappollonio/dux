import type * as React from "react"
import { useLayoutEffect, useRef, useState } from "react"

import { PaneActionCluster } from "@/components/PaneActionCluster"
import { PaneMenu, type PaneMenuSubject } from "@/components/PaneMenu"
import { useTheaterToggleFocusWhen } from "@/hooks/use-theater"
import { buildFlapShape } from "@/lib/flapShape"
import { FLAP_FILL_VAR, registerFlapElement } from "@/lib/theaterFlight"
import type { SelectedTarget } from "@/lib/store"
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
  // The way back into focus after the pill's own theater button was pressed:
  // that press destroyed the pill, and this is the control that replaced it.
  // Only once the flap is really on screen; focus on an invisible control is a
  // keyboard pointed at nothing.
  useTheaterToggleFocusWhen(theaterRef, !hidden)
  // Published so the flight can measure the dock. What travels is one
  // measurement, never control over what the flap does.
  useLayoutEffect(() => registerFlapElement(ref.current), [])

  return (
    <div
      ref={ref}
      data-testid="mobile-action-flap"
      // `-top-px` is the one pixel that makes it a flap rather than a box: the
      // body starts ON the band's bottom hairline, so the fill covers that line
      // for the flap's own width and the two are visibly one shape.
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
        "absolute -top-px right-3 z-30 flex items-center gap-0.5 p-[5px]",
        hidden && "invisible",
        // No drop shadow, deliberately: on the near-black terminal a big soft
        // shadow quantizes into one-step bands whose contours read as a squared
        // ghost box around the flap. The hairline outline is its whole edge
        // treatment.
      )}
      // PUBLISHED, not merely used: the detach and the arrival snap paint the
      // travelling capsule in this colour, and they read it back off this
      // element rather than assuming the strip's tone (see `peekFlapFill`).
      style={
        {
          [FLAP_FILL_VAR]:
            band === "strip" ? "var(--dux-flap-bg)" : "var(--background)",
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
      const width = el.offsetWidth
      const height = el.offsetHeight
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
