import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { useSecondTicks } from "@/hooks/use-second-ticks"
import { DOCS_AGENT_TABS_RESUME } from "@/lib/docs"
import { startStoppedAgent } from "@/lib/store"
import { verdictSentenceNow } from "@/lib/tabVerdict"
import type { AgentTabView } from "@/lib/types"

// What a stopped agent shows while its terminals run. Opening it never starts
// it: the band takes the place of its tab strip and says so, and wherever the
// agent part is on screen beside the dock it gets the short card instead of
// the full one, whose tip and large logo would crowd the dock.

// The band's buttons keep their 28px box: a 40px filled Start looked oversized
// in a 36px band, and the band must not grow. A finger gets the 40px from an
// invisible tap area instead, taller only (inset-y, never inset-x), so the two
// buttons' tap areas stay the band's 12px gap apart and cannot overlap. The
// extra 6px above and below reaches past the band by 1.5px onto the strip above
// and the dock divider below, neither of which is a tap target a finger lands
// on beside these buttons.
const BAND_BUTTON =
  "h-7 shrink-0 relative after:absolute after:-inset-y-1.5 after:inset-x-0 after:hidden max-md:after:block pointer-coarse:after:block"

/**
 * The band in place of a stopped agent's tab strip. It carries Start only when
 * nothing else on screen does, which is when the terminal is maximized; beside
 * the short card the card carries it. A first tab whose last run ended badly
 * shows how it ended, with the run's last output one click away.
 */
export function StoppedAgentBand({
  sessionId,
  slotTab,
  withStart,
}: {
  sessionId: string
  slotTab: AgentTabView | undefined
  withStart: boolean
}) {
  const verdict = slotTab?.last_run_failed ? slotTab.last_run_verdict : null
  useSecondTicks(verdict != null)
  const provider = slotTab?.provider ?? "The agent"
  const excerpt = verdict?.excerpt ?? []
  return (
    <div
      data-testid="stopped-agent-band"
      className="flex min-h-9 shrink-0 items-center gap-3 border-b bg-muted/30 px-3 py-1 text-sm"
    >
      <p className="min-w-0 flex-1 truncate">
        <span className="font-mono">{provider}</span> isn&rsquo;t running.
        {verdict ? (
          <span className="text-muted-foreground"> {verdictSentenceNow(verdict)}</span>
        ) : null}
      </p>
      {excerpt.length > 0 ? (
        <Popover>
          <PopoverTrigger
            render={<Button variant="ghost" size="sm" className={BAND_BUTTON} />}
          >
            Last output
          </PopoverTrigger>
          <PopoverContent align="end" className="w-[min(36rem,90vw)]">
            <pre className="max-h-60 overflow-auto whitespace-pre-wrap break-words font-mono text-xs text-muted-foreground">
              {excerpt.join("\n")}
            </pre>
          </PopoverContent>
        </Popover>
      ) : null}
      {withStart && slotTab ? (
        <Button
          size="sm"
          className={BAND_BUTTON}
          onClick={() => startStoppedAgent(sessionId, slotTab.id)}
        >
          Start
        </Button>
      ) : null}
    </div>
  )
}

/**
 * The agent part's card while it shares the screen with its terminals: a small
 * duck, one line, how resume works, and Start.
 */
export function StoppedAgentShortCard({
  sessionId,
  tab,
}: {
  sessionId: string
  tab: AgentTabView
}) {
  return (
    <div
      data-testid="stopped-agent-short-card"
      className="flex h-full w-full select-none flex-col items-center justify-center gap-3 overflow-hidden px-6 text-center"
    >
      <img
        src="/dux-logo.png"
        alt=""
        aria-hidden
        className="size-10 object-contain opacity-70"
      />
      <p className="max-w-md text-sm">
        <span className="font-mono">{tab.provider}</span> isn&rsquo;t running. Start
        picks up its last conversation.{" "}
        <a
          href={DOCS_AGENT_TABS_RESUME}
          target="_blank"
          rel="noopener noreferrer"
          className="text-primary underline underline-offset-2"
        >
          How resume works&nbsp;&rarr;
        </a>
      </p>
      <Button onClick={() => startStoppedAgent(sessionId, tab.id)}>Start</Button>
    </div>
  )
}
