import { Ellipsis } from "lucide-react"
import type * as React from "react"
import { useLayoutEffect, useRef, useState } from "react"

import { AgentActionsMenu } from "@/components/FlatAgentList"
import { MobileActionCluster } from "@/components/MobileActionCluster"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useTheaterToggleFocusWhen } from "@/hooks/use-theater"
import { buildFlapShape } from "@/lib/flapShape"
import { registerFlapElement } from "@/lib/theaterFlight"
import type { SelectedTarget } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// THE DOCKED FLAP: where the phone's pane actions live when theater is off.
//
// It hangs from the band above the terminal (the tab strip, or the header when
// there is no strip) at the top right, a browser tab turned upside down. The
// header it came out of is now Back, the agent's identity across the whole
// remaining width, and the pull-request chip: on a phone the identity is the
// thing worth reading and four buttons were eating it.
//
// It is dux's own chrome painted OVER the terminal, not part of the terminal,
// so a press on it is never forwarded to the PTY. It sits outside the pane's
// overlay slot deliberately: the overlay is withheld while a full-pane cover
// owns the pane, and the flap is the only surface carrying the `⋯`, the changed
// files and the way into theater. A watcher looking at somebody else's terminal
// still gets to reach them.
export function MobileActionFlap({
  target,
  session,
  band,
  hidden = false,
}: {
  target: SelectedTarget
  /// The agent behind the pane, when there is one. Only the count needs it.
  session: SessionView
  /// What the flap is hanging from, which decides its body color: the tab
  /// strip's own composited tone, or the plain app background when the strip is
  /// not on screen (a single-tab agent, or a hidden top bar).
  band: "strip" | "plain"
  /// MOUNTED BUT NOT PAINTED, which is what the flap is for the whole return
  /// flight: it IS the dock the capsule is flying onto, so the choreography
  /// measures the real element rather than reconstructing where it would have
  /// been, and the final swap therefore moves nothing.
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
      // `z-20` clears the chrome stack's own `z-10`. The flap has to paint over
      // the band's border to interrupt it, and the chrome's layer would
      // otherwise win however the two are ordered in the document.
      className={cn(
        "absolute -top-px right-3 z-20 flex items-center gap-0.5 p-[5px]",
        hidden && "invisible",
        // No drop shadow, deliberately: on the near-black terminal a big soft
        // shadow quantizes into one-step bands whose contours read as a squared
        // ghost box around the flap. The hairline outline is its whole edge
        // treatment.
      )}
      style={
        {
          "--dux-flap-fill":
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
          <path d={shape.fill} fill="var(--dux-flap-fill)" />
          <path
            d={shape.stroke}
            fill="none"
            stroke="var(--border)"
            strokeWidth={1}
          />
        </svg>
      ) : null}

      <MobileActionCluster
        target={target}
        sessionId={session.id}
        theaterRef={theaterRef}
        ellipsis={
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-10 shrink-0 rounded-full"
                  aria-label="Session actions"
                />
              }
            >
              <Ellipsis />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <AgentActionsMenu session={session} context="terminal" />
            </DropdownMenuContent>
          </DropdownMenu>
        }
      />
    </div>
  )
}

/// Measure the flap's own box and turn it into a silhouette.
///
/// The shape is generated rather than drawn once, because the cluster's width
/// is not a constant: the count grows a digit, and a pane with no agent behind
/// it carries no count at all. A fixed path would leave the outline half a
/// button adrift of the buttons it is supposed to be wrapped around.
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
