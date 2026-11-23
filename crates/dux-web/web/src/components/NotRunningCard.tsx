import type { ReactNode } from "react"

import { StartAgentButton } from "@/components/StartAgentButton"
import { DOCS_AGENT_TABS_RESUME } from "@/lib/docs"
import { cn } from "@/lib/utils"

// The card an agent's part shows when nothing runs in it: the short card beside
// a stopped agent's terminals, the card of a tab whose last run ended badly, and
// the plain dormant tab under the idle screen. They differ in what they say and
// how much room they have; their order, type, spacing, the resume docs link and
// the Start button's touch size are decided here.
//
// The stopped agent's band above the dock is not one of these: it is one line
// that truncates, shows the output in a popover and words a blocked Start as a
// short hint, so it shares the parts (`RunOutput`, `StartAgentButton`) rather
// than the layout.

const ICON_SIZE = {
  large: "size-20",
  small: "size-10",
} as const

export function NotRunningCard({
  icon,
  title,
  subtitle,
  output,
  details,
  start,
  testId,
}: {
  /** The duck: large on a card with the pane to itself, small beside the
   * dock, none where the idle screen above already draws the logo. */
  icon: "large" | "small" | "none"
  /** Which part is not running, in one sentence. */
  title: ReactNode
  /** How the last run ended, when it ended badly. */
  subtitle?: ReactNode
  /** The last lines the run printed; no section when there are none. */
  output?: readonly string[]
  /** What starting it does. The resume docs link follows it. */
  details: ReactNode
  /** The words on the button are the caller's: "Start" starts the agent,
   * "Start session" one of its tabs. */
  start: { sessionId: string; onStart: () => void; label: string }
  testId?: string
}) {
  return (
    <div
      data-testid={testId}
      className="flex h-full w-full select-none flex-col items-center justify-center gap-3 overflow-hidden px-6 text-center"
    >
      {icon === "none" ? null : (
        <img
          src="/dux-logo.png"
          alt=""
          aria-hidden
          className={cn("object-contain opacity-70", ICON_SIZE[icon])}
        />
      )}
      <div className="max-w-md space-y-2">
        <p className="text-sm font-medium">{title}</p>
        {subtitle ? <p className="text-sm text-muted-foreground">{subtitle}</p> : null}
        {output && output.length > 0 ? (
          <div className="space-y-1 text-left">
            <p className="text-xs font-medium text-muted-foreground">Last output</p>
            <RunOutput lines={output} where="card" />
          </div>
        ) : null}
        <p className="text-sm text-muted-foreground">
          {details}{" "}
          <a
            href={DOCS_AGENT_TABS_RESUME}
            target="_blank"
            rel="noopener noreferrer"
            className="text-primary underline underline-offset-2"
          >
            How resume works&nbsp;&rarr;
          </a>
        </p>
      </div>
      {/* The touch floor, at the take-over card's height idiom: that card is
        * the nearest analogue, a full-pane card with one primary act, so
        * matching it keeps the two the same size under a finger. */}
      <StartAgentButton
        sessionId={start.sessionId}
        onStart={start.onStart}
        className="max-md:min-h-11 pointer-coarse:min-h-11"
      >
        {start.label}
      </StartAgentButton>
    </div>
  )
}

/**
 * The last lines a run printed. On a card it is framed and shorter, sitting
 * among other text; in the band's popover the popover is the frame.
 */
export function RunOutput({
  lines,
  where,
}: {
  lines: readonly string[]
  where: "card" | "popover"
}) {
  return (
    <pre
      className={cn(
        "overflow-auto whitespace-pre-wrap break-words font-mono text-xs",
        where === "card"
          ? "max-h-40 rounded-md border border-border/60 bg-muted/40 px-2 py-1.5 text-left text-muted-foreground/80"
          : "max-h-60 text-muted-foreground",
      )}
    >
      {lines.join("\n")}
    </pre>
  )
}
