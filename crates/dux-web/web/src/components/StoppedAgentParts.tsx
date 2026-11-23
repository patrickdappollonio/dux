import { NotRunningCard, RunOutput } from "@/components/NotRunningCard"
import { StartAgentButton } from "@/components/StartAgentButton"
import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { useSecondTicks } from "@/hooks/use-second-ticks"
import { startStoppedAgent } from "@/lib/store"
import { chip, prose, renderProse } from "@/lib/prose"
import { verdictSentenceNow } from "@/lib/tabVerdict"
import type { AgentTabView } from "@/lib/types"

// What a stopped agent shows while its terminals run. Opening it never starts
// it: the band takes the place of its tab strip and says so, and wherever the
// agent part is on screen beside the dock it gets the short card instead of
// the full one, whose tip and large logo would crowd the dock.

// The band's buttons keep their 28px box: a 40px filled Start looked oversized.
// A finger gets the 40px from an invisible tap area instead, taller only
// (inset-y, never inset-x), so the two buttons' tap areas stay the band's 12px
// gap apart and cannot overlap. It is 7px above and below, measured from inside
// the 1px border. Under a coarse pointer the band is 44px tall (see
// StoppedAgentBand), so the whole 40px sits inside it, clear of the dock
// divider below.
const BAND_BUTTON =
  "h-7 shrink-0 relative after:absolute after:-inset-y-[7px] after:inset-x-0 after:hidden max-md:after:block pointer-coarse:after:block"

/**
 * The band in place of a stopped agent's tab strip. It carries the agent's
 * Start unless the short card beside the dock carries it; beside another tab's
 * card both are on screen, the card's starting that tab alone. A first tab
 * whose last run ended badly shows how it ended, with the run's last output
 * one click away.
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
  const excerpt = verdict?.excerpt ?? []
  return (
    <div
      data-testid="stopped-agent-band"
      className="flex min-h-9 pointer-coarse:min-h-11 shrink-0 items-center gap-3 border-b bg-muted/30 px-3 py-1 text-sm"
    >
      <p className="min-w-0 flex-1 truncate">
        {slotTab ? renderProse(prose`${chip(slotTab.provider)} isn’t running.`) : "The agent isn’t running."}
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
            <RunOutput lines={excerpt} where="popover" />
          </PopoverContent>
        </Popover>
      ) : null}
      {withStart && slotTab ? (
        <StartAgentButton
          sessionId={sessionId}
          reason="hint"
          size="sm"
          className={BAND_BUTTON}
          label="Start the agent"
          onStart={() => startStoppedAgent(sessionId, slotTab.id)}
        >
          Start
        </StartAgentButton>
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
    <NotRunningCard
      testId="stopped-agent-short-card"
      icon="small"
      title={
        renderProse(prose`${chip(tab.provider)} isn’t running.`)
      }
      details="Start picks up its last conversation."
      start={{
        sessionId,
        onStart: () => startStoppedAgent(sessionId, tab.id),
        label: "Start",
      }}
    />
  )
}
