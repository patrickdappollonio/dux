import { Button } from "@/components/ui/button"
import { DormantTabCard } from "@/components/DormantTabCard"
import { Welcome } from "@/components/Welcome"
import { DOCS_AGENT_TABS_RESUME } from "@/lib/docs"
import { startDormantTab } from "@/lib/store"
import type { TabRunVerdict } from "@/lib/types"

// What a dormant tab looks like, in one decision both shells read:
//   - simply NOT RUNNING is the workspace at rest, so it gets the idle screen
//     the terminal UI paints, plus the one act that starts the tab;
//   - a LAST RUN THAT ENDED BADLY gets the card: what the run did, how long ago,
//     its last output, and the same button.
export function DormantTabSurface({
  sessionId,
  tabId,
  provider,
  lastRunFailed,
  lastRunVerdict,
}: {
  sessionId: string
  tabId: string
  provider: string
  lastRunFailed?: boolean
  lastRunVerdict?: TabRunVerdict | null
}) {
  if (lastRunFailed) {
    return (
      <DormantTabCard
        sessionId={sessionId}
        tabId={tabId}
        provider={provider}
        lastRunFailed
        lastRunVerdict={lastRunVerdict}
      />
    )
  }
  return (
    <Welcome
      action={
        <div className="mt-6 flex max-w-md flex-col items-center gap-3 px-6 text-center">
          <p className="text-sm text-muted-foreground">
            This <span className="font-mono">{provider}</span> tab isn&rsquo;t
            running. Starting it picks up this provider&rsquo;s most recent
            conversation in this worktree, unless another tab of the same
            provider is already running.{" "}
            <a
              href={DOCS_AGENT_TABS_RESUME}
              target="_blank"
              rel="noopener noreferrer"
              className="text-primary underline underline-offset-2"
            >
              How resume works&nbsp;&rarr;
            </a>
          </p>
          {/* Same touch floor as the card's own button; see the note there. */}
          <Button
            onClick={() => startDormantTab(sessionId, tabId)}
            className="max-md:min-h-11"
          >
            Start session
          </Button>
        </div>
      }
    />
  )
}
